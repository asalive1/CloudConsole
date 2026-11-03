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

mod pipeline;

use pipeline::{AudioEngine, AudioMetrics, DelayStatus, EngineError};

#[derive(Clone)]
struct AppState {
    engine: std::sync::Arc<Mutex<AudioEngine>>,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    gst::init()?;

    let engine = AudioEngine::new()
        .map_err(|error| format!("audio engine initialization failed: {error}"))?;
    let state = AppState {
        engine: std::sync::Arc::new(Mutex::new(engine)),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(health))
        .route("/metrics", get(metrics))
        .route("/delay", get(delay_status))
        .route("/delay/config", patch(configure_delay))
        .route("/delay/dump", post(dump_delay))
        .route("/delay/end", post(end_delay))
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/{id}", delete(remove_source))
        .route("/sources/{id}/recover", post(recover_source))
        .route("/outputs", get(list_outputs).post(create_output))
        .route("/outputs/{id}", delete(remove_output))
        .with_state(state);

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

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        gstreamer_version: gst::version_string().to_string(),
    })
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
    let source = engine.add_source(request.name, request.source_type, request.transport)?;
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
    let output = engine.add_output(request.name, request.transport)?;
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
