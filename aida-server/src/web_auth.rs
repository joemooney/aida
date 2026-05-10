use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use reqwest::Url;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebAuthMode {
    None,
    Pin,
    Oidc,
    PinAndOidc,
}

impl WebAuthMode {
    pub fn from_env() -> Self {
        match std::env::var("AIDA_WEB_AUTH_MODE")
            .ok()
            .unwrap_or_else(|| "none".to_string())
            .to_lowercase()
            .as_str()
        {
            "pin" | "required" | "team" => WebAuthMode::Pin,
            "oidc" | "oauth" | "sso" => WebAuthMode::Oidc,
            "both" | "pin+oidc" | "all" => WebAuthMode::PinAndOidc,
            _ => WebAuthMode::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            WebAuthMode::None => "none",
            WebAuthMode::Pin => "pin",
            WebAuthMode::Oidc => "oidc",
            WebAuthMode::PinAndOidc => "both",
        }
    }

    pub fn pin_enabled(self) -> bool {
        matches!(self, WebAuthMode::Pin | WebAuthMode::PinAndOidc)
    }

    pub fn oidc_eligible(self) -> bool {
        matches!(self, WebAuthMode::Oidc | WebAuthMode::PinAndOidc)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    Editor,
    Viewer,
}

impl UserRole {
    fn from_str(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "admin" => UserRole::Admin,
            "viewer" => UserRole::Viewer,
            _ => UserRole::Editor,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            UserRole::Admin => "admin",
            UserRole::Editor => "editor",
            UserRole::Viewer => "viewer",
        }
    }

    pub fn can_write(self) -> bool {
        matches!(self, UserRole::Admin | UserRole::Editor)
    }

    pub fn is_admin(self) -> bool {
        matches!(self, UserRole::Admin)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub handle: String,
    pub name: String,
    pub project: String,
    pub role: UserRole,
}

#[derive(Debug, Clone)]
struct Session {
    user: AuthenticatedUser,
    expires_at: DateTime<Utc>,
}

enum SessionStore {
    Memory(RwLock<HashMap<String, Session>>),
    Sqlite { path: String },
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Discovery URL for OIDC metadata; loaded at config time but not yet
    /// consumed by the auth flow (we use the explicit endpoints below).
    #[allow(dead_code)]
    pub issuer_url: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_url: String,
    pub scopes: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: String,
}

impl OidcConfig {
    pub fn from_env() -> Option<Self> {
        let client_id = std::env::var("AIDA_OIDC_CLIENT_ID").ok()?;
        let redirect_url = std::env::var("AIDA_OIDC_REDIRECT_URL").ok()?;
        let issuer_url = std::env::var("AIDA_OIDC_ISSUER_URL").ok();
        let authorization_endpoint = std::env::var("AIDA_OIDC_AUTH_URL").ok().or_else(|| {
            issuer_url
                .as_ref()
                .map(|v| format!("{}/protocol/openid-connect/auth", v.trim_end_matches('/')))
        })?;
        let token_endpoint = std::env::var("AIDA_OIDC_TOKEN_URL").ok().or_else(|| {
            issuer_url
                .as_ref()
                .map(|v| format!("{}/protocol/openid-connect/token", v.trim_end_matches('/')))
        })?;
        let userinfo_endpoint = std::env::var("AIDA_OIDC_USERINFO_URL").ok().or_else(|| {
            issuer_url.as_ref().map(|v| {
                format!(
                    "{}/protocol/openid-connect/userinfo",
                    v.trim_end_matches('/')
                )
            })
        })?;

        Some(Self {
            issuer_url,
            client_id,
            client_secret: std::env::var("AIDA_OIDC_CLIENT_SECRET").ok(),
            redirect_url,
            scopes: std::env::var("AIDA_OIDC_SCOPES")
                .ok()
                .unwrap_or_else(|| "openid profile email".to_string()),
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct OidcTokenResponse {
    pub access_token: String,
}

#[derive(Debug, Deserialize)]
pub struct OidcUserInfo {
    pub sub: Option<String>,
    pub email: Option<String>,
    pub preferred_username: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug)]
#[allow(dead_code)] // InvalidState is reserved for upcoming CSRF state checks
pub enum OidcError {
    NotConfigured,
    InvalidState,
    ExchangeFailed(String),
    UserInfoFailed(String),
}

pub struct WebAuthState {
    mode: WebAuthMode,
    session_ttl: Duration,
    store: SessionStore,
    role_default: UserRole,
    role_admin_users: Vec<String>,
    role_editor_users: Vec<String>,
    role_viewer_users: Vec<String>,
    oidc_config: Option<OidcConfig>,
    oidc_state_ttl: Duration,
}

impl WebAuthState {
    pub fn new_from_env() -> Arc<Self> {
        let ttl_hours = std::env::var("AIDA_WEB_SESSION_TTL_HOURS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(24)
            .max(1);

        let store = match std::env::var("AIDA_WEB_SESSION_STORE")
            .ok()
            .unwrap_or_else(|| "memory".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "sqlite" | "persistent" => {
                let path = std::env::var("AIDA_WEB_SESSION_SQLITE_PATH")
                    .ok()
                    .unwrap_or_else(|| "/tmp/aida-web-sessions.sqlite3".to_string());
                if let Err(err) = initialize_sqlite_store(&path) {
                    tracing::warn!(
                        "Failed to initialize SQLite session store ({}), using memory",
                        err
                    );
                    SessionStore::Memory(RwLock::new(HashMap::new()))
                } else {
                    SessionStore::Sqlite { path }
                }
            }
            _ => SessionStore::Memory(RwLock::new(HashMap::new())),
        };

        Arc::new(Self {
            mode: WebAuthMode::from_env(),
            session_ttl: Duration::hours(ttl_hours),
            store,
            role_default: UserRole::from_str(
                &std::env::var("AIDA_WEB_DEFAULT_ROLE")
                    .ok()
                    .unwrap_or_else(|| "editor".to_string()),
            ),
            role_admin_users: parse_csv_env("AIDA_WEB_ADMIN_USERS"),
            role_editor_users: parse_csv_env("AIDA_WEB_EDITOR_USERS"),
            role_viewer_users: parse_csv_env("AIDA_WEB_VIEWER_USERS"),
            oidc_config: OidcConfig::from_env(),
            oidc_state_ttl: Duration::minutes(10),
        })
    }

    pub fn mode(&self) -> WebAuthMode {
        self.mode
    }

    pub fn is_enabled(&self) -> bool {
        self.mode != WebAuthMode::None
    }

    pub fn role_for_handle(&self, handle: &str) -> UserRole {
        let normalized = handle.to_ascii_lowercase();
        if self.role_admin_users.iter().any(|u| u == &normalized) {
            return UserRole::Admin;
        }
        if self.role_editor_users.iter().any(|u| u == &normalized) {
            return UserRole::Editor;
        }
        if self.role_viewer_users.iter().any(|u| u == &normalized) {
            return UserRole::Viewer;
        }
        self.role_default
    }

    pub fn oidc_enabled(&self) -> bool {
        self.mode.oidc_eligible() && self.oidc_config.is_some()
    }

    pub fn oidc_config(&self) -> Option<&OidcConfig> {
        self.oidc_config.as_ref()
    }

    pub async fn create_session(
        &self,
        user_id: String,
        handle: String,
        name: String,
        project: String,
        role: UserRole,
    ) -> String {
        let token = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
        let session = Session {
            user: AuthenticatedUser {
                user_id,
                handle,
                name,
                project,
                role,
            },
            expires_at: Utc::now() + self.session_ttl,
        };
        self.store_session(token.clone(), session).await;
        token
    }

    pub async fn get_session(&self, token: &str) -> Option<AuthenticatedUser> {
        self.cleanup_expired().await;
        self.fetch_session(token).await.map(|s| s.user)
    }

    pub async fn remove_session(&self, token: &str) {
        match &self.store {
            SessionStore::Memory(map) => {
                map.write().await.remove(token);
            }
            SessionStore::Sqlite { path } => {
                let path = path.clone();
                let token = token.to_string();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = Connection::open(path) {
                        let _ =
                            conn.execute("DELETE FROM web_sessions WHERE token=?1", params![token]);
                    }
                })
                .await;
            }
        }
    }

    async fn store_session(&self, token: String, session: Session) {
        match &self.store {
            SessionStore::Memory(map) => {
                map.write().await.insert(token, session);
            }
            SessionStore::Sqlite { path } => {
                let path = path.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = Connection::open(path) {
                        let _ = conn.execute(
                            "INSERT OR REPLACE INTO web_sessions (token, user_id, handle, name, project, role, expires_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![
                                token,
                                session.user.user_id,
                                session.user.handle,
                                session.user.name,
                                session.user.project,
                                session.user.role.as_str(),
                                session.expires_at.to_rfc3339(),
                            ],
                        );
                    }
                })
                .await;
            }
        }
    }

    async fn fetch_session(&self, token: &str) -> Option<Session> {
        match &self.store {
            SessionStore::Memory(map) => map.read().await.get(token).cloned(),
            SessionStore::Sqlite { path } => {
                let path = path.clone();
                let token = token.to_string();
                tokio::task::spawn_blocking(move || {
                    let conn = Connection::open(path).ok()?;
                    let mut stmt = conn
                        .prepare(
                            "SELECT user_id, handle, name, project, role, expires_at
                             FROM web_sessions WHERE token=?1",
                        )
                        .ok()?;
                    let row = stmt
                        .query_row(params![token], |r| {
                            let expires_at_s: String = r.get(5)?;
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?,
                                r.get::<_, String>(4)?,
                                expires_at_s,
                            ))
                        })
                        .ok()?;
                    let expires_at = DateTime::parse_from_rfc3339(&row.5)
                        .ok()?
                        .with_timezone(&Utc);
                    Some(Session {
                        user: AuthenticatedUser {
                            user_id: row.0,
                            handle: row.1,
                            name: row.2,
                            project: row.3,
                            role: UserRole::from_str(&row.4),
                        },
                        expires_at,
                    })
                })
                .await
                .ok()
                .flatten()
            }
        }
    }

    async fn cleanup_expired(&self) {
        match &self.store {
            SessionStore::Memory(map) => {
                let mut sessions = map.write().await;
                let expired = sessions
                    .iter()
                    .filter_map(|(k, s)| (s.expires_at <= Utc::now()).then_some(k.clone()))
                    .collect::<Vec<_>>();
                for key in expired {
                    sessions.remove(&key);
                }
            }
            SessionStore::Sqlite { path } => {
                let path = path.clone();
                let now = Utc::now().to_rfc3339();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = Connection::open(path) {
                        let _ = conn.execute(
                            "DELETE FROM web_sessions WHERE expires_at <= ?1",
                            params![now],
                        );
                        let _ = conn.execute(
                            "DELETE FROM web_oidc_state WHERE expires_at <= ?1",
                            params![now],
                        );
                    }
                })
                .await;
            }
        }
    }

    pub async fn create_oidc_state(&self, project: &str) -> String {
        let state = format!("{}{}", Uuid::now_v7().simple(), Uuid::now_v7().simple());
        let expires_at = Utc::now() + self.oidc_state_ttl;

        match &self.store {
            SessionStore::Memory(map) => {
                map.write().await.insert(
                    format!("oidc:{}", state),
                    Session {
                        user: AuthenticatedUser {
                            user_id: String::new(),
                            handle: String::new(),
                            name: String::new(),
                            project: project.to_string(),
                            role: self.role_default,
                        },
                        expires_at,
                    },
                );
            }
            SessionStore::Sqlite { path } => {
                let path = path.clone();
                let project = project.to_string();
                let state_for_db = state.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(conn) = Connection::open(path) {
                        let _ = conn.execute(
                            "INSERT OR REPLACE INTO web_oidc_state (state, project, expires_at)
                             VALUES (?1, ?2, ?3)",
                            params![state_for_db, project, expires_at.to_rfc3339()],
                        );
                    }
                })
                .await;
            }
        }

        state
    }

    pub async fn consume_oidc_state(&self, state: &str) -> Option<String> {
        self.cleanup_expired().await;
        match &self.store {
            SessionStore::Memory(map) => map
                .write()
                .await
                .remove(&format!("oidc:{}", state))
                .map(|s| s.user.project),
            SessionStore::Sqlite { path } => {
                let path = path.clone();
                let state = state.to_string();
                tokio::task::spawn_blocking(move || {
                    let conn = Connection::open(path).ok()?;
                    let project = conn
                        .query_row(
                            "SELECT project FROM web_oidc_state WHERE state=?1",
                            params![state.clone()],
                            |r| r.get::<_, String>(0),
                        )
                        .ok()?;
                    let _ =
                        conn.execute("DELETE FROM web_oidc_state WHERE state=?1", params![state]);
                    Some(project)
                })
                .await
                .ok()
                .flatten()
            }
        }
    }

    pub async fn build_oidc_authorize_url(
        &self,
        project: &str,
    ) -> Result<(String, String), OidcError> {
        let cfg = self.oidc_config().ok_or(OidcError::NotConfigured)?;
        let state = self.create_oidc_state(project).await;
        let mut url = Url::parse(&cfg.authorization_endpoint)
            .map_err(|e| OidcError::ExchangeFailed(format!("Invalid auth URL: {}", e)))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &cfg.client_id)
            .append_pair("redirect_uri", &cfg.redirect_url)
            .append_pair("scope", &cfg.scopes)
            .append_pair("state", &state);
        Ok((url.to_string(), state))
    }

    pub async fn exchange_oidc_code(&self, code: &str) -> Result<OidcUserInfo, OidcError> {
        let cfg = self.oidc_config().ok_or(OidcError::NotConfigured)?;

        let client = reqwest::Client::new();
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code.to_string()),
            ("redirect_uri", cfg.redirect_url.clone()),
            ("client_id", cfg.client_id.clone()),
        ];
        if let Some(secret) = &cfg.client_secret {
            form.push(("client_secret", secret.clone()));
        }

        let token_resp = client
            .post(&cfg.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| OidcError::ExchangeFailed(e.to_string()))?;

        if !token_resp.status().is_success() {
            let status = token_resp.status();
            let body = token_resp.text().await.unwrap_or_else(|_| "".to_string());
            return Err(OidcError::ExchangeFailed(format!(
                "Token exchange failed: {} {}",
                status, body
            )));
        }

        let token_json: OidcTokenResponse = token_resp
            .json()
            .await
            .map_err(|e| OidcError::ExchangeFailed(e.to_string()))?;

        let userinfo_resp = client
            .get(&cfg.userinfo_endpoint)
            .bearer_auth(token_json.access_token)
            .send()
            .await
            .map_err(|e| OidcError::UserInfoFailed(e.to_string()))?;

        if !userinfo_resp.status().is_success() {
            let status = userinfo_resp.status();
            let body = userinfo_resp
                .text()
                .await
                .unwrap_or_else(|_| "".to_string());
            return Err(OidcError::UserInfoFailed(format!(
                "Userinfo failed: {} {}",
                status, body
            )));
        }

        userinfo_resp
            .json::<OidcUserInfo>()
            .await
            .map_err(|e| OidcError::UserInfoFailed(e.to_string()))
    }
}

fn parse_csv_env(name: &str) -> Vec<String> {
    std::env::var(name)
        .ok()
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn initialize_sqlite_store(path: &str) -> anyhow::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(path)?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS web_sessions (
            token TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            handle TEXT NOT NULL,
            name TEXT NOT NULL,
            project TEXT NOT NULL,
            role TEXT NOT NULL,
            expires_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_web_sessions_expires_at ON web_sessions(expires_at)",
        [],
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS web_oidc_state (
            state TEXT PRIMARY KEY,
            project TEXT NOT NULL,
            expires_at TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_web_oidc_state_expires_at ON web_oidc_state(expires_at)",
        [],
    )?;
    Ok(())
}

pub fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    if let Some(value) = headers.get("x-session-token").and_then(|v| v.to_str().ok()) {
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let k = parts.next().unwrap_or("");
        let v = parts.next().unwrap_or("");
        if k == key && !v.is_empty() {
            return Some(v.to_string());
        }
    }
    None
}

fn is_public_api_path(path: &str) -> bool {
    path == "/api/ping"
        || path == "/api/status"
        || path == "/api/v2/auth/config"
        || path == "/api/v2/auth/login"
        || path == "/api/v2/auth/logout"
        || path == "/api/v2/auth/register"
        || path == "/api/v2/auth/oidc/start"
        || path == "/api/v2/auth/oidc/callback"
}

fn is_read_method(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD || method == Method::OPTIONS
}

pub async fn auth_middleware(
    State(auth): State<Arc<WebAuthState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api") || !auth.is_enabled() || is_public_api_path(path) {
        return next.run(req).await;
    }

    let token =
        extract_token(req.headers()).or_else(|| query_param(req.uri().query(), "session_token"));
    let token = match token {
        Some(token) => token,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Authentication required" })),
            )
                .into_response();
        }
    };

    let session = match auth.get_session(&token).await {
        Some(session) => session,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "Session expired or invalid" })),
            )
                .into_response();
        }
    };

    if let Some(project) = req
        .headers()
        .get("x-project")
        .and_then(|v| v.to_str().ok())
        .filter(|p| !p.is_empty())
    {
        if project != session.project {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "Session is for a different project" })),
            )
                .into_response();
        }
    }

    if path.starts_with("/api/v2/admin/") && !session.role.is_admin() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "Admin role required" })),
        )
            .into_response();
    }

    if !is_read_method(req.method()) && !session.role.can_write() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "Editor role required for write operations" })),
        )
            .into_response();
    }

    req.extensions_mut().insert(session);
    next.run(req).await
}
