use std::{
    collections::BTreeMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use serde_json::Value;
use url::Url;

use crate::{
    constants::{DEFAULT_HTTP_PORT, LOCAL_ENDPOINT, TOKEN_COOKIE_NAME},
    types::CaptureAuth,
};

#[derive(Debug, Clone)]
pub(crate) struct NanoKvmSession {
    pub(crate) endpoint: String,
    pub(crate) cookie_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionAuth {
    None,
    Token(String),
    Login { username: String, password: String },
}

static NEXT_SESSION_REP: AtomicU32 = AtomicU32::new(1);
static SESSION_REGISTRY: OnceLock<Mutex<BTreeMap<u32, NanoKvmSession>>> = OnceLock::new();

fn session_registry() -> &'static Mutex<BTreeMap<u32, NanoKvmSession>> {
    SESSION_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn register_session(session: NanoKvmSession) -> u32 {
    loop {
        let rep = NEXT_SESSION_REP.fetch_add(1, Ordering::Relaxed);
        if rep == 0 {
            continue;
        }

        let mut sessions = session_registry()
            .lock()
            .expect("session registry lock should not be poisoned");
        if sessions.insert(rep, session.clone()).is_none() {
            return rep;
        }
    }
}

pub(crate) fn lookup_session(rep: u32) -> Result<NanoKvmSession, String> {
    session_registry()
        .lock()
        .map_err(|_| "nanokvm session registry lock poisoned".to_string())?
        .get(&rep)
        .cloned()
        .ok_or_else(|| format!("nanokvm session not found: rep={rep}"))
}

pub(crate) fn remove_session(rep: u32) {
    if let Ok(mut sessions) = session_registry().lock() {
        sessions.remove(&rep);
    }
}

pub(crate) fn parse_session_auth(auth: CaptureAuth) -> Result<SessionAuth, String> {
    match auth {
        CaptureAuth::None => Ok(SessionAuth::None),
        CaptureAuth::Token(token) => {
            let token = token.trim();
            if token.is_empty() {
                return Err("nanokvm auth.token must not be empty".to_string());
            }
            Ok(SessionAuth::Token(token.to_string()))
        }
        CaptureAuth::Login(credentials) => {
            let username = credentials.username.trim();
            let password = credentials.password.trim();
            if username.is_empty() {
                return Err("nanokvm auth.login.username must not be empty".to_string());
            }
            if password.is_empty() {
                return Err("nanokvm auth.login.password must not be empty".to_string());
            }
            Ok(SessionAuth::Login {
                username: username.to_string(),
                password: password.to_string(),
            })
        }
    }
}

fn cookie_from_token(token: &str) -> String {
    format!("{TOKEN_COOKIE_NAME}={token}")
}

pub(crate) fn resolve_auth_cookie<F>(
    endpoint: &str,
    auth: SessionAuth,
    login_provider: F,
) -> Result<Option<String>, String>
where
    F: FnOnce(&str, &str, &str) -> Result<String, String>,
{
    match auth {
        SessionAuth::None => Ok(None),
        SessionAuth::Token(token) => Ok(Some(cookie_from_token(token.trim()))),
        SessionAuth::Login { username, password } => {
            let token = login_provider(endpoint, &username, &password)?;
            let token = token.trim();
            if token.is_empty() {
                return Err("nanokvm login returned an empty token".to_string());
            }
            Ok(Some(cookie_from_token(token)))
        }
    }
}

pub(crate) fn normalize_endpoint(endpoint: &str) -> Result<String, String> {
    let parsed = Url::parse(endpoint).map_err(|err| format!("invalid endpoint url: {err}"))?;
    if parsed.scheme() != "http" {
        return Err("nanokvm endpoint must use http:// scheme".to_string());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("nanokvm endpoint must not include userinfo".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("nanokvm endpoint must not include query or fragment".to_string());
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err("nanokvm endpoint must not include a path".to_string());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "nanokvm endpoint host is required".to_string())?
        .to_ascii_lowercase();
    let port = parsed.port().unwrap_or(DEFAULT_HTTP_PORT);
    if port == 0 {
        return Err("nanokvm endpoint port must be greater than zero".to_string());
    }

    let host = crate::common::format_host_for_url(&host);
    Ok(format!("http://{host}:{port}"))
}

pub(crate) fn create_session(
    endpoint: String,
    auth: SessionAuth,
) -> Result<NanoKvmSession, String> {
    let cookie_header = resolve_auth_cookie(&endpoint, auth, request_login_token)?;
    Ok(NanoKvmSession {
        endpoint,
        cookie_header,
    })
}

pub(crate) fn create_local_session(auth: SessionAuth) -> Result<NanoKvmSession, String> {
    create_session(LOCAL_ENDPOINT.to_string(), auth)
}

pub(crate) fn build_http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(10))
        .build()
}

pub(crate) fn request_login_token(
    endpoint: &str,
    username: &str,
    password: &str,
) -> Result<String, String> {
    let url = format!("{endpoint}/api/auth/login");
    let payload = serde_json::json!({
        "username": username,
        "password": password,
    })
    .to_string();

    let response = build_http_agent()
        .post(&url)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .send_string(&payload)
        .map_err(|err| map_http_error("nanokvm login request failed", err))?;

    let body = response
        .into_string()
        .map_err(|err| format!("nanokvm login response read failed: {err}"))?;
    parse_login_response(&body)
}

pub(crate) fn map_http_error(context: &str, err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            if body.trim().is_empty() {
                format!("{context}: http status {code}")
            } else {
                format!("{context}: http status {code}: {}", body.trim())
            }
        }
        ureq::Error::Transport(err) => format!("{context}: transport error: {err}"),
    }
}

pub(crate) fn parse_login_response(body: &str) -> Result<String, String> {
    let json: Value =
        serde_json::from_str(body).map_err(|err| format!("invalid login response json: {err}"))?;
    let code = json
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| "nanokvm login response missing numeric code".to_string())?;

    if code != 0 {
        let message = json
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("login failed");
        return Err(format!("nanokvm login failed(code={code}): {message}"));
    }

    let token = json
        .get("data")
        .and_then(|data| data.get("token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "nanokvm login response missing data.token".to_string())?;

    Ok(token.to_string())
}
