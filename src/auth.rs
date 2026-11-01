use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::js;
use crate::totp;
use crate::{Error, Result};

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

pub const HOME: &str = "https://open.spotify.com";
pub const TOKEN_URL: &str = "https://open.spotify.com/api/token";
pub const CLIENT_TOKEN_URL: &str = "https://clienttoken.spotify.com/v1/clienttoken";
pub const PATHFINDER_URL: &str = "https://api-partner.spotify.com/pathfinder/v1/query";
pub const SPC_URL: &str = "https://spclient.wg.spotify.com";
pub const SECRETS_URL: &str =
    "https://code.thetadev.de/ThetaDev/spotify-secrets/raw/branch/main/secrets/secretDict.json";

/// Secrets shipped with the crate so a first run works offline. Spotify rotates
/// these; `Builder::secrets_url(None)` disables the refresh, and
/// `Builder::totp_secrets` overrides them entirely.
pub const BUNDLED_SECRETS: &[(u32, &[u8])] = &[
    (
        59,
        &[
            123, 105, 79, 70, 110, 59, 52, 125, 60, 49, 80, 70, 89, 75, 80, 86, 63, 53, 123, 37, 117,
            49, 52, 93, 77, 62, 47, 86, 48, 104, 68, 72,
        ],
    ),
    (
        60,
        &[79, 109, 69, 123, 90, 65, 46, 74, 94, 34, 58, 48, 70, 71, 92, 85, 122, 63, 91, 64, 87, 87],
    ),
    (
        61,
        &[
            44, 55, 47, 42, 70, 40, 34, 114, 76, 74, 50, 111, 120, 97, 75, 76, 94, 102, 43, 69, 49,
            120, 118, 80, 64, 78,
        ],
    ),
];

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "accessTokenExpirationTimestampMs", default)]
    expires_at_ms: u64,
    #[serde(rename = "clientId", default)]
    client_id: String,
}

#[derive(Debug, Deserialize)]
struct ClientTokenResponse {
    granted_token: GrantedToken,
}

#[derive(Debug, Deserialize)]
struct GrantedToken {
    token: String,
    #[serde(default)]
    expires_in: u64,
}

pub struct Session {
    pub access_token: String,
    pub client_token: String,
    pub client_id: String,
    pub client_version: String,
    pub expires_at_ms: u64,
    pub fetched_at: SystemTime,
}

impl Session {
    pub fn is_fresh(&self) -> bool {
        let now = now_ms();
        // client tokens are shorter lived; refresh with the access token
        now + 30_000 < self.expires_at_ms.min(self.fetched_at_ms() + 1_800_000)
    }

    fn fetched_at_ms(&self) -> u64 {
        self.fetched_at
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

pub struct Auth {
    http: reqwest::blocking::Client,
    secrets: BTreeMap<u32, Vec<u8>>,
    secrets_url: Option<String>,
    session: Mutex<Option<Session>>,
}

impl Auth {
    pub fn new(
        http: reqwest::blocking::Client,
        secrets: BTreeMap<u32, Vec<u8>>,
        secrets_url: Option<String>,
    ) -> Self {
        Self {
            http,
            secrets,
            secrets_url,
            session: Mutex::new(None),
        }
    }

    pub fn secrets(&self) -> &BTreeMap<u32, Vec<u8>> {
        &self.secrets
    }

    /// Refresh the secret list from Spotify's own published mirror. Failure is
    /// not fatal: the bundled secrets are kept.
    pub fn refresh_secrets(&mut self) -> Result<()> {
        let Some(url) = self.secrets_url.clone() else {
            return Ok(());
        };
        let Ok(response) = self.http.get(&url).send() else {
            return Ok(());
        };
        if !response.status().is_success() {
            return Ok(());
        }
        let Ok(raw) = response.json::<BTreeMap<String, Vec<u8>>>() else {
            return Ok(());
        };
        let mut found = false;
        for (version, cipher) in raw {
            if let Ok(version) = version.parse::<u32>() {
                if cipher.len() >= 16 {
                    self.secrets.insert(version, cipher);
                    found = true;
                }
            }
        }
        let _ = found;
        Ok(())
    }

    /// A valid session, fetching or refreshing it as needed.
    pub fn session(&self) -> Result<Session> {
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(session) = guard.as_ref() {
            if session.is_fresh() {
                return Ok(session.snapshot());
            }
        }
        let session = self.fetch_session()?;
        *guard = Some(session.snapshot());
        Ok(session)
    }

    /// Drop the cached session, e.g. after a 401.
    pub fn invalidate(&self) {
        *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn fetch_session(&self) -> Result<Session> {
        let html = self
            .http
            .get(HOME)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()?
            .text()?;
        let client_version = js::app_server_config(&html)
            .and_then(|cfg| cfg.get("clientVersion")?.as_str().map(str::to_string))
            .ok_or_else(|| Error::Protocol("no clientVersion in appServerConfig".into()))?;

        let now = now_seconds();
        let mut last_error = None;
        for version in self.secrets.keys().copied().collect::<Vec<_>>().into_iter().rev() {
            let ver = version.to_string();
            let cipher = &self.secrets[&version];
            let code = totp::totp_code(&totp::totp_secret_b32(cipher), now);
            let response = self
                .http
                .get(TOKEN_URL)
                .query(&[
                    ("reason", "init"),
                    ("productType", "web-player"),
                    ("totp", code.as_str()),
                    ("totpVer", ver.as_str()),
                    ("totpServer", code.as_str()),
                ])
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::REFERER, format!("{HOME}/"))
                .header("App-Platform", "WebPlayer")
                .send()?;
            let status = response.status();
            let body = response.text()?;
            if !status.is_success() {
                last_error = Some(Error::Status {
                    status: status.as_u16(),
                    body: body.clone(),
                });
                continue;
            }
            let token: TokenResponse = serde_json::from_str(&body)?;
            let expires_at_ms = if token.expires_at_ms > 0 {
                token.expires_at_ms
            } else {
                now_ms() + 3_600_000
            };
            let client_token = self.fetch_client_token(&client_version, &token.client_id)?;
            return Ok(Session {
                access_token: token.access_token,
                client_token,
                client_id: token.client_id,
                client_version,
                expires_at_ms,
                fetched_at: SystemTime::now(),
            });
        }
        Err(last_error.unwrap_or_else(|| Error::Protocol("no TOTP secrets available".into())))
    }

    fn fetch_client_token(&self, client_version: &str, client_id: &str) -> Result<String> {
        let body = serde_json::json!({
            "client_data": {
                "client_version": client_version,
                "client_id": client_id,
                "js_sdk_data": {
                    "device_brand": "unknown",
                    "device_model": "unknown",
                    "os": "windows",
                    "os_version": "NT 10.0",
                    "device_id": format!("{:032x}", rand_u128()),
                    "device_type": "computer",
                }
            }
        });
        let response = self
            .http
            .post(CLIENT_TOKEN_URL)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::ORIGIN, HOME)
            .json(&body)
            .send()?;
        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            return Err(Error::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        let parsed: ClientTokenResponse = serde_json::from_str(&text)?;
        if parsed.granted_token.token.is_empty() {
            return Err(Error::Protocol(
                "empty client token; Spotify answers 200 with an empty body when it does not like the request"
                    .into(),
            ));
        }
        let _ = parsed.granted_token.expires_in;
        Ok(parsed.granted_token.token)
    }
}

impl Session {
    fn snapshot(&self) -> Session {
        Session {
            access_token: self.access_token.clone(),
            client_token: self.client_token.clone(),
            client_id: self.client_id.clone(),
            client_version: self.client_version.clone(),
            expires_at_ms: self.expires_at_ms,
            fetched_at: self.fetched_at,
        }
    }
}

fn rand_u128() -> u128 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    let addr = &nanos as *const u128 as u128;
    nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(addr)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn now_seconds() -> u64 {
    now_ms() / 1000
}
