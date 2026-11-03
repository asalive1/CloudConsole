use serde_json::{json, Value};
use uuid::Uuid;

pub async fn ensure_audio_room(
    client: &reqwest::Client,
    base_url: &str,
    room_id: u64,
    pin: &str,
) -> Result<(), String> {
    let base_url = base_url.trim_end_matches('/');
    let session = request(
        client,
        base_url,
        json!({
            "janus": "create",
            "transaction": Uuid::new_v4().to_string()
        }),
    )
    .await?;
    let session_id = session
        .pointer("/data/id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Janus session response did not contain an id".to_owned())?;

    let result = ensure_room_in_session(client, base_url, session_id, room_id, pin).await;
    let destroy = client
        .post(format!("{base_url}/{session_id}"))
        .json(&json!({
            "janus": "destroy",
            "transaction": Uuid::new_v4().to_string()
        }))
        .send()
        .await;
    if let Err(error) = destroy {
        tracing::warn!(%error, "failed to close temporary Janus control session");
    }
    result
}

async fn ensure_room_in_session(
    client: &reqwest::Client,
    base_url: &str,
    session_id: u64,
    room_id: u64,
    pin: &str,
) -> Result<(), String> {
    let session_url = format!("{base_url}/{session_id}");
    let attached = request(
        client,
        &session_url,
        json!({
            "janus": "attach",
            "plugin": "janus.plugin.videoroom",
            "transaction": Uuid::new_v4().to_string()
        }),
    )
    .await?;
    let handle_id = attached
        .pointer("/data/id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Janus plugin response did not contain a handle id".to_owned())?;
    let handle_url = format!("{session_url}/{handle_id}");

    let exists = plugin_request(
        client,
        &handle_url,
        json!({ "request": "exists", "room": room_id }),
    )
    .await?;
    if exists
        .pointer("/plugindata/data/exists")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Ok(());
    }

    let create = plugin_request(
        client,
        &handle_url,
        json!({
            "request": "create",
            "room": room_id,
            "description": "Cloud Console live audio",
            "publishers": 8,
            "pin": pin,
            "is_private": true,
            "audiocodec": "opus",
            "permanent": false
        }),
    )
    .await;
    if create.is_ok() {
        return Ok(());
    }

    let exists_after_create = plugin_request(
        client,
        &handle_url,
        json!({ "request": "exists", "room": room_id }),
    )
    .await?;
    if exists_after_create
        .pointer("/plugindata/data/exists")
        .and_then(Value::as_bool)
        == Some(true)
    {
        Ok(())
    } else {
        create.map(|_| ())
    }
}

async fn plugin_request(client: &reqwest::Client, url: &str, body: Value) -> Result<Value, String> {
    request(
        client,
        url,
        json!({
            "janus": "message",
            "body": body,
            "transaction": Uuid::new_v4().to_string()
        }),
    )
    .await
}

async fn request(client: &reqwest::Client, url: &str, body: Value) -> Result<Value, String> {
    let response = client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Janus request failed: {error}"))?;
    let status = response.status();
    let response: Value = response
        .json()
        .await
        .map_err(|error| format!("invalid Janus response: {error}"))?;
    if !status.is_success() || response["janus"] == "error" {
        return Err(format!(
            "Janus returned an error: {}",
            response
                .pointer("/error/reason")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
        ));
    }
    Ok(response)
}
