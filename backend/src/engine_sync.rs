use std::{collections::HashSet, time::Duration};

use serde_json::{json, Value};
use sqlx::Row;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::AppState;

// Serialises engine mutations so reconciliation never races a config change.
static SYNC_LOCK: Mutex<()> = Mutex::const_new(());

fn engine_url(state: &AppState, path: &str) -> String {
    format!(
        "{}/{}",
        state.audio_engine_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub async fn run(state: AppState) {
    loop {
        if let Err(error) = reconcile(&state).await {
            tracing::warn!(%error, "audio engine reconciliation failed");
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

pub async fn apply_input(state: &AppState, id: Uuid) -> Result<(), String> {
    let _guard = SYNC_LOCK.lock().await;
    remove_engine_resource(state, "sources", id).await?;
    add_input(state, id).await
}

pub async fn apply_output(state: &AppState, id: Uuid) -> Result<(), String> {
    let _guard = SYNC_LOCK.lock().await;
    remove_engine_resource(state, "outputs", id).await?;
    add_output(state, id).await
}

pub async fn remove_input(state: &AppState, id: Uuid) -> Result<(), String> {
    let _guard = SYNC_LOCK.lock().await;
    remove_engine_resource(state, "sources", id).await
}

pub async fn remove_output(state: &AppState, id: Uuid) -> Result<(), String> {
    let _guard = SYNC_LOCK.lock().await;
    remove_engine_resource(state, "outputs", id).await
}

pub async fn apply_source_mix(state: &AppState, id: Uuid) -> Result<(), String> {
    let row = sqlx::query(
        "SELECT fader, is_on, muted, cue_enabled FROM mixer_channel_state WHERE station_id = $1 AND channel_id = $2",
    )
    .bind(state.station_id)
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| error.to_string())?;
    let (volume, muted, cue) = match row {
        Some(row) => (
            row.get::<f64, _>("fader"),
            !row.get::<bool, _>("is_on") || row.get::<bool, _>("muted"),
            row.get::<bool, _>("cue_enabled"),
        ),
        None => (1.0, false, false),
    };
    let response = state
        .http_client
        .patch(engine_url(state, &format!("sources/{id}/mix")))
        .json(&json!({ "volume": volume, "muted": muted, "cue": cue }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(format!(
            "engine rejected mix update ({})",
            response.status()
        ))
    }
}

async fn reconcile(state: &AppState) -> Result<(), String> {
    let _guard = SYNC_LOCK.lock().await;
    let engine_sources = engine_ids(state, "sources").await?;
    let engine_outputs = engine_ids(state, "outputs").await?;

    let wanted_inputs = enabled_ids(state, "inputs").await?;
    let wanted_outputs = enabled_ids(state, "outputs").await?;

    for id in engine_sources.difference(&wanted_inputs) {
        remove_engine_resource(state, "sources", *id).await?;
    }
    for id in engine_outputs.difference(&wanted_outputs) {
        remove_engine_resource(state, "outputs", *id).await?;
    }
    for id in wanted_inputs.difference(&engine_sources) {
        if let Err(error) = add_input(state, *id).await {
            tracing::warn!(%id, %error, "could not start configured audio input");
        }
    }
    for id in wanted_outputs.difference(&engine_outputs) {
        if let Err(error) = add_output(state, *id).await {
            tracing::warn!(%id, %error, "could not start configured audio output");
        }
    }
    Ok(())
}

async fn engine_ids(state: &AppState, collection: &str) -> Result<HashSet<Uuid>, String> {
    let items: Vec<Value> = state
        .http_client
        .get(engine_url(state, collection))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    Ok(items
        .iter()
        .filter_map(|item| item.get("id")?.as_str()?.parse().ok())
        .collect())
}

async fn enabled_ids(state: &AppState, table: &str) -> Result<HashSet<Uuid>, String> {
    let query = match table {
        "inputs" => "SELECT id FROM inputs WHERE station_id = $1 AND enabled",
        _ => "SELECT id FROM outputs WHERE station_id = $1 AND enabled",
    };
    sqlx::query_scalar::<_, Uuid>(query)
        .bind(state.station_id)
        .fetch_all(&state.pool)
        .await
        .map(|ids| ids.into_iter().collect())
        .map_err(|error| error.to_string())
}

async fn remove_engine_resource(
    state: &AppState,
    collection: &str,
    id: Uuid,
) -> Result<(), String> {
    let response = state
        .http_client
        .delete(engine_url(state, &format!("{collection}/{id}")))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(format!(
            "engine could not remove {collection} ({})",
            response.status()
        ))
    }
}

async fn add_input(state: &AppState, id: Uuid) -> Result<(), String> {
    let row = sqlx::query(
        "SELECT name, source_type, enabled, config FROM inputs WHERE id = $1 AND station_id = $2",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| error.to_string())?;
    let Some(row) = row else { return Ok(()) };
    if !row.get::<bool, _>("enabled") {
        return Ok(());
    }
    let config: Value = row.get("config");
    post_engine(
        state,
        "sources",
        json!({
            "id": id,
            "name": row.get::<String, _>("name"),
            "source_type": row.get::<String, _>("source_type"),
            "transport": config,
        }),
    )
    .await?;
    apply_source_mix(state, id).await
}

async fn add_output(state: &AppState, id: Uuid) -> Result<(), String> {
    let row = sqlx::query(
        "SELECT name, output_type, enabled, config FROM outputs WHERE id = $1 AND station_id = $2",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| error.to_string())?;
    let Some(row) = row else { return Ok(()) };
    if !row.get::<bool, _>("enabled") {
        return Ok(());
    }
    let mut transport: Value = row.get("config");
    if let Some(object) = transport.as_object_mut() {
        object
            .entry("type")
            .or_insert_with(|| json!(row.get::<String, _>("output_type")));
    }
    post_engine(
        state,
        "outputs",
        json!({
            "id": id,
            "name": row.get::<String, _>("name"),
            "transport": transport,
        }),
    )
    .await
}

async fn post_engine(state: &AppState, collection: &str, body: Value) -> Result<(), String> {
    let response = state
        .http_client
        .post(engine_url(state, collection))
        .json(&body)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let detail = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| status.to_string());
    Err(detail)
}
