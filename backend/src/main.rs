use std::{env, net::SocketAddr, str::FromStr, sync::Arc};

use axum::{routing::get, Router};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::{broadcast, Mutex};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod janus;
mod routes;

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub station_id: Uuid,
    pub station_api_key: String,
    pub guest_jwt_secret: String,
    pub audio_engine_url: String,
    pub http_client: reqwest::Client,
    pub janus_http_url: String,
    pub janus_room_id: u64,
    pub janus_cue_room_id: u64,
    pub janus_room_pin: String,
    pub janus_ice_servers: Vec<Value>,
    pub janus_room_lock: Arc<Mutex<()>>,
    pub mixer_updates: broadcast::Sender<Value>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let database_url = required_env("DATABASE_URL")?;
    let station_api_key = required_env("STATION_API_KEY")?;
    let guest_jwt_secret = required_env("GUEST_JWT_SECRET")?;
    if guest_jwt_secret.len() < 32 {
        return Err("GUEST_JWT_SECRET must contain at least 32 characters".into());
    }

    let station_id = Uuid::from_str(&required_env("STATION_ID")?)?;
    let janus_room_id = env::var("JANUS_ROOM_ID")
        .unwrap_or_else(|_| "4815162342".to_owned())
        .parse::<u64>()?;
    let janus_room_pin = env::var("JANUS_ROOM_PIN")
        .unwrap_or_else(|_| "local-development-room-pin-change-me".to_owned());
    if janus_room_pin.len() < 12 {
        return Err("JANUS_ROOM_PIN must contain at least 12 characters".into());
    }
    let janus_cue_room_id = match env::var("JANUS_CUE_ROOM_ID") {
        Ok(value) if !value.trim().is_empty() => value.parse::<u64>()?,
        _ => janus_room_id
            .checked_add(1)
            .ok_or("JANUS_ROOM_ID is too large")?,
    };
    if janus_cue_room_id == janus_room_id {
        return Err("JANUS_CUE_ROOM_ID must differ from JANUS_ROOM_ID".into());
    }

    let mut janus_ice_servers = Vec::new();
    if let Ok(stun_server) = env::var("STUN_SERVER") {
        if !stun_server.trim().is_empty() {
            janus_ice_servers.push(json!({ "urls": stun_server }));
        }
    }
    if let Ok(turn_server) = env::var("TURN_SERVER_URL") {
        if !turn_server.trim().is_empty() {
            let username = required_env("TURN_SERVER_USERNAME")?;
            let credential = required_env("TURN_SERVER_CREDENTIAL")?;
            janus_ice_servers.push(json!({
                "urls": turn_server,
                "username": username,
                "credential": credential
            }));
        }
    }

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    sqlx::migrate!("../migrations").run(&pool).await?;
    sqlx::query(
        "INSERT INTO stations (id, name) VALUES ($1, 'Cloud Console') ON CONFLICT (id) DO NOTHING",
    )
    .bind(station_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO station_delay_settings (station_id) VALUES ($1) ON CONFLICT (station_id) DO NOTHING",
    )
    .bind(station_id)
    .execute(&pool)
    .await?;

    let (mixer_updates, _) = broadcast::channel(128);
    let state = AppState {
        pool,
        station_id,
        station_api_key,
        guest_jwt_secret,
        audio_engine_url: env::var("AUDIO_ENGINE_URL")
            .unwrap_or_else(|_| "http://audio-engine:8001".to_owned()),
        http_client: reqwest::Client::new(),
        janus_http_url: env::var("JANUS_HTTP_URL")
            .unwrap_or_else(|_| "http://janus:8088/janus".to_owned()),
        janus_room_id,
        janus_cue_room_id,
        janus_room_pin,
        janus_ice_servers,
        janus_room_lock: Arc::new(Mutex::new(())),
        mixer_updates,
    };
    routes::restore_delay_settings(&state).await?;
    let app = Router::new()
        .route("/api/health", get(routes::health))
        .route("/api/ready", get(routes::ready))
        .route("/api/openapi.json", get(routes::openapi))
        .route("/api/docs", get(routes::api_docs))
        .route("/api/auth/login", axum::routing::post(routes::auth_login))
        .route("/api/auth/validate", get(routes::auth_validate))
        .route(
            "/api/station/config",
            get(routes::station_config).patch(routes::update_station_settings),
        )
        .route(
            "/api/station/config/{kind}",
            get(routes::list_station_resources).post(routes::create_station_resource),
        )
        .route(
            "/api/station/config/{kind}/{id}",
            axum::routing::patch(routes::update_station_resource)
                .delete(routes::delete_station_resource),
        )
        .route("/api/station/health", get(routes::station_health))
        .route(
            "/api/guest-links",
            get(routes::list_guest_links).post(routes::create_guest_link),
        )
        .route(
            "/api/guest-links/{id}",
            axum::routing::delete(routes::invalidate_guest_link),
        )
        .route(
            "/api/guest-links/{token}/sessions",
            axum::routing::post(routes::create_guest_session),
        )
        .route(
            "/api/live-sessions",
            get(routes::list_live_sessions).post(routes::create_live_session),
        )
        .route(
            "/api/live-sessions/{id}/approve",
            axum::routing::post(routes::approve_guest_session),
        )
        .route(
            "/api/live-sessions/{id}/reject",
            axum::routing::post(routes::reject_guest_session),
        )
        .route(
            "/api/live-sessions/{id}/disconnect",
            axum::routing::post(routes::disconnect_guest_session),
        )
        .route(
            "/api/live-sessions/{id}/connection",
            get(routes::get_live_connection).patch(routes::update_live_connection),
        )
        .route("/api/janus-auth", get(routes::janus_websocket_auth))
        .route(
            "/api/guest-session/{id}",
            get(routes::get_guest_session),
        )
        .route("/api/users", get(routes::list_users))
        .route(
            "/api/delay",
            get(routes::get_delay_status).patch(routes::configure_delay),
        )
        .route("/api/delay/dump", axum::routing::post(routes::dump_delay))
        .route("/api/delay/end", axum::routing::post(routes::end_delay))
        .route(
            "/api/mixer/channels",
            get(routes::list_mixer_channels),
        )
        .route(
            "/api/mixer/channels/{id}",
            axum::routing::patch(routes::update_mixer_channel),
        )
        .route("/api/mixer/ws", get(routes::mixer_websocket))
        .route("/api/audio/{*path}", axum::routing::any(routes::proxy_audio))
        .with_state(state)
        .layer(
            tower_http::trace::TraceLayer::new_for_http().make_span_with(
                |request: &axum::http::Request<_>| {
                    tracing::info_span!("http_request", method = %request.method())
                },
            ),
        );

    let address = env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8000".to_owned());
    let listener = tokio::net::TcpListener::bind(address.parse::<SocketAddr>()?).await?;
    tracing::info!("Cloud Console API listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

fn required_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    env::var(name).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("required environment variable {name} is not set"),
        )
        .into()
    })
}
