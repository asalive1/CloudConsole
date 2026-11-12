use axum::{
    body::{to_bytes, Body},
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        OriginalUri, Path, Query, State,
    },
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("token operation failed")]
    Token(#[from] jsonwebtoken::errors::Error),
    #[error("audio engine is unavailable")]
    AudioEngine(#[from] reqwest::Error),
    #[error("Janus Gateway is unavailable: {0}")]
    Janus(String),
    #[error("audio engine rejected the delay request: {0}")]
    AudioEngineMessage(String),
    #[error("configuration saved but not applied: {0}")]
    SavedButNotApplied(String),
    #[error("configuration deleted but not applied: {0}")]
    DeletedButNotApplied(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_owned()),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message),
            ApiError::InvalidRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::Database(error) => {
                tracing::error!(error = %error, "database request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database request failed".to_owned(),
                )
            }
            ApiError::Token(error) => {
                tracing::error!(error = %error, "guest token operation failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "guest token operation failed".to_owned(),
                )
            }
            ApiError::AudioEngine(error) => {
                tracing::error!(error = %error, "audio engine request failed");
                (
                    StatusCode::BAD_GATEWAY,
                    "audio engine is unavailable".to_owned(),
                )
            }
            ApiError::Janus(error) => {
                tracing::error!(error = %error, "Janus Gateway request failed");
                (
                    StatusCode::BAD_GATEWAY,
                    "live audio service is unavailable".to_owned(),
                )
            }
            ApiError::AudioEngineMessage(error) => {
                tracing::error!(error = %error, "audio delay engine request failed");
                (
                    StatusCode::BAD_GATEWAY,
                    "audio delay engine is unavailable".to_owned(),
                )
            }
            ApiError::SavedButNotApplied(error) => {
                tracing::warn!(%error, "saved configuration was not applied");
                (
                    StatusCode::BAD_GATEWAY,
                    format!("Configuration was saved but could not be applied: {error}. Edit the saved item to correct it."),
                )
            }
            ApiError::DeletedButNotApplied(error) => {
                tracing::warn!(%error, "deleted configuration was not removed from the engine");
                (
                    StatusCode::BAD_GATEWAY,
                    format!("Configuration was deleted but the engine could not apply the deletion: {error}. It will retry automatically."),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub async fn proxy_audio(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    request: axum::http::Request<Body>,
) -> Result<Response, ApiError> {
    authorize_station(&headers, &state)?;
    let path = uri
        .path()
        .strip_prefix("/api/audio/")
        .ok_or_else(|| ApiError::InvalidRequest("invalid audio engine path".to_owned()))?;
    let mut upstream_url = format!("{}/{}", state.audio_engine_url.trim_end_matches('/'), path);
    if let Some(query) = uri.query() {
        upstream_url.push('?');
        upstream_url.push_str(query);
    }

    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 1_048_576)
        .await
        .map_err(|error| ApiError::InvalidRequest(error.to_string()))?;
    let mut upstream_request = state.http_client.request(
        Method::from_bytes(parts.method.as_str().as_bytes())
            .map_err(|_| ApiError::InvalidRequest("invalid HTTP method".to_owned()))?,
        upstream_url,
    );
    if let Some(content_type) = parts.headers.get(header::CONTENT_TYPE) {
        upstream_request = upstream_request.header(header::CONTENT_TYPE, content_type);
    }
    let upstream = upstream_request.body(body).send().await?;
    let status = StatusCode::from_u16(upstream.status().as_u16())
        .map_err(|_| ApiError::InvalidRequest("invalid upstream status".to_owned()))?;
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let response_body = upstream.bytes().await?;
    let mut response = Response::builder().status(status);
    if let Some(content_type) = content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    response
        .body(Body::from(response_body))
        .map_err(|error| ApiError::InvalidRequest(error.to_string()))
}

#[derive(Deserialize)]
pub struct ConfigureDelayRequest {
    delay_seconds: Option<u32>,
    enabled: Option<bool>,
}

async fn delay_enabled(state: &AppState) -> Result<bool, ApiError> {
    let enabled: Option<bool> =
        sqlx::query_scalar("SELECT enabled FROM station_delay_settings WHERE station_id = $1")
            .bind(state.station_id)
            .fetch_optional(&state.pool)
            .await?;
    Ok(enabled.unwrap_or(true))
}

async fn delay_status_with_enabled(state: &AppState) -> Result<Json<Value>, ApiError> {
    let mut status = delay_engine_request(state, reqwest::Method::GET, "", None).await?;
    if let Some(object) = status.as_object_mut() {
        object.insert("enabled".to_owned(), json!(delay_enabled(state).await?));
    }
    Ok(Json(status))
}

pub async fn get_delay_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_delay_operator(&headers, &state).await?;
    delay_status_with_enabled(&state).await
}

pub async fn configure_delay(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ConfigureDelayRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize_delay_operator(&headers, &state).await?;
    if let Some(seconds) = request.delay_seconds {
        if !(5..=60).contains(&seconds) {
            return Err(ApiError::InvalidRequest(
                "delay_seconds must be between 5 and 60".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO station_delay_settings (station_id, delay_seconds) VALUES ($1, $2) ON CONFLICT (station_id) DO UPDATE SET delay_seconds = EXCLUDED.delay_seconds, updated_at = NOW()",
        )
        .bind(state.station_id)
        .bind(seconds as i16)
        .execute(&state.pool)
        .await?;
        delay_engine_request(
            &state,
            reqwest::Method::PATCH,
            "/config",
            Some(json!({ "delay_seconds": seconds })),
        )
        .await?;
    }
    if let Some(enabled) = request.enabled {
        sqlx::query(
            "INSERT INTO station_delay_settings (station_id, enabled) VALUES ($1, $2) ON CONFLICT (station_id) DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = NOW()",
        )
        .bind(state.station_id)
        .bind(enabled)
        .execute(&state.pool)
        .await?;
        if !enabled {
            // Drain any buffered audio so a hidden delay cannot keep Program late.
            let _ = delay_engine_request(&state, reqwest::Method::POST, "/end", None).await;
        }
    }
    delay_status_with_enabled(&state).await
}

pub async fn dump_delay(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_delay_operator(&headers, &state).await?;
    Ok(Json(
        delay_engine_request(&state, reqwest::Method::POST, "/dump", None).await?,
    ))
}

pub async fn build_delay(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_delay_operator(&headers, &state).await?;
    Ok(Json(
        delay_engine_request(&state, reqwest::Method::POST, "/build", None).await?,
    ))
}

pub async fn end_delay(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_delay_operator(&headers, &state).await?;
    Ok(Json(
        delay_engine_request(&state, reqwest::Method::POST, "/end", None).await?,
    ))
}

async fn authorize_delay_operator(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    if headers.contains_key("x-api-key") {
        return authorize_station(headers, state);
    }
    let claims = authorize_participant(headers, state)?;
    if !matches!(claims.role.as_str(), "host" | "co_host") {
        return Err(ApiError::Unauthorized);
    }
    let session_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE id = $1 AND station_id = $2 AND role = $3 AND status IN ('approved', 'connected', 'disconnected') AND expires_at > NOW())",
    )
    .bind(session_id)
    .bind(state.station_id)
    .bind(&claims.role)
    .fetch_one(&state.pool)
    .await?;
    if !active {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

async fn delay_engine_request(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, ApiError> {
    let url = format!(
        "{}/delay{path}",
        state.audio_engine_url.trim_end_matches('/')
    );
    let mut request = state.http_client.request(method, url);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let response: Value = response.json().await.map_err(|error| {
        ApiError::AudioEngineMessage(format!("invalid engine response: {error}"))
    })?;
    if status.is_success() {
        return Ok(response);
    }
    let message = response
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("delay request failed")
        .to_owned();
    match status {
        StatusCode::BAD_REQUEST => Err(ApiError::InvalidRequest(message)),
        StatusCode::CONFLICT => Err(ApiError::Conflict(message)),
        _ => Err(ApiError::AudioEngineMessage(message)),
    }
}

pub async fn restore_delay_settings(state: &AppState) -> Result<(), ApiError> {
    let client = state.http_client.clone();
    let pool = state.pool.clone();
    let station_id = state.station_id;
    let url = format!(
        "{}/delay/config",
        state.audio_engine_url.trim_end_matches('/')
    );
    tokio::spawn(async move {
        let mut retry_seconds = 1u64;
        loop {
            let delay_seconds: i16 = match sqlx::query_scalar(
                "SELECT delay_seconds FROM station_delay_settings WHERE station_id = $1",
            )
            .bind(station_id)
            .fetch_one(&pool)
            .await
            {
                Ok(delay_seconds) => delay_seconds,
                Err(error) => {
                    tracing::error!(%error, "could not load persisted Program delay");
                    tokio::time::sleep(std::time::Duration::from_secs(retry_seconds)).await;
                    retry_seconds = (retry_seconds * 2).min(30);
                    continue;
                }
            };
            match client
                .patch(&url)
                .json(&json!({ "delay_seconds": delay_seconds }))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    tracing::info!(delay_seconds, "restored persisted Program delay");
                    break;
                }
                Ok(response) => {
                    tracing::warn!(
                        status = %response.status(),
                        "audio engine has not accepted persisted delay settings yet"
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "audio engine is not available for delay restoration");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(retry_seconds)).await;
            retry_seconds = (retry_seconds * 2).min(30);
        }
    });
    Ok(())
}

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn ready(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query("SELECT 1").execute(&state.pool).await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

pub async fn openapi() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        include_str!("../openapi.json"),
    )
        .into_response()
}

pub async fn api_docs() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        r#"<!doctype html><html><head><title>Cloud Console API</title></head><body><h1>Cloud Console API</h1><p>OpenAPI document: <a href="/api/openapi.json">/api/openapi.json</a></p></body></html>"#,
    )
        .into_response()
}

#[derive(Serialize)]
pub struct AuthResponse {
    authenticated: bool,
    station_id: Uuid,
    role: String,
    status: Option<String>,
}

pub async fn auth_login(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthResponse>, ApiError> {
    authorize_station(&headers, &state)?;
    Ok(Json(AuthResponse {
        authenticated: true,
        station_id: state.station_id,
        role: "station".to_owned(),
        status: None,
    }))
}

pub async fn auth_validate(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthResponse>, ApiError> {
    if headers.contains_key("x-api-key") {
        authorize_station(&headers, &state)?;
        return Ok(Json(AuthResponse {
            authenticated: true,
            station_id: state.station_id,
            role: "station".to_owned(),
            status: None,
        }));
    }

    let claims = authorize_participant(&headers, &state)?;
    let session_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
    let session: Option<(String, String)> = sqlx::query_as(
        "SELECT role, status FROM sessions WHERE id = $1 AND station_id = $2 AND expires_at > NOW()",
    )
    .bind(session_id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await?;
    let (role, status) = session.ok_or(ApiError::Unauthorized)?;
    if role != claims.role {
        return Err(ApiError::Unauthorized);
    }
    Ok(Json(AuthResponse {
        authenticated: true,
        station_id: state.station_id,
        role: claims.role,
        status: Some(status),
    }))
}

#[derive(Serialize)]
pub struct StationConfig {
    id: Uuid,
    name: String,
    inputs: Vec<Value>,
    outputs: Vec<Value>,
    metadata: Vec<Value>,
    delay: Vec<Value>,
    routing_rules: Vec<Value>,
}

pub async fn station_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StationConfig>, ApiError> {
    authorize_station(&headers, &state)?;
    let station = sqlx::query("SELECT name FROM stations WHERE id = $1")
        .bind(state.station_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(StationConfig {
        id: state.station_id,
        name: station.try_get("name")?,
        inputs: rows_as_json(&state.pool, "inputs", state.station_id).await?,
        outputs: rows_as_json(&state.pool, "outputs", state.station_id).await?,
        metadata: rows_as_json(&state.pool, "metadata_configs", state.station_id).await?,
        delay: rows_as_json(&state.pool, "delay_configs", state.station_id).await?,
        routing_rules: rows_as_json(&state.pool, "routing_rules", state.station_id).await?,
    }))
}

#[derive(Deserialize)]
pub struct StationSettingsRequest {
    name: String,
}

pub async fn update_station_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<StationSettingsRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize_station(&headers, &state)?;
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::InvalidRequest(
            "station name must contain between 1 and 100 characters".to_owned(),
        ));
    }
    let row = sqlx::query(
        "UPDATE stations SET name = $1, updated_at = NOW() WHERE id = $2 RETURNING id, name",
    )
    .bind(name)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(Json(json!({
        "id": row.get::<Uuid, _>("id"),
        "name": row.get::<String, _>("name")
    })))
}

#[derive(Deserialize)]
pub struct StationResourceRequest {
    name: Option<String>,
    resource_type: Option<String>,
    direction: Option<String>,
    format: Option<String>,
    enabled: Option<bool>,
    config: Option<Value>,
}

fn station_resource_table(kind: &str) -> Result<&'static str, ApiError> {
    match kind {
        "inputs" => Ok("inputs"),
        "outputs" => Ok("outputs"),
        "metadata" => Ok("metadata_configs"),
        "routes" => Ok("routing_rules"),
        _ => Err(ApiError::NotFound),
    }
}

fn validate_setting_text(value: &str, field: &str, max_length: usize) -> Result<(), ApiError> {
    if value.trim().is_empty() || value.len() > max_length {
        return Err(ApiError::InvalidRequest(format!(
            "{field} must contain between 1 and {max_length} characters"
        )));
    }
    Ok(())
}

fn validate_station_resource(request: &StationResourceRequest, kind: &str) -> Result<(), ApiError> {
    if let Some(name) = request.name.as_deref() {
        validate_setting_text(name, "name", 100)?;
    }
    if let Some(resource_type) = request.resource_type.as_deref() {
        validate_setting_text(resource_type, "resource_type", 40)?;
    }
    if let Some(direction) = request.direction.as_deref() {
        if !matches!(direction, "input" | "output") {
            return Err(ApiError::InvalidRequest(
                "direction must be input or output".to_owned(),
            ));
        }
    }
    if let Some(format) = request.format.as_deref() {
        validate_setting_text(format, "format", 40)?;
    }
    if request
        .config
        .as_ref()
        .is_some_and(|config| !config.is_object())
    {
        return Err(ApiError::InvalidRequest(
            "config must be a JSON object".to_owned(),
        ));
    }
    if kind == "metadata" && request.direction.is_some() != request.format.is_some() {
        return Err(ApiError::InvalidRequest(
            "metadata direction and format must be provided together".to_owned(),
        ));
    }
    match kind {
        "inputs" => {
            if let Some(resource_type) = request.resource_type.as_deref() {
                if !matches!(resource_type.trim(), "automation" | "aux") {
                    return Err(ApiError::InvalidRequest(
                        "input type must be automation or aux".to_owned(),
                    ));
                }
            }
            validate_transport_type(request.config.as_ref(), &["alsa", "srt", "rtp", "test"])?;
        }
        "outputs" => {
            if let Some(resource_type) = request.resource_type.as_deref() {
                if !matches!(resource_type.trim(), "alsa" | "srt" | "rtp") {
                    return Err(ApiError::InvalidRequest(
                        "output type must be alsa, srt, or rtp".to_owned(),
                    ));
                }
            }
            validate_transport_type(request.config.as_ref(), &["alsa", "srt", "rtp"])?;
        }
        "metadata" => validate_metadata_config(request.config.as_ref())?,
        _ => {}
    }
    Ok(())
}

fn validate_transport_type(config: Option<&Value>, allowed: &[&str]) -> Result<(), ApiError> {
    let Some(config) = config else { return Ok(()) };
    match config.get("type").and_then(Value::as_str) {
        Some(kind) if allowed.contains(&kind) => Ok(()),
        _ => Err(ApiError::InvalidRequest(format!(
            "config.type must be one of: {}",
            allowed.join(", ")
        ))),
    }
}

fn validate_metadata_config(config: Option<&Value>) -> Result<(), ApiError> {
    let Some(config) = config else { return Ok(()) };
    let mode = config.get("mode").and_then(Value::as_str);
    if !matches!(mode, Some("server" | "client")) {
        return Err(ApiError::InvalidRequest(
            "metadata mode must be server or client".to_owned(),
        ));
    }
    let port = config.get("port").and_then(Value::as_u64);
    if !port.is_some_and(|port| (1..=65535).contains(&port)) {
        return Err(ApiError::InvalidRequest(
            "metadata port must be between 1 and 65535".to_owned(),
        ));
    }
    if mode == Some("client")
        && config
            .get("host")
            .and_then(Value::as_str)
            .is_none_or(|host| host.trim().is_empty() || host.len() > 253)
    {
        return Err(ApiError::InvalidRequest(
            "metadata client mode requires a host".to_owned(),
        ));
    }
    Ok(())
}

pub async fn list_station_resources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    authorize_station(&headers, &state)?;
    let table = station_resource_table(&kind)?;
    Ok(Json(
        rows_as_json(&state.pool, table, state.station_id).await?,
    ))
}

pub async fn create_station_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Json(request): Json<StationResourceRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    authorize_station(&headers, &state)?;
    station_resource_table(&kind)?;
    validate_station_resource(&request, &kind)?;
    let name = request
        .name
        .as_deref()
        .ok_or_else(|| ApiError::InvalidRequest("name is required".to_owned()))?
        .trim();
    let config = sqlx::types::Json(request.config.unwrap_or_else(|| json!({})));
    let id = Uuid::new_v4();
    match kind.as_str() {
        "inputs" => {
            let resource_type = request
                .resource_type
                .as_deref()
                .ok_or_else(|| ApiError::InvalidRequest("resource_type is required".to_owned()))?;
            if config.0.get("type").is_none() {
                return Err(ApiError::InvalidRequest(
                    "config.type is required".to_owned(),
                ));
            }
            let existing: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM inputs WHERE station_id = $1 AND source_type = $2",
            )
            .bind(state.station_id)
            .bind(resource_type.trim())
            .fetch_one(&state.pool)
            .await?;
            if (resource_type.trim() == "automation" && existing >= 1) || existing >= 5 {
                return Err(ApiError::Conflict(
                    "a station supports one Automation input and up to five AUX inputs".to_owned(),
                ));
            }
            sqlx::query(
                "INSERT INTO inputs (id, station_id, name, source_type, enabled, config) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(state.station_id)
            .bind(name)
            .bind(resource_type.trim())
            .bind(request.enabled.unwrap_or(true))
            .bind(config)
            .execute(&state.pool)
            .await?;
        }
        "outputs" => {
            let resource_type = request
                .resource_type
                .as_deref()
                .ok_or_else(|| ApiError::InvalidRequest("resource_type is required".to_owned()))?;
            sqlx::query(
                "INSERT INTO outputs (id, station_id, name, output_type, enabled, config) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(state.station_id)
            .bind(name)
            .bind(resource_type.trim())
            .bind(request.enabled.unwrap_or(true))
            .bind(config)
            .execute(&state.pool)
            .await?;
        }
        "metadata" => {
            let direction = request
                .direction
                .as_deref()
                .ok_or_else(|| ApiError::InvalidRequest("direction is required".to_owned()))?;
            let format = request
                .format
                .as_deref()
                .ok_or_else(|| ApiError::InvalidRequest("format is required".to_owned()))?;
            sqlx::query(
                "INSERT INTO metadata_configs (id, station_id, name, direction, format, enabled, config) VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(id)
            .bind(state.station_id)
            .bind(name)
            .bind(direction)
            .bind(format.trim())
            .bind(request.enabled.unwrap_or(true))
            .bind(config)
            .execute(&state.pool)
            .await?;
        }
        "routes" => {
            sqlx::query(
                "INSERT INTO routing_rules (id, station_id, name, enabled, config) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(state.station_id)
            .bind(name)
            .bind(request.enabled.unwrap_or(true))
            .bind(config)
            .execute(&state.pool)
            .await?;
        }
        _ => return Err(ApiError::NotFound),
    }
    sync_engine_resource(&state, &kind, id, false)
        .await
        .map_err(ApiError::SavedButNotApplied)?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn update_station_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, Uuid)>,
    Json(request): Json<StationResourceRequest>,
) -> Result<StatusCode, ApiError> {
    authorize_station(&headers, &state)?;
    station_resource_table(&kind)?;
    validate_station_resource(&request, &kind)?;
    if request.name.is_none()
        && request.resource_type.is_none()
        && request.direction.is_none()
        && request.format.is_none()
        && request.enabled.is_none()
        && request.config.is_none()
    {
        return Err(ApiError::InvalidRequest(
            "at least one station resource setting must be provided".to_owned(),
        ));
    }
    let config = request.config.map(sqlx::types::Json);
    let result = match kind.as_str() {
        "inputs" => {
            sqlx::query(
                "UPDATE inputs SET name = COALESCE($1, name), source_type = COALESCE($2, source_type), enabled = COALESCE($3, enabled), config = COALESCE($4, config), updated_at = NOW() WHERE id = $5 AND station_id = $6",
            )
            .bind(request.name.map(|value| value.trim().to_owned()))
            .bind(request.resource_type.map(|value| value.trim().to_owned()))
            .bind(request.enabled)
            .bind(config)
            .bind(id)
            .bind(state.station_id)
            .execute(&state.pool)
            .await?
        }
        "outputs" => {
            sqlx::query(
                "UPDATE outputs SET name = COALESCE($1, name), output_type = COALESCE($2, output_type), enabled = COALESCE($3, enabled), config = COALESCE($4, config), updated_at = NOW() WHERE id = $5 AND station_id = $6",
            )
            .bind(request.name.map(|value| value.trim().to_owned()))
            .bind(request.resource_type.map(|value| value.trim().to_owned()))
            .bind(request.enabled)
            .bind(config)
            .bind(id)
            .bind(state.station_id)
            .execute(&state.pool)
            .await?
        }
        "metadata" => {
            sqlx::query(
                "UPDATE metadata_configs SET name = COALESCE($1, name), direction = COALESCE($2, direction), format = COALESCE($3, format), enabled = COALESCE($4, enabled), config = COALESCE($5, config), updated_at = NOW() WHERE id = $6 AND station_id = $7",
            )
            .bind(request.name.map(|value| value.trim().to_owned()))
            .bind(request.direction)
            .bind(request.format.map(|value| value.trim().to_owned()))
            .bind(request.enabled)
            .bind(config)
            .bind(id)
            .bind(state.station_id)
            .execute(&state.pool)
            .await?
        }
        "routes" => {
            sqlx::query(
                "UPDATE routing_rules SET name = COALESCE($1, name), enabled = COALESCE($2, enabled), config = COALESCE($3, config), updated_at = NOW() WHERE id = $4 AND station_id = $5",
            )
            .bind(request.name.map(|value| value.trim().to_owned()))
            .bind(request.enabled)
            .bind(config)
            .bind(id)
            .bind(state.station_id)
            .execute(&state.pool)
            .await?
        }
        _ => return Err(ApiError::NotFound),
    };
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    sync_engine_resource(&state, &kind, id, false)
        .await
        .map_err(ApiError::SavedButNotApplied)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sync_engine_resource(
    state: &AppState,
    kind: &str,
    id: Uuid,
    removed: bool,
) -> Result<(), String> {
    if kind == "metadata" {
        return crate::engine_sync::apply_metadata(state).await;
    }
    let result = match (kind, removed) {
        ("inputs", false) => crate::engine_sync::apply_input(state, id).await,
        ("inputs", true) => crate::engine_sync::remove_input(state, id).await,
        ("outputs", false) => crate::engine_sync::apply_output(state, id).await,
        ("outputs", true) => crate::engine_sync::remove_output(state, id).await,
        _ => return Ok(()),
    };
    if let Err(error) = result {
        tracing::warn!(kind, %id, %error, "audio engine did not accept the configuration yet");
        return Err(error);
    }
    Ok(())
}

pub async fn delete_station_resource(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    authorize_station(&headers, &state)?;
    let table = station_resource_table(&kind)?;
    let query = match table {
        "inputs" => "DELETE FROM inputs WHERE id = $1 AND station_id = $2",
        "outputs" => "DELETE FROM outputs WHERE id = $1 AND station_id = $2",
        "metadata_configs" => "DELETE FROM metadata_configs WHERE id = $1 AND station_id = $2",
        "routing_rules" => "DELETE FROM routing_rules WHERE id = $1 AND station_id = $2",
        _ => return Err(ApiError::NotFound),
    };
    let result = sqlx::query(query)
        .bind(id)
        .bind(state.station_id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    let sync_result = sync_engine_resource(&state, &kind, id, true).await;
    if kind == "inputs" {
        sqlx::query("DELETE FROM mixer_channel_state WHERE station_id = $1 AND channel_id = $2")
            .bind(state.station_id)
            .bind(id)
            .execute(&state.pool)
            .await?;
    }
    sync_result.map_err(ApiError::DeletedButNotApplied)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn station_health(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_station(&headers, &state)?;
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE station_id = $1")
        .bind(state.station_id)
        .fetch_one(&state.pool)
        .await?;
    let sessions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions WHERE station_id = $1 AND status = 'connected'",
    )
    .bind(state.station_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(json!({
        "status": "ok",
        "station_id": state.station_id,
        "users": users,
        "connected_sessions": sessions
    })))
}

async fn rows_as_json(
    pool: &sqlx::PgPool,
    table: &str,
    station_id: Uuid,
) -> Result<Vec<Value>, ApiError> {
    let query = match table {
        "inputs" => "SELECT jsonb_build_object('id', id, 'name', name, 'type', source_type, 'enabled', enabled, 'config', config) FROM inputs WHERE station_id = $1 ORDER BY created_at",
        "outputs" => "SELECT jsonb_build_object('id', id, 'name', name, 'type', output_type, 'enabled', enabled, 'config', config) FROM outputs WHERE station_id = $1 ORDER BY created_at",
        "metadata_configs" => "SELECT jsonb_build_object('id', id, 'name', name, 'direction', direction, 'format', format, 'enabled', enabled, 'config', config) FROM metadata_configs WHERE station_id = $1 ORDER BY created_at",
        "delay_configs" => "SELECT jsonb_build_object('id', id, 'delay_seconds', delay_seconds, 'enabled', enabled, 'config', config) FROM delay_configs WHERE station_id = $1 ORDER BY created_at",
        "routing_rules" => "SELECT jsonb_build_object('id', id, 'name', name, 'enabled', enabled, 'config', config) FROM routing_rules WHERE station_id = $1 ORDER BY created_at",
        _ => return Err(ApiError::InvalidRequest("unsupported configuration".to_owned())),
    };
    Ok(sqlx::query_scalar::<_, Value>(query)
        .bind(station_id)
        .fetch_all(pool)
        .await?)
}

#[derive(Deserialize)]
pub struct CreateGuestLink {
    name: Option<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct GuestLinkResponse {
    id: Uuid,
    name: Option<String>,
    expires_at: DateTime<Utc>,
    invalidated_at: Option<DateTime<Utc>>,
    invitation_token: Option<String>,
    status: String,
}

pub async fn list_guest_links(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<GuestLinkResponse>>, ApiError> {
    authorize_station(&headers, &state)?;
    let rows = sqlx::query(
        "SELECT id, name, expires_at, invalidated_at, CASE WHEN invalidated_at IS NOT NULL THEN 'invalidated' WHEN expires_at <= NOW() THEN 'expired' ELSE 'active' END AS status FROM guest_links WHERE station_id = $1 ORDER BY created_at DESC",
    )
    .bind(state.station_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| GuestLinkResponse {
                id: row.get("id"),
                name: row.get("name"),
                expires_at: row.get("expires_at"),
                invalidated_at: row.get("invalidated_at"),
                invitation_token: None,
                status: row.get("status"),
            })
            .collect(),
    ))
}

pub async fn create_guest_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateGuestLink>,
) -> Result<(StatusCode, Json<GuestLinkResponse>), ApiError> {
    authorize_station(&headers, &state)?;
    if request.expires_at <= Utc::now() {
        return Err(ApiError::InvalidRequest(
            "expires_at must be in the future".to_owned(),
        ));
    }
    let token = random_token();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO guest_links (id, station_id, name, token_hash, expires_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(state.station_id)
    .bind(request.name.as_deref())
    .bind(hash_token(&token))
    .bind(request.expires_at)
    .execute(&state.pool)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(GuestLinkResponse {
            id,
            name: request.name,
            expires_at: request.expires_at,
            invalidated_at: None,
            invitation_token: Some(token),
            status: "active".to_owned(),
        }),
    ))
}

pub async fn invalidate_guest_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    authorize_station(&headers, &state)?;
    let mut transaction = state.pool.begin().await?;
    let result = sqlx::query(
        "UPDATE guest_links SET invalidated_at = COALESCE(invalidated_at, NOW()) WHERE id = $1 AND station_id = $2",
    )
    .bind(id)
    .bind(state.station_id)
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    sqlx::query(
        "UPDATE sessions SET status = 'expired', approval_expires_at = NULL, updated_at = NOW() WHERE station_id = $1 AND guest_link_id = $2 AND status IN ('pending', 'approved', 'connected', 'disconnected', 'timed_out')",
    )
    .bind(state.station_id)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct GuestSessionRequest {
    name: String,
}

#[derive(Serialize)]
pub struct GuestSessionResponse {
    session_id: Uuid,
    display_name: String,
    role: String,
    status: String,
    access_token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
struct ParticipantClaims {
    sub: String,
    station_id: String,
    role: String,
    exp: usize,
}

pub async fn create_guest_session(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(request): Json<GuestSessionRequest>,
) -> Result<(StatusCode, Json<GuestSessionResponse>), ApiError> {
    let name = request.name.trim();
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::InvalidRequest(
            "name must contain between 1 and 100 characters".to_owned(),
        ));
    }
    expire_stale_sessions(&state).await?;
    let link = sqlx::query(
        "SELECT id, expires_at FROM guest_links WHERE station_id = $1 AND token_hash = $2 AND invalidated_at IS NULL AND expires_at > NOW()",
    )
    .bind(state.station_id)
    .bind(hash_token(&token))
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let guest_link_id: Uuid = link.try_get("id")?;
    let expires_at: DateTime<Utc> = link.try_get("expires_at")?;
    let active = sqlx::query(
        "SELECT id, display_name, status FROM sessions WHERE station_id = $1 AND guest_link_id = $2 AND status IN ('pending', 'approved', 'connected', 'disconnected') AND expires_at > NOW() ORDER BY created_at DESC LIMIT 1",
    )
    .bind(state.station_id)
    .bind(guest_link_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(active) = active {
        let display_name: String = active.try_get("display_name")?;
        if display_name != name {
            return Err(ApiError::Conflict(
                "this invitation already has an active session".to_owned(),
            ));
        }
        let session_id: Uuid = active.try_get("id")?;
        let status: String = active.try_get("status")?;
        return Ok((
            StatusCode::OK,
            Json(GuestSessionResponse {
                session_id,
                display_name,
                role: "guest".to_owned(),
                status,
                access_token: participant_token(&state, session_id, "guest", expires_at)?,
                expires_at,
            }),
        ));
    }

    let session_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO sessions (id, station_id, guest_link_id, role, display_name, status, expires_at, approval_expires_at) VALUES ($1, $2, $3, 'guest', $4, 'pending', $5, LEAST($5, NOW() + INTERVAL '20 minutes')) ON CONFLICT DO NOTHING",
    )
    .bind(session_id)
    .bind(state.station_id)
    .bind(guest_link_id)
    .bind(name)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    if inserted.rows_affected() == 0 {
        let active = sqlx::query(
            "SELECT id, display_name, status FROM sessions WHERE station_id = $1 AND guest_link_id = $2 AND status IN ('pending', 'approved', 'connected', 'disconnected') AND expires_at > NOW() ORDER BY created_at DESC LIMIT 1",
        )
        .bind(state.station_id)
        .bind(guest_link_id)
        .fetch_optional(&state.pool)
        .await?;
        if let Some(active) = active {
            let display_name: String = active.try_get("display_name")?;
            if display_name == name {
                let session_id: Uuid = active.try_get("id")?;
                let status: String = active.try_get("status")?;
                return Ok((
                    StatusCode::OK,
                    Json(GuestSessionResponse {
                        session_id,
                        display_name,
                        role: "guest".to_owned(),
                        status,
                        access_token: participant_token(&state, session_id, "guest", expires_at)?,
                        expires_at,
                    }),
                ));
            }
        }
        return Err(ApiError::Conflict(
            "this guest link already has an active session".to_owned(),
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(GuestSessionResponse {
            session_id,
            display_name: name.to_owned(),
            role: "guest".to_owned(),
            status: "pending".to_owned(),
            access_token: participant_token(&state, session_id, "guest", expires_at)?,
            expires_at,
        }),
    ))
}

#[derive(Deserialize)]
pub struct CreateLiveSession {
    display_name: String,
    role: String,
}

#[derive(Deserialize)]
pub struct RenameLiveSession {
    display_name: String,
}

pub async fn rename_live_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<RenameLiveSession>,
) -> Result<StatusCode, ApiError> {
    authorize_station(&headers, &state)?;
    let display_name = request.display_name.trim();
    if display_name.is_empty() || display_name.len() > 100 {
        return Err(ApiError::InvalidRequest(
            "display_name must contain between 1 and 100 characters".to_owned(),
        ));
    }
    let updated = sqlx::query(
        "UPDATE sessions SET display_name = $1, updated_at = NOW() WHERE id = $2 AND station_id = $3 AND guest_link_id IS NULL",
    )
    .bind(display_name)
    .bind(id)
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub struct LiveSessionResponse {
    id: Uuid,
    display_name: String,
    role: String,
    status: String,
    expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct LiveSessionTokenResponse {
    session_id: Uuid,
    display_name: String,
    role: String,
    status: String,
    access_token: String,
    expires_at: DateTime<Utc>,
}

pub async fn create_live_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateLiveSession>,
) -> Result<(StatusCode, Json<LiveSessionTokenResponse>), ApiError> {
    authorize_station(&headers, &state)?;
    let display_name = request.display_name.trim();
    if display_name.is_empty() || display_name.len() > 100 {
        return Err(ApiError::InvalidRequest(
            "display_name must contain between 1 and 100 characters".to_owned(),
        ));
    }
    if !matches!(request.role.as_str(), "host" | "co_host") {
        return Err(ApiError::InvalidRequest(
            "role must be host or co_host".to_owned(),
        ));
    }

    let expires_at = Utc::now() + chrono::Duration::hours(8);
    let mut transaction = state.pool.begin().await?;
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (station_id, name, role) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(state.station_id)
    .bind(display_name)
    .bind(&request.role)
    .fetch_one(&mut *transaction)
    .await?;
    let session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO sessions (id, station_id, user_id, role, display_name, status, expires_at) VALUES ($1, $2, $3, $4, $5, 'approved', $6)",
    )
    .bind(session_id)
    .bind(state.station_id)
    .bind(user_id)
    .bind(&request.role)
    .bind(display_name)
    .bind(expires_at)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(LiveSessionTokenResponse {
            session_id,
            display_name: display_name.to_owned(),
            role: request.role.clone(),
            status: "approved".to_owned(),
            access_token: participant_token(&state, session_id, &request.role, expires_at)?,
            expires_at,
        }),
    ))
}

pub async fn list_live_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<LiveSessionResponse>>, ApiError> {
    authorize_station(&headers, &state)?;
    expire_stale_sessions(&state).await?;
    let rows = sqlx::query(
        "SELECT id, display_name, role, status, expires_at FROM sessions WHERE station_id = $1 AND created_at > NOW() - INTERVAL '24 hours' ORDER BY created_at DESC LIMIT 100",
    )
    .bind(state.station_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| LiveSessionResponse {
                id: row.get("id"),
                display_name: row.get("display_name"),
                role: row.get("role"),
                status: row.get("status"),
                expires_at: row.get("expires_at"),
            })
            .collect(),
    ))
}

pub async fn approve_guest_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<LiveSessionResponse>, ApiError> {
    authorize_station(&headers, &state)?;
    expire_stale_sessions(&state).await?;
    let row = sqlx::query(
        "UPDATE sessions s SET status = 'approved', expires_at = gl.expires_at, approval_expires_at = NULL, updated_at = NOW() FROM guest_links gl WHERE s.id = $1 AND s.station_id = $2 AND s.guest_link_id = gl.id AND s.status = 'pending' AND s.approval_expires_at > NOW() AND gl.invalidated_at IS NULL AND gl.expires_at > NOW() RETURNING s.id, s.display_name, s.role, s.status, s.expires_at",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(Json(LiveSessionResponse {
        id: row.get("id"),
        display_name: row.get("display_name"),
        role: row.get("role"),
        status: row.get("status"),
        expires_at: row.get("expires_at"),
    }))
}

pub async fn reject_guest_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<LiveSessionResponse>, ApiError> {
    authorize_station(&headers, &state)?;
    expire_stale_sessions(&state).await?;
    let row = sqlx::query(
        "UPDATE sessions SET status = 'rejected', approval_expires_at = NULL, updated_at = NOW() WHERE id = $1 AND station_id = $2 AND role = 'guest' AND status = 'pending' RETURNING id, display_name, role, status, expires_at",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(Json(LiveSessionResponse {
        id: row.get("id"),
        display_name: row.get("display_name"),
        role: row.get("role"),
        status: row.get("status"),
        expires_at: row.get("expires_at"),
    }))
}

pub async fn disconnect_guest_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<LiveSessionResponse>, ApiError> {
    authorize_station(&headers, &state)?;
    expire_stale_sessions(&state).await?;
    let mut transaction = state.pool.begin().await?;
    let row = sqlx::query(
        "UPDATE sessions SET status = 'rejected', approval_expires_at = NULL, updated_at = NOW() WHERE id = $1 AND station_id = $2 AND status IN ('approved', 'connected', 'disconnected') RETURNING id, display_name, role, status, expires_at",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(ApiError::NotFound)?;
    let channel = sqlx::query(
        "INSERT INTO mixer_channel_state (station_id, channel_id, muted) VALUES ($1, $2, TRUE) ON CONFLICT (station_id, channel_id) DO UPDATE SET muted = TRUE, revision = mixer_channel_state.revision + 1, updated_at = NOW() RETURNING channel_id, fader, is_on, cue_enabled, muted, revision",
    )
    .bind(state.station_id)
    .bind(id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let _ = state.mixer_updates.send(mixer_channel_value(
        channel.get("channel_id"),
        channel.get("fader"),
        channel.get("is_on"),
        channel.get("cue_enabled"),
        channel.get("muted"),
        channel.get("revision"),
    ));
    Ok(Json(LiveSessionResponse {
        id: row.get("id"),
        display_name: row.get("display_name"),
        role: row.get("role"),
        status: row.get("status"),
        expires_at: row.get("expires_at"),
    }))
}

pub async fn get_guest_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let claims = authorize_participant(&headers, &state)?;
    if claims.role != "guest" || Uuid::parse_str(&claims.sub).ok() != Some(id) {
        return Err(ApiError::Unauthorized);
    }
    expire_stale_sessions(&state).await?;
    let row = sqlx::query(
        "SELECT s.id, s.display_name, s.role, s.status, s.expires_at FROM sessions s JOIN guest_links gl ON gl.id = s.guest_link_id AND gl.station_id = s.station_id WHERE s.id = $1 AND s.station_id = $2",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    Ok(Json(json!({
        "session_id": row.get::<Uuid, _>("id"),
        "display_name": row.get::<String, _>("display_name"),
        "role": row.get::<String, _>("role"),
        "status": row.get::<String, _>("status"),
        "expires_at": row.get::<DateTime<Utc>, _>("expires_at")
    })))
}

#[derive(Deserialize)]
pub struct ConnectionStateRequest {
    state: String,
}

pub async fn get_live_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let claims = authorize_participant(&headers, &state)?;
    let claim_session_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
    if claim_session_id != id {
        return Err(ApiError::Unauthorized);
    }
    expire_stale_sessions(&state).await?;
    let row = sqlx::query(
        "SELECT s.display_name, s.role, s.status FROM sessions s LEFT JOIN guest_links gl ON gl.id = s.guest_link_id AND gl.station_id = s.station_id WHERE s.id = $1 AND s.station_id = $2 AND s.role = $3 AND s.expires_at > NOW() AND (s.guest_link_id IS NULL OR (gl.invalidated_at IS NULL AND gl.expires_at > NOW()))",
    )
    .bind(id)
    .bind(state.station_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    let status: String = row.try_get("status")?;
    if !matches!(status.as_str(), "approved" | "connected" | "disconnected") {
        return Err(ApiError::Conflict(format!(
            "session is not approved for live audio ({status})"
        )));
    }

    let _room_guard = state.janus_room_lock.lock().await;
    crate::janus::ensure_audio_room(
        &state.http_client,
        &state.janus_http_url,
        state.janus_room_id,
        &state.janus_room_pin,
    )
    .await
    .map_err(ApiError::Janus)?;
    crate::janus::ensure_audio_room(
        &state.http_client,
        &state.janus_http_url,
        state.janus_cue_room_id,
        &state.janus_room_pin,
    )
    .await
    .map_err(ApiError::Janus)?;

    Ok(Json(json!({
        "session_id": id,
        "display_name": row.try_get::<String, _>("display_name")?,
        "role": claims.role,
        "websocket_url": "",
        "room_id": state.janus_room_id,
        "cue_room_id": state.janus_cue_room_id,
        "room_pin": state.janus_room_pin,
        "ice_servers": state.janus_ice_servers
    })))
}

pub async fn list_mixer_channels(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    authorize_station(&headers, &state)?;
    Ok(Json(load_mixer_channels(&state).await?))
}

#[derive(Deserialize)]
pub struct MixerChannelRequest {
    fader: Option<f64>,
    is_on: Option<bool>,
    cue_enabled: Option<bool>,
    muted: Option<bool>,
}

pub async fn update_mixer_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<MixerChannelRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize_station(&headers, &state)?;
    if request.fader.is_none()
        && request.is_on.is_none()
        && request.cue_enabled.is_none()
        && request.muted.is_none()
    {
        return Err(ApiError::InvalidRequest(
            "at least one mixer setting must be provided".to_owned(),
        ));
    }
    if request
        .fader
        .is_some_and(|fader| !fader.is_finite() || !(0.0..=2.0).contains(&fader))
    {
        return Err(ApiError::InvalidRequest(
            "fader must be between 0 and 2".to_owned(),
        ));
    }
    let is_input: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM inputs WHERE id = $1 AND station_id = $2 AND enabled)",
    )
    .bind(id)
    .bind(state.station_id)
    .fetch_one(&state.pool)
    .await?;
    let active: bool = is_input
        || sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sessions WHERE id = $1 AND station_id = $2 AND status IN ('approved', 'connected', 'disconnected') AND expires_at > NOW())",
        )
        .bind(id)
        .bind(state.station_id)
        .fetch_one(&state.pool)
        .await?;
    if !active {
        return Err(ApiError::NotFound);
    }

    let row = sqlx::query(
        "INSERT INTO mixer_channel_state (station_id, channel_id, fader, is_on, cue_enabled, muted) VALUES ($1, $2, COALESCE($3, 1.0), COALESCE($4, TRUE), COALESCE($5, FALSE), COALESCE($6, FALSE)) ON CONFLICT (station_id, channel_id) DO UPDATE SET fader = COALESCE($3, mixer_channel_state.fader), is_on = COALESCE($4, mixer_channel_state.is_on), cue_enabled = COALESCE($5, mixer_channel_state.cue_enabled), muted = COALESCE($6, mixer_channel_state.muted), revision = mixer_channel_state.revision + 1, updated_at = NOW() RETURNING channel_id, fader, is_on, cue_enabled, muted, revision",
    )
    .bind(state.station_id)
    .bind(id)
    .bind(request.fader)
    .bind(request.is_on)
    .bind(request.cue_enabled)
    .bind(request.muted)
    .fetch_one(&state.pool)
    .await?;
    let channel = mixer_channel_value(
        row.get("channel_id"),
        row.get("fader"),
        row.get("is_on"),
        row.get("cue_enabled"),
        row.get("muted"),
        row.get("revision"),
    );
    if is_input {
        if let Err(error) = crate::engine_sync::apply_source_mix(&state, id).await {
            tracing::warn!(%id, %error, "audio engine did not apply the input fader");
        }
    }
    let _ = state.mixer_updates.send(channel.clone());
    Ok(Json(channel))
}

pub async fn mixer_websocket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MixerSocketQuery>,
    websocket: WebSocketUpgrade,
) -> Result<impl IntoResponse, ApiError> {
    let participant_id = if let Some(token) = query.token {
        let claims = decode_participant_token(&token, &state)?;
        let id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sessions s LEFT JOIN guest_links gl ON gl.id = s.guest_link_id AND gl.station_id = s.station_id WHERE s.id = $1 AND s.station_id = $2 AND s.role = $3 AND s.status IN ('approved', 'connected', 'disconnected') AND s.expires_at > NOW() AND (s.guest_link_id IS NULL OR (gl.invalidated_at IS NULL AND gl.expires_at > NOW())))",
        )
        .bind(id)
        .bind(state.station_id)
        .bind(&claims.role)
        .fetch_one(&state.pool)
        .await?;
        if !active {
            return Err(ApiError::Unauthorized);
        }
        Some(id)
    } else {
        if let Some(api_key) = query.api_key.as_deref() {
            if api_key
                .as_bytes()
                .ct_eq(state.station_api_key.as_bytes())
                .unwrap_u8()
                != 1
            {
                return Err(ApiError::Unauthorized);
            }
        } else {
            authorize_station(&headers, &state)?;
        }
        None
    };
    Ok(websocket.on_upgrade(move |socket| handle_mixer_websocket(socket, state, participant_id)))
}

#[derive(Deserialize)]
pub struct MixerSocketQuery {
    token: Option<String>,
    api_key: Option<String>,
}

async fn handle_mixer_websocket(
    mut socket: WebSocket,
    state: AppState,
    participant_id: Option<Uuid>,
) {
    let mut updates = state.mixer_updates.subscribe();
    match load_mixer_channels(&state).await {
        Ok(channels) => {
            let channels: Vec<_> = channels
                .into_iter()
                .filter(|channel| {
                    participant_id.is_none()
                        || channel["channel_id"].as_str()
                            == participant_id.map(|id| id.to_string()).as_deref()
                })
                .collect();
            let snapshot = json!({ "type": "snapshot", "channels": channels });
            if socket
                .send(Message::Text(snapshot.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
        Err(error) => {
            tracing::error!(%error, "failed to load mixer snapshot");
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    }
    loop {
        match updates.recv().await {
            Ok(channel) => {
                if participant_id.is_some()
                    && channel["channel_id"].as_str()
                        != participant_id.map(|id| id.to_string()).as_deref()
                {
                    continue;
                }
                let message = json!({ "type": "update", "channel": channel });
                if socket
                    .send(Message::Text(message.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                tracing::warn!(count, "mixer websocket client missed updates");
                match load_mixer_channels(&state).await {
                    Ok(channels) => {
                        let channels: Vec<_> = channels
                            .into_iter()
                            .filter(|channel| {
                                participant_id.is_none()
                                    || channel["channel_id"].as_str()
                                        == participant_id.map(|id| id.to_string()).as_deref()
                            })
                            .collect();
                        let snapshot = json!({ "type": "snapshot", "channels": channels });
                        if socket
                            .send(Message::Text(snapshot.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "failed to refresh mixer snapshot");
                        break;
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn load_mixer_channels(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        "SELECT id, display_name, role, fader, is_on, cue_enabled, muted, revision FROM (SELECT i.id, i.name AS display_name, i.source_type AS role, COALESCE(mc.fader, 1.0) AS fader, COALESCE(mc.is_on, TRUE) AS is_on, COALESCE(mc.cue_enabled, FALSE) AS cue_enabled, COALESCE(mc.muted, FALSE) AS muted, COALESCE(mc.revision, 0) AS revision, CASE i.source_type WHEN 'automation' THEN 0 ELSE 1 END AS grp, i.created_at FROM inputs i LEFT JOIN mixer_channel_state mc ON mc.station_id = i.station_id AND mc.channel_id = i.id WHERE i.station_id = $1 AND i.enabled UNION ALL SELECT s.id, s.display_name, s.role, COALESCE(mc.fader, 1.0), COALESCE(mc.is_on, TRUE), COALESCE(mc.cue_enabled, FALSE), COALESCE(mc.muted, FALSE), COALESCE(mc.revision, 0), 2, s.created_at FROM sessions s LEFT JOIN mixer_channel_state mc ON mc.station_id = s.station_id AND mc.channel_id = s.id WHERE s.station_id = $1 AND s.status IN ('approved', 'connected', 'disconnected') AND s.expires_at > NOW()) channels ORDER BY grp, created_at",
    )
    .bind(state.station_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "channel_id": row.get::<Uuid, _>("id"),
                "display_name": row.get::<String, _>("display_name"),
                "role": row.get::<String, _>("role"),
                "fader": row.get::<f64, _>("fader"),
                "is_on": row.get::<bool, _>("is_on"),
                "cue_enabled": row.get::<bool, _>("cue_enabled"),
                "muted": row.get::<bool, _>("muted"),
                "revision": row.get::<i64, _>("revision")
            })
        })
        .collect())
}

fn mixer_channel_value(
    channel_id: Uuid,
    fader: f64,
    is_on: bool,
    cue_enabled: bool,
    muted: bool,
    revision: i64,
) -> Value {
    json!({
        "channel_id": channel_id,
        "fader": fader,
        "is_on": is_on,
        "cue_enabled": cue_enabled,
        "muted": muted,
        "revision": revision
    })
}

pub async fn janus_websocket_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let original_uri = headers
        .get("x-original-uri")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = original_uri
        .split_once('?')
        .map(|(_, query)| query)
        .and_then(|query| {
            query
                .split('&')
                .find_map(|parameter| parameter.strip_prefix("token="))
        })
        .filter(|token| !token.is_empty())
        .ok_or(ApiError::Unauthorized)?;
    let claims = decode_participant_token(token, &state)?;
    let session_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions s LEFT JOIN guest_links gl ON gl.id = s.guest_link_id AND gl.station_id = s.station_id WHERE s.id = $1 AND s.station_id = $2 AND s.role = $3 AND s.status IN ('approved', 'connected', 'disconnected') AND s.expires_at > NOW() AND (s.guest_link_id IS NULL OR (gl.invalidated_at IS NULL AND gl.expires_at > NOW())))",
    )
    .bind(session_id)
    .bind(state.station_id)
    .bind(&claims.role)
    .fetch_one(&state.pool)
    .await?;
    if !active {
        return Err(ApiError::Unauthorized);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_live_connection(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<ConnectionStateRequest>,
) -> Result<Json<LiveSessionResponse>, ApiError> {
    let claims = authorize_participant(&headers, &state)?;
    if Uuid::parse_str(&claims.sub).ok() != Some(id) {
        return Err(ApiError::Unauthorized);
    }
    expire_stale_sessions(&state).await?;
    let status = match request.state.as_str() {
        "connected" => "connected",
        "disconnected" => "disconnected",
        _ => {
            return Err(ApiError::InvalidRequest(
                "state must be connected or disconnected".to_owned(),
            ));
        }
    };
    let row = sqlx::query(
        "UPDATE sessions s SET status = $1, updated_at = NOW() WHERE s.id = $2 AND s.station_id = $3 AND s.role = $4 AND s.status IN ('approved', 'connected', 'disconnected') AND s.expires_at > NOW() AND (s.guest_link_id IS NULL OR EXISTS (SELECT 1 FROM guest_links gl WHERE gl.id = s.guest_link_id AND gl.station_id = s.station_id AND gl.invalidated_at IS NULL AND gl.expires_at > NOW())) RETURNING s.id, s.display_name, s.role, s.status, s.expires_at",
    )
    .bind(status)
    .bind(id)
    .bind(state.station_id)
    .bind(&claims.role)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(ApiError::Unauthorized)?;
    Ok(Json(LiveSessionResponse {
        id: row.get("id"),
        display_name: row.get("display_name"),
        role: row.get("role"),
        status: row.get("status"),
        expires_at: row.get("expires_at"),
    }))
}

async fn expire_stale_sessions(state: &AppState) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE sessions SET status = 'timed_out', approval_expires_at = NULL, updated_at = NOW() WHERE station_id = $1 AND status = 'pending' AND approval_expires_at <= NOW() AND expires_at > NOW()",
    )
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE sessions s SET status = 'expired', approval_expires_at = NULL, updated_at = NOW() WHERE s.station_id = $1 AND s.status IN ('pending', 'approved', 'connected', 'disconnected', 'timed_out') AND (s.expires_at <= NOW() OR EXISTS (SELECT 1 FROM guest_links gl WHERE gl.id = s.guest_link_id AND gl.station_id = s.station_id AND (gl.invalidated_at IS NOT NULL OR gl.expires_at <= NOW())))",
    )
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    // Host consoles that closed without reporting a disconnect would otherwise linger as ghosts.
    sqlx::query(
        "UPDATE sessions SET status = 'expired', updated_at = NOW() WHERE station_id = $1 AND guest_link_id IS NULL AND role IN ('host', 'co_host') AND status IN ('approved', 'disconnected') AND updated_at < NOW() - INTERVAL '2 minutes'",
    )
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

fn participant_token(
    state: &AppState,
    session_id: Uuid,
    role: &str,
    expires_at: DateTime<Utc>,
) -> Result<String, ApiError> {
    let claims = ParticipantClaims {
        sub: session_id.to_string(),
        station_id: state.station_id.to_string(),
        role: role.to_owned(),
        exp: expires_at.timestamp() as usize,
    };
    Ok(encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(state.guest_jwt_secret.as_bytes()),
    )?)
}

#[derive(Serialize)]
pub struct UserResponse {
    id: Uuid,
    name: String,
    role: String,
    created_at: DateTime<Utc>,
}

pub async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserResponse>>, ApiError> {
    authorize_station(&headers, &state)?;
    let rows = sqlx::query(
        "SELECT id, name, role, created_at FROM users WHERE station_id = $1 ORDER BY created_at",
    )
    .bind(state.station_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| UserResponse {
                id: row.get("id"),
                name: row.get("name"),
                role: row.get("role"),
                created_at: row.get("created_at"),
            })
            .collect(),
    ))
}

fn authorize_station(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    let supplied = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    if api_key_matches(supplied, &state.station_api_key) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

fn api_key_matches(supplied: &str, expected: &str) -> bool {
    supplied.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() == 1
}

fn authorize_participant(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<ParticipantClaims, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    let claims = decode_participant_token(token, state)?;
    Ok(claims)
}

fn decode_participant_token(token: &str, state: &AppState) -> Result<ParticipantClaims, ApiError> {
    let claims = decode::<ParticipantClaims>(
        token,
        &DecodingKey::from_secret(state.guest_jwt_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|_| ApiError::Unauthorized)?
    .claims;
    if !matches!(claims.role.as_str(), "guest" | "host" | "co_host")
        || claims.station_id != state.station_id.to_string()
    {
        return Err(ApiError::Unauthorized);
    }
    Ok(claims)
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_api_key_requires_an_exact_match() {
        assert!(api_key_matches("station-key", "station-key"));
        assert!(!api_key_matches("wrong-key", "station-key"));
        assert!(!api_key_matches("station-key-extra", "station-key"));
    }

    #[test]
    fn guest_invitation_tokens_are_random_hex_values() {
        let first = random_token();
        let second = random_token();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn invitation_tokens_are_stored_as_fixed_length_hashes() {
        let token = random_token();
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 32);
        assert_ne!(hash, token.as_bytes());
        assert_eq!(hash, hash_token(&token));
    }
}

#[derive(Deserialize)]
pub struct MonitorSettingsRequest {
    local_source: Option<String>,
    local_input_id: Option<Uuid>,
    local_volume: Option<f64>,
    local_muted: Option<bool>,
    guest_source: Option<String>,
    guest_input_id: Option<Uuid>,
}

fn valid_monitor_source(source: &str) -> bool {
    matches!(source, "cue" | "program_mix" | "mix_minus" | "input")
}

async fn load_monitor_settings(state: &AppState) -> Result<Value, ApiError> {
    sqlx::query(
        "INSERT INTO station_monitor_settings (station_id) VALUES ($1) ON CONFLICT (station_id) DO NOTHING",
    )
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    let row = sqlx::query(
        "SELECT local_source, local_input_id, local_volume, local_muted, guest_source, guest_input_id FROM station_monitor_settings WHERE station_id = $1",
    )
    .bind(state.station_id)
    .fetch_one(&state.pool)
    .await?;
    Ok(json!({
        "local_source": row.get::<String, _>("local_source"),
        "local_input_id": row.get::<Option<Uuid>, _>("local_input_id"),
        "local_volume": row.get::<f64, _>("local_volume"),
        "local_muted": row.get::<bool, _>("local_muted"),
        "guest_source": row.get::<String, _>("guest_source"),
        "guest_input_id": row.get::<Option<Uuid>, _>("guest_input_id")
    }))
}

pub async fn get_monitor_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    authorize_station(&headers, &state)?;
    Ok(Json(load_monitor_settings(&state).await?))
}

pub async fn update_monitor_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<MonitorSettingsRequest>,
) -> Result<Json<Value>, ApiError> {
    authorize_station(&headers, &state)?;
    for source in [
        request.local_source.as_deref(),
        request.guest_source.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !valid_monitor_source(source) {
            return Err(ApiError::InvalidRequest(
                "monitor source must be cue, program_mix, mix_minus, or input".to_owned(),
            ));
        }
    }
    if request
        .local_volume
        .is_some_and(|volume| !volume.is_finite() || !(0.0..=1.0).contains(&volume))
    {
        return Err(ApiError::InvalidRequest(
            "local_volume must be between 0 and 1".to_owned(),
        ));
    }
    load_monitor_settings(&state).await?;
    sqlx::query(
        "UPDATE station_monitor_settings SET local_source = COALESCE($1, local_source), local_input_id = CASE WHEN COALESCE($1, local_source) = 'input' THEN COALESCE($2, local_input_id) ELSE NULL END, local_volume = COALESCE($3, local_volume), local_muted = COALESCE($4, local_muted), guest_source = COALESCE($5, guest_source), guest_input_id = CASE WHEN COALESCE($5, guest_source) = 'input' THEN COALESCE($6, guest_input_id) ELSE NULL END, updated_at = NOW() WHERE station_id = $7",
    )
    .bind(request.local_source)
    .bind(request.local_input_id)
    .bind(request.local_volume)
    .bind(request.local_muted)
    .bind(request.guest_source)
    .bind(request.guest_input_id)
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    Ok(Json(load_monitor_settings(&state).await?))
}
