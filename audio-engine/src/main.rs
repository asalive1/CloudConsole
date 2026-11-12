use std::{env, net::SocketAddr};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use gstreamer as gst;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod metadata;
mod network;
mod pipeline;
mod system;

use pipeline::{
    AudioEngine, AudioMetrics, BridgeBus, BridgeConfig, BridgeStatus, DelayStatus, EngineError,
};

#[derive(Clone)]
struct AppState {
    engine: std::sync::Arc<Mutex<AudioEngine>>,
    metadata: std::sync::Arc<Mutex<metadata::MetadataRelay>>,
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("not found")]
    NotFound,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("pipeline operation failed: {0}")]
    Pipeline(String),
}

impl From<EngineError> for ApiError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::InvalidRequest(message) => Self::InvalidRequest(message),
            EngineError::NotFound => Self::NotFound,
            EngineError::Conflict(message) => Self::Conflict(message),
            EngineError::Pipeline(message) => Self::Pipeline(message),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::InvalidRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_owned()),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message),
            ApiError::Pipeline(message) => {
                tracing::error!(error = %message, "audio pipeline operation failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "audio pipeline operation failed".to_owned(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    gstreamer_version: String,
}

#[derive(Deserialize)]
struct CreateSourceRequest {
    id: Option<Uuid>,
    name: String,
    source_type: pipeline::SourceType,
    transport: pipeline::InputTransport,
}

#[derive(Serialize)]
struct SourceResponse {
    id: Uuid,
    name: String,
    source_type: pipeline::SourceType,
    transport: &'static str,
    state: &'static str,
}

#[derive(Deserialize)]
struct CreateOutputRequest {
    id: Option<Uuid>,
    name: String,
    transport: pipeline::OutputTransport,
}

#[derive(Serialize)]
struct OutputResponse {
    id: Uuid,
    name: String,
    transport: &'static str,
    state: &'static str,
}

#[derive(Deserialize)]
struct ConfigureDelayRequest {
    delay_seconds: u32,
}

#[derive(Deserialize)]
struct SourceMixRequest {
    volume: f64,
    muted: bool,
    #[serde(default)]
    cue: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if env::args().nth(1).as_deref() == Some("--socket-health") {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut socket = tokio::net::UnixStream::connect("/run/cloudconsole/engine.sock").await?;
        socket.write_all(b"GET /health HTTP/1.0\r\n\r\n").await?;
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await?;
        if !response.starts_with(b"HTTP/1.1 200") && !response.starts_with(b"HTTP/1.0 200") {
            return Err("engine health check failed".into());
        }
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    gst::init()?;
    network::validate_configuration().map_err(|error| format!("network configuration: {error}"))?;

    let engine = AudioEngine::new()
        .map_err(|error| format!("audio engine initialization failed: {error}"))?;
    let engine = std::sync::Arc::new(Mutex::new(engine));
    let clock = std::sync::Arc::new(metadata::DelayClock::default());
    tokio::spawn(follow_delay(engine.clone(), clock.clone()));
    let state = AppState {
        engine,
        metadata: std::sync::Arc::new(Mutex::new(metadata::MetadataRelay::new(clock))),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(health))
        .route("/metrics", get(metrics))
        .route("/system/devices", get(system_devices))
        .route(
            "/bridge",
            get(bridge_status).put(start_bridge).delete(stop_bridge),
        )
        .route(
            "/bridge/cue",
            get(cue_bridge_status)
                .put(start_cue_bridge)
                .delete(stop_cue_bridge),
        )
        .route("/metadata", get(metadata_status).put(configure_metadata))
        .route("/delay", get(delay_status))
        .route("/delay/config", patch(configure_delay))
        .route("/delay/dump", post(dump_delay))
        .route("/delay/end", post(end_delay))
        .route("/delay/build", post(build_delay))
        .route("/sources/{id}/mix", patch(set_source_mix))
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/{id}", delete(remove_source))
        .route("/sources/{id}/recover", post(recover_source))
        .route("/outputs", get(list_outputs).post(create_output))
        .route("/outputs/{id}", delete(remove_output))
        .with_state(state);

    if network::host_mode() {
        use std::os::unix::fs::{FileTypeExt, PermissionsExt};
        let dir = "/run/cloudconsole";
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let path = "/run/cloudconsole/engine.sock";
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path)?,
            Ok(_) => return Err(format!("{path} exists and is not a socket").into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = tokio::net::UnixListener::bind(path)?;
        tracing::info!("Cloud Console audio engine listening on private Unix socket {path}");
        axum::serve(listener, app).await?;
        return Ok(());
    }
    let address = env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8001".to_owned());
    let listener = tokio::net::TcpListener::bind(address.parse::<SocketAddr>()?).await?;
    tracing::info!(
        "Cloud Console audio engine listening on {} (GStreamer {})",
        listener.local_addr()?,
        gst::version_string()
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn follow_delay(
    engine: std::sync::Arc<Mutex<AudioEngine>>,
    clock: std::sync::Arc<metadata::DelayClock>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    loop {
        tick.tick().await;
        if let Ok(status) = engine.lock().await.delay_status() {
            clock.update(status.buffered_delay_seconds);
        }
    }
}

async fn metadata_status(State(state): State<AppState>) -> Json<Vec<metadata::EndpointStatus>> {
    Json(state.metadata.lock().await.status())
}

async fn configure_metadata(
    State(state): State<AppState>,
    Json(config): Json<metadata::MetadataConfig>,
) -> Result<Json<Vec<metadata::EndpointStatus>>, ApiError> {
    let mut relay = state.metadata.lock().await;
    relay
        .apply(config.endpoints)
        .map_err(ApiError::InvalidRequest)?;
    Ok(Json(relay.status()))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        gstreamer_version: gst::version_string().to_string(),
    })
}

async fn system_devices() -> Result<Json<system::SystemDevices>, ApiError> {
    tokio::task::spawn_blocking(system::discover)
        .await
        .map(Json)
        .map_err(|error| ApiError::Pipeline(error.to_string()))
}

async fn bridge_status(State(state): State<AppState>) -> Json<BridgeStatus> {
    Json(state.engine.lock().await.bridge_status(BridgeBus::Program))
}

async fn cue_bridge_status(State(state): State<AppState>) -> Json<BridgeStatus> {
    Json(state.engine.lock().await.bridge_status(BridgeBus::Cue))
}

async fn start_cue_bridge(
    State(state): State<AppState>,
    Json(mut config): Json<BridgeConfig>,
) -> Result<Json<BridgeStatus>, ApiError> {
    config.bus = BridgeBus::Cue;
    Ok(Json(state.engine.lock().await.start_bridge(config)?))
}

async fn stop_cue_bridge(State(state): State<AppState>) -> StatusCode {
    state.engine.lock().await.stop_cue_bridge();
    StatusCode::NO_CONTENT
}

async fn start_bridge(
    State(state): State<AppState>,
    Json(config): Json<BridgeConfig>,
) -> Result<Json<BridgeStatus>, ApiError> {
    Ok(Json(state.engine.lock().await.start_bridge(config)?))
}

async fn stop_bridge(State(state): State<AppState>) -> StatusCode {
    state.engine.lock().await.stop_bridge();
    StatusCode::NO_CONTENT
}

async fn metrics(State(state): State<AppState>) -> Result<Json<AudioMetrics>, ApiError> {
    let engine = state.engine.lock().await;
    Ok(Json(engine.metrics()?))
}

async fn delay_status(State(state): State<AppState>) -> Result<Json<DelayStatus>, ApiError> {
    Ok(Json(state.engine.lock().await.delay_status()?))
}

async fn configure_delay(
    State(state): State<AppState>,
    Json(request): Json<ConfigureDelayRequest>,
) -> Result<Json<DelayStatus>, ApiError> {
    Ok(Json(
        state
            .engine
            .lock()
            .await
            .configure_delay(request.delay_seconds)?,
    ))
}

async fn dump_delay(State(state): State<AppState>) -> Result<Json<DelayStatus>, ApiError> {
    Ok(Json(state.engine.lock().await.dump_delay()?))
}

async fn end_delay(State(state): State<AppState>) -> Result<Json<DelayStatus>, ApiError> {
    Ok(Json(state.engine.lock().await.end_delay()?))
}

async fn build_delay(State(state): State<AppState>) -> Result<Json<DelayStatus>, ApiError> {
    Ok(Json(state.engine.lock().await.build_delay()?))
}

async fn set_source_mix(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<SourceMixRequest>,
) -> Result<StatusCode, ApiError> {
    if !request.volume.is_finite() {
        return Err(
            EngineError::InvalidRequest("volume must be a finite number".to_owned()).into(),
        );
    }
    state
        .engine
        .lock()
        .await
        .set_source_mix(id, request.volume, request.muted, request.cue)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_sources(
    State(state): State<AppState>,
) -> Result<Json<Vec<SourceResponse>>, ApiError> {
    let engine = state.engine.lock().await;
    Ok(Json(
        engine
            .sources()
            .into_iter()
            .map(|source| SourceResponse {
                id: source.id,
                name: source.name,
                source_type: source.source_type,
                transport: source.transport,
                state: source.state,
            })
            .collect(),
    ))
}

async fn create_source(
    State(state): State<AppState>,
    Json(request): Json<CreateSourceRequest>,
) -> Result<(StatusCode, Json<SourceResponse>), ApiError> {
    let mut engine = state.engine.lock().await;
    let source = engine.add_source_with_id(
        request.id.unwrap_or_else(Uuid::new_v4),
        request.name,
        request.source_type,
        request.transport,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(SourceResponse {
            id: source.id,
            name: source.name,
            source_type: source.source_type,
            transport: source.transport,
            state: source.state,
        }),
    ))
}

async fn remove_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.engine.lock().await.remove_source(id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn recover_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceResponse>, ApiError> {
    let source = state.engine.lock().await.recover_source(id)?;
    Ok(Json(SourceResponse {
        id: source.id,
        name: source.name,
        source_type: source.source_type,
        transport: source.transport,
        state: source.state,
    }))
}

async fn list_outputs(
    State(state): State<AppState>,
) -> Result<Json<Vec<OutputResponse>>, ApiError> {
    let engine = state.engine.lock().await;
    Ok(Json(
        engine
            .outputs()
            .into_iter()
            .map(|output| OutputResponse {
                id: output.id,
                name: output.name,
                transport: output.transport,
                state: output.state,
            })
            .collect(),
    ))
}

async fn create_output(
    State(state): State<AppState>,
    Json(request): Json<CreateOutputRequest>,
) -> Result<(StatusCode, Json<OutputResponse>), ApiError> {
    let mut engine = state.engine.lock().await;
    let output = engine.add_output_with_id(
        request.id.unwrap_or_else(Uuid::new_v4),
        request.name,
        request.transport,
    )?;
    Ok((
        StatusCode::CREATED,
        Json(OutputResponse {
            id: output.id,
            name: output.name,
            transport: output.transport,
            state: output.state,
        }),
    ))
}

async fn remove_output(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.engine.lock().await.remove_output(id)?;
    Ok(StatusCode::NO_CONTENT)
}
