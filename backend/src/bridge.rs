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

const PROGRAM_LISTEN_PORT: u16 = 10500;
const CUE_LISTEN_PORT: u16 = 10501;
const PAYLOAD_TYPE: u8 = 100;
const POLL_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Copy)]
enum Bus {
    Program,
    Cue,
}

impl Bus {
    fn listen_port(self, slot: Option<u16>) -> u16 {
        let base = match self {
            Bus::Program => PROGRAM_LISTEN_PORT,
            Bus::Cue => CUE_LISTEN_PORT,
        };
        base + slot.unwrap_or(0) * 2
    }

    fn display(self) -> &'static str {
        match self {
            Bus::Program => "Cloud Console Engine",
            Bus::Cue => "Cloud Console Engine Cue",
        }
    }

    fn engine_path(self) -> &'static str {
        match self {
            Bus::Program => "bridge",
            Bus::Cue => "bridge/cue",
        }
    }

    fn room(self, state: &AppState) -> u64 {
        match self {
            Bus::Program => state.janus_room_id,
            Bus::Cue => state.janus_cue_room_id,
        }
    }
}

pub async fn run(state: AppState) {
    tokio::join!(
        run_bus(state.clone(), Bus::Program),
        run_bus(state, Bus::Cue)
    );
}

async fn run_bus(state: AppState, bus: Bus) {
    loop {
        if let Err(error) = session(&state, bus).await {
            tracing::warn!(%error, bus = bus.display(), "Janus audio bridge session ended");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn session(state: &AppState, bus: Bus) -> Result<(), String> {
    let base = state.janus_http_url.trim_end_matches('/').to_owned();
    let engine_ip = if state.engine_host_network_slot.is_some() {
        docker_gateway()?
    } else {
        resolve(&state.audio_engine_url).await?
    };
    let janus_ip = resolve(&base).await?;
    let room = bus.room(state);

    {
        let _guard = state.janus_room_lock.lock().await;
        janus::ensure_audio_room(&state.http_client, &base, room, &state.janus_room_pin).await?;
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
                "room": room,
                "pin": state.janus_room_pin,
                "display": bus.display(),
                "codec": "opus",
                "rtp": { "ip": engine_ip.to_string(), "port": bus.listen_port(state.engine_host_network_slot), "payload_type": PAYLOAD_TYPE }
            }
        }),
    )
    .await?;

    let mut target = joined_target(&joined);
    loop {
        if let Some((port, payload_type)) = target {
            ensure_engine_bridge(state, bus, engine_ip, janus_ip, port, payload_type).await?;
        }
        let event = poll(state, &session_url).await?;
        if let Some(found) = joined_target(&event) {
            tracing::info!(
                port = found.0,
                bus = bus.display(),
                "audio engine joined Janus"
            );
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
    bus: Bus,
    engine_ip: IpAddr,
    janus_ip: IpAddr,
    port: u16,
    payload_type: u8,
) -> Result<(), String> {
    let url = format!(
        "{}/{}",
        state.audio_engine_url.trim_end_matches('/'),
        bus.engine_path()
    );
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
    let listen_address = if state.engine_host_network_slot.is_some() {
        engine_ip.to_string()
    } else {
        "0.0.0.0".to_owned()
    };
    if status["active"] == true
        && status["remote"] == expected.as_str()
        && status["listen_address"] == listen_address.as_str()
        && status["listen_port"] == bus.listen_port(state.engine_host_network_slot)
    {
        return Ok(());
    }
    let response = state
        .http_client
        .put(&url)
        .json(&json!({
            "listen_port": bus.listen_port(state.engine_host_network_slot),
            "listen_address": listen_address,
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

fn docker_gateway() -> Result<IpAddr, String> {
    let routes = std::fs::read_to_string("/proc/net/route")
        .map_err(|error| format!("could not read container routes: {error}"))?;
    parse_default_gateway(&routes)
}

fn parse_default_gateway(routes: &str) -> Result<IpAddr, String> {
    for line in routes.lines().skip(1) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() >= 4 && fields[1] == "00000000" {
            let flags = u32::from_str_radix(fields[3], 16)
                .map_err(|error| format!("invalid route flags: {error}"))?;
            if flags & 2 == 0 {
                continue;
            }
            let gateway = u32::from_str_radix(fields[2], 16)
                .map_err(|error| format!("invalid gateway address: {error}"))?;
            if gateway != 0 {
                return Ok(std::net::Ipv4Addr::from(gateway.to_ne_bytes()).into());
            }
        }
    }
    Err("container has no IPv4 default gateway for the audio bridge".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_bridge_gateway_and_requires_a_gateway_route() {
        let routes = "Iface Destination Gateway Flags RefCnt Use Metric Mask\n\
                      eth0 00000000 010013AC 0003 0 0 0 00000000\n";
        assert_eq!(
            parse_default_gateway(routes).unwrap(),
            "172.19.0.1".parse::<IpAddr>().unwrap()
        );
        assert!(parse_default_gateway(
            "Iface Destination Gateway Flags\neth0 00000000 00000000 0001"
        )
        .is_err());
    }

    #[test]
    fn assigns_non_overlapping_bridge_ports_to_four_slots() {
        let ports: Vec<_> = (0..4)
            .flat_map(|slot| {
                [
                    Bus::Program.listen_port(Some(slot)),
                    Bus::Cue.listen_port(Some(slot)),
                ]
            })
            .collect();
        assert_eq!(
            ports,
            vec![10500, 10501, 10502, 10503, 10504, 10505, 10506, 10507]
        );
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
