use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
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
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
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

    let claims = authorize_guest(&headers, &state)?;
    let session_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized)?;
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM sessions WHERE id = $1 AND station_id = $2 AND expires_at > NOW()",
    )
    .bind(session_id)
    .bind(state.station_id)
    .fetch_optional(&state.pool)
    .await?;
    let status = status.ok_or(ApiError::Unauthorized)?;
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
}

pub async fn list_guest_links(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<GuestLinkResponse>>, ApiError> {
    authorize_station(&headers, &state)?;
    let rows = sqlx::query(
        "SELECT id, name, expires_at, invalidated_at FROM guest_links WHERE station_id = $1 ORDER BY created_at DESC",
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
        }),
    ))
}

pub async fn invalidate_guest_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    authorize_station(&headers, &state)?;
    let result = sqlx::query(
        "UPDATE guest_links SET invalidated_at = COALESCE(invalidated_at, NOW()) WHERE id = $1 AND station_id = $2",
    )
    .bind(id)
    .bind(state.station_id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct GuestSessionRequest {
    name: String,
}

#[derive(Serialize)]
pub struct GuestSessionResponse {
    session_id: Uuid,
    status: &'static str,
    access_token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
struct GuestClaims {
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
    let session_id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO sessions (id, station_id, guest_link_id, role, display_name, status, expires_at) VALUES ($1, $2, $3, 'guest', $4, 'pending', $5) ON CONFLICT DO NOTHING",
    )
    .bind(session_id)
    .bind(state.station_id)
    .bind(guest_link_id)
    .bind(name)
    .bind(expires_at)
    .execute(&state.pool)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "this guest link already has an active session".to_owned(),
        ));
    }
    let claims = GuestClaims {
        sub: session_id.to_string(),
        station_id: state.station_id.to_string(),
        role: "guest".to_owned(),
        exp: expires_at.timestamp() as usize,
    };
    let access_token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(state.guest_jwt_secret.as_bytes()),
    )?;
    Ok((
        StatusCode::CREATED,
        Json(GuestSessionResponse {
            session_id,
            status: "pending",
            access_token,
            expires_at,
        }),
    ))
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

fn authorize_guest(headers: &HeaderMap, state: &AppState) -> Result<GuestClaims, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    let claims = decode::<GuestClaims>(
        token,
        &DecodingKey::from_secret(state.guest_jwt_secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    )
    .map_err(|_| ApiError::Unauthorized)?
    .claims;
    if claims.role != "guest" || claims.station_id != state.station_id.to_string() {
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
