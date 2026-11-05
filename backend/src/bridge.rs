//! Keeps the audio engine registered in the Janus Program room as a plain-RTP
//! participant. Janus never mixes a participant's own audio back to it, so the
//! engine hears every microphone but not its own sources (mix-minus), and the
//! browsers hear the microphones plus the engine's automation and aux sources.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::{janus, AppState};

const ENGINE_LISTEN_PORT: u16 = 10500;
const PAYLOAD_TYPE: u8 = 100;
const POLL_TIMEOUT: Duration = Duration::from_secs(12);

pub async fn run(state: AppState) {
    loop {
        if let Err(error) = session(&state).await {
            tracing::warn!(%error, "Janus audio bridge session ended");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn session(state: &AppState) -> Result<(), String> {
    let base = state.janus_http_url.trim_end_matches('/').to_owned();
    let engine_ip = resolve(&state.audio_engine_url).await?;
    let janus_ip = resolve(&base).await?;

    {
        let _guard = state.janus_room_lock.lock().await;
        janus::ensure_audio_room(
            &state.http_client,
            &base,
            state.janus_room_id,
            &state.janus_room_pin,
        )
        .await?;
    }

    let created = post(state, &base, json!({ "janus": "create" })).await?;
    let session_id = created
        .pointer("/data/id")
        .and_then(Value::as_u64)
        .ok_or("Janus session response did not contain an id")?;
    let session_url = format!("{base}/{session_id}");
    let attached = post(
        state,
        &session_url,
        json!({ "janus": "attach", "plugin": "janus.plugin.audiobridge" }),
    )
    .await?;
    let handle_id = attached
        .pointer("/data/id")
        .and_then(Value::as_u64)
        .ok_or("Janus plugin response did not contain a handle id")?;

    let joined = post(
        state,
        &format!("{session_url}/{handle_id}"),
        json!({
            "janus": "message",
            "body": {
                "request": "join",
                "room": state.janus_room_id,
                "pin": state.janus_room_pin,
                "display": "Cloud Console Engine",
                "codec": "opus",
                "rtp": { "ip": engine_ip.to_string(), "port": ENGINE_LISTEN_PORT, "payload_type": PAYLOAD_TYPE }
            }
        }),
    )
    .await?;

    let mut target = joined_target(&joined);
    loop {
        if let Some((port, payload_type)) = target {
            ensure_engine_bridge(state, janus_ip, port, payload_type).await?;
        }
        let event = poll(state, &session_url).await?;
        if let Some(found) = joined_target(&event) {
            tracing::info!(port = found.0, "audio engine joined the Janus Program room");
            target = Some(found);
        }
        if event.pointer("/plugindata/data/error_code").is_some() {
            return Err(format!(
                "Janus rejected the engine participant: {}",
                event
                    .pointer("/plugindata/data/error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            ));
        }
        if matches!(
            event
                .pointer("/plugindata/data/audiobridge")
                .and_then(Value::as_str),
            Some("destroyed" | "kicked")
        ) {
            return Err("the Janus room was closed".to_owned());
        }
    }
}

fn joined_target(response: &Value) -> Option<(u16, u8)> {
    let data = response.pointer("/plugindata/data")?;
    if data.get("audiobridge")?.as_str()? != "joined" {
        return None;
    }
    let port = u16::try_from(data.pointer("/rtp/port")?.as_u64()?).ok()?;
    let payload_type = data
        .pointer("/rtp/audio_pt")
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(PAYLOAD_TYPE);
    Some((port, payload_type))
}

async fn ensure_engine_bridge(
    state: &AppState,
    janus_ip: IpAddr,
    port: u16,
    payload_type: u8,
) -> Result<(), String> {
    let url = format!("{}/bridge", state.audio_engine_url.trim_end_matches('/'));
    let status: Value = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let expected = format!("{janus_ip}:{port}");
    if status["active"] == true && status["remote"] == expected.as_str() {
        return Ok(());
    }
    let response = state
        .http_client
        .put(&url)
        .json(&json!({
            "listen_port": ENGINE_LISTEN_PORT,
            "remote_host": janus_ip.to_string(),
            "remote_port": port,
            "payload_type": payload_type,
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        tracing::info!(remote = %expected, "audio engine bridge started");
        Ok(())
    } else {
        Err(format!(
            "engine rejected the bridge ({})",
            response.status()
        ))
    }
}

async fn resolve(url: &str) -> Result<IpAddr, String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    let host = parsed.host_str().ok_or("URL has no host")?.to_owned();
    let mut addresses = tokio::net::lookup_host((host.as_str(), 0))
        .await
        .map_err(|error| format!("could not resolve {host}: {error}"))?;
    addresses
        .find(SocketAddr::is_ipv4)
        .map(|address| address.ip())
        .ok_or_else(|| format!("{host} has no IPv4 address"))
}

async fn post(state: &AppState, url: &str, mut body: Value) -> Result<Value, String> {
    body["transaction"] = json!(Uuid::new_v4().to_string());
    let response = state
        .http_client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Janus request failed: {error}"))?;
    parse(response).await
}

async fn poll(state: &AppState, session_url: &str) -> Result<Value, String> {
    match state
        .http_client
        .get(format!("{session_url}?maxev=1"))
        .timeout(POLL_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => parse(response).await,
        Err(error) if error.is_timeout() => Ok(Value::Null),
        Err(error) => Err(format!("Janus poll failed: {error}")),
    }
}

async fn parse(response: reqwest::Response) -> Result<Value, String> {
    let value: Value = response
        .json()
        .await
        .map_err(|error| format!("invalid Janus response: {error}"))?;
    if value["janus"] == "error" {
        return Err(format!(
            "Janus returned an error: {}",
            value
                .pointer("/error/reason")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
        ));
    }
    Ok(value)
}
