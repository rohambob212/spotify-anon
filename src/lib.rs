//! Keyless Spotify client.
//!
//! Spotify's web player creates an anonymous session for every visitor without
//! asking for credentials. This crate does the same thing: it mints that session,
//! then uses Spotify's own GraphQL and metadata endpoints to search tracks and
//! look up full track, album and artist data.
//!
//! ```no_run
//! # fn main() -> Result<(), spotify_anon::Error> {
//! let spotify = spotify_anon::Spotify::new()?;
//!
//! for hit in spotify.search_tracks("daft punk get lucky", 5)? {
//!     println!("{} - {}", hit.name, hit.artists.join(", "));
//! }
//!
//! let track = spotify.track("5ThumMdlPp3BWqSMdLefea")?;
//! println!("{} by {}", track.name, track.artists().join(", "));
//! # Ok(()) }
//! ```
//!
//! No client id, client secret or user login is involved. The session is
//! anonymous (no user data is reachable), but it is still Spotify's private web
//! API: it can change without notice, and it is subject to Spotify's terms.
//! Do not build anything that needs a guaranteed-stable contract on top of it
//! without a bundled-secret / endpoint refresh path.

pub mod auth;
pub mod js;
pub mod model;
pub mod totp;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use model::{parse_id, to_uri, Album, Artist, Track, TrackHit};
pub use model::{
    AlbumRef, AlbumTrack, ArtistGroup, ArtistRef, Biography, Copyright, CoverGroup, Date, Disc,
    ExternalId, GidRef, Image,
};

use auth::{Auth, Session, BUNDLED_SECRETS, HOME, PATHFINDER_URL, SPC_URL, USER_AGENT};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("spotify answered {status}: {body}")]
    Status { status: u16, body: String },
    #[error("spotify graphql error: {0}")]
    Graphql(String),
    #[error("not a spotify id: {0}")]
    BadId(String),
    #[error("spotify did not expose the persisted query hash for '{0}' (build changed?)")]
    NoHash(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;

const DEFAULT_CACHE_FILE: &str = "spotify-anon/queries.json";

/// Anonymous Spotify client. Cheap to clone-share behind an `Arc`; all methods
/// take `&self` and the session is refreshed internally.
pub struct Spotify {
    http: reqwest::blocking::Client,
    auth: Auth,
    hashes: Mutex<BTreeMap<String, String>>,
    cache_path: Option<PathBuf>,
}

impl Spotify {
    /// Default client: bundled TOTP secrets plus a live refresh, hashes cached
    /// under the user cache dir.
    pub fn new() -> Result<Self> {
        Builder::new().build()
    }

    pub fn builder() -> Builder {
        Builder::new()
    }

    /// Search tracks. `limit` is clamped to 50 (one Spotify page).
    pub fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<TrackHit>> {
        self.search_tracks_page(query, limit, 0)
    }

    /// Search tracks with an explicit offset for paging.
    pub fn search_tracks_page(&self, query: &str, limit: usize, offset: usize) -> Result<Vec<TrackHit>> {
        let limit = limit.clamp(1, 50);
        let response = self.pathfinder(
            "searchTracks",
            json!({ "searchTerm": query, "offset": offset, "limit": limit }),
        )?;
        let root: SearchRoot = serde_json::from_value(response)?;
        let items = root
            .data
            .and_then(|d| d.search_v2)
            .and_then(|s| s.tracks_v2)
            .map(|p| p.items)
            .unwrap_or_default();
        Ok(items.into_iter().map(|i| i.item.data.into_hit()).collect())
    }

    /// Full metadata for one track. Accepts a bare id, a `spotify:track:` URI or
    /// an `open.spotify.com/track/...` URL.
    pub fn track(&self, id: &str) -> Result<Track> {
        self.entity("track", id)
    }

    /// Full metadata for several tracks. Fails on the first bad id; asks Spotify
    /// one track at a time (there is no batch endpoint on this API surface).
    pub fn tracks(&self, ids: &[&str]) -> Result<Vec<Track>> {
        ids.iter().map(|id| self.track(id)).collect()
    }

    pub fn album(&self, id: &str) -> Result<Album> {
        self.entity("album", id)
    }

    pub fn artist(&self, id: &str) -> Result<Artist> {
        self.entity("artist", id)
    }

    /// The web player build the session was minted from (handy in bug reports).
    pub fn client_version(&self) -> Result<String> {
        Ok(self.auth.session()?.client_version)
    }

    /// Drop the cached session; the next call mints a new one.
    pub fn invalidate(&self) {
        self.auth.invalidate();
    }

    fn session(&self) -> Result<Session> {
        self.auth.session()
    }

    fn entity<T: for<'de> Deserialize<'de>>(&self, kind: &str, id: &str) -> Result<T> {
        let gid = parse_id(id)
            .and_then(|id| model::id_to_gid(&id))
            .ok_or_else(|| Error::BadId(id.to_string()))?;
        let url = format!("{SPC_URL}/metadata/4/{kind}/{gid}");
        let body = self.send_authed(reqwest::Method::GET, &url, None)?;
        Ok(serde_json::from_str(&body)?)
    }

    fn pathfinder(&self, operation: &str, variables: Value) -> Result<Value> {
        let mut rediscoveries = 0;
        let mut auth_retries = 0;
        loop {
            let hash = self.persisted_hash(operation)?;
            let session = self.session()?;
            let extensions = json!({ "persistedQuery": { "version": 1, "sha256Hash": hash } });
            let url = format!(
                "{PATHFINDER_URL}?operationName={}&variables={}&extensions={}",
                urlencode(operation),
                urlencode(&variables.to_string()),
                urlencode(&extensions.to_string())
            );
            let request = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", session.access_token))
                .header("Client-Token", &session.client_token)
                .header("Spotify-App-Version", &session.client_version)
                .header("App-Platform", "WebPlayer")
                .header(reqwest::header::REFERER, format!("{HOME}/"))
                .header(reqwest::header::ORIGIN, HOME)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                // Spotify's front end 411s a POST without a content length
                .body(Vec::new());
            let (status, body) = self.execute(request)?;
            if status == 401 && auth_retries < 1 {
                auth_retries += 1;
                self.invalidate();
                continue;
            }
            if (status == 400 || status == 404)
                && body.contains("PersistedQuery")
                && rediscoveries < 1
            {
                rediscoveries += 1;
                self.hashes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(operation);
                self.forget_cached_hash(operation);
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(Error::Status { status, body });
            }
            let value: Value = serde_json::from_str(&body)?;
            if let Some(errors) = value.get("errors") {
                if !errors.is_null() {
                    return Err(Error::Graphql(errors.to_string()));
                }
            }
            return Ok(value);
        }
    }

    fn send_authed(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<String> {
        let mut auth_retries = 0;
        loop {
            let session = self.session()?;
            let mut request = self
                .http
                .request(method.clone(), url)
                .header("Authorization", format!("Bearer {}", session.access_token))
                .header("Client-Token", &session.client_token)
                .header("Spotify-App-Version", &session.client_version)
                .header("App-Platform", "WebPlayer")
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(body) = &body {
                request = request.json(body);
            }
            let (status, text) = self.execute(request)?;
            if status == 401 && auth_retries < 1 {
                auth_retries += 1;
                self.invalidate();
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(Error::Status { status, body: text });
            }
            return Ok(text);
        }
    }

    /// Runs a request with rate-limit backoff.
    fn execute(&self, request: reqwest::blocking::RequestBuilder) -> Result<(u16, String)> {
        let request = request.header(reqwest::header::USER_AGENT, USER_AGENT);
        let mut wait = Duration::from_millis(800);
        for attempt in 0..4 {
            let response = request
                .try_clone()
                .ok_or_else(|| Error::Protocol("request body not cloneable".into()))?
                .send()?;
            let status = response.status().as_u16();
            let retryable = status == 429 || status >= 500;
            let text = response.text()?;
            if !retryable || attempt == 3 {
                return Ok((status, text));
            }
            std::thread::sleep(wait);
            wait *= 2;
        }
        unreachable!("loop returns on the last attempt")
    }

    /// Persisted queries are content-addressed by a hash that changes with every
    /// web player build, so resolve it from the shipped JS and cache the result.
    fn persisted_hash(&self, operation: &str) -> Result<String> {
        if let Some(hash) = self
            .hashes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(operation)
            .cloned()
        {
            return Ok(hash);
        }
        let version = self.client_version().unwrap_or_default();
        if let Some(hash) = self.load_cached_hash(operation, &version) {
            self.store_hash(operation, &hash);
            return Ok(hash);
        }
        let hash = self.discover_hash(operation)?;
        self.store_hash(operation, &hash);
        self.save_cached_hash(operation, &hash, &version);
        Ok(hash)
    }

    fn discover_hash(&self, operation: &str) -> Result<String> {
        let html = self
            .http
            .get(HOME)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()?
            .text()?;
        let pack = js::script_src(&html, "web-player/web-player")
            .ok_or_else(|| Error::NoHash(operation.to_string()))?;
        let base = pack
            .rsplit_once('/')
            .map(|(base, _)| base.to_string())
            .ok_or_else(|| Error::NoHash(operation.to_string()))?;
        let pack_js = self
            .http
            .get(pack)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()?
            .text()?;
        for chunk in js::chunk_files(&pack_js) {
            let Ok(response) = self
                .http
                .get(format!("{base}/{chunk}"))
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .send()
            else {
                continue;
            };
            let Ok(text) = response.text() else { continue };
            if let Some(hash) = js::operation_hash(&text, operation) {
                return Ok(hash);
            }
        }
        Err(Error::NoHash(operation.to_string()))
    }

    fn store_hash(&self, operation: &str, hash: &str) {
        self.hashes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(operation.to_string(), hash.to_string());
    }

    fn cache(&self) -> Option<&PathBuf> {
        self.cache_path.as_ref()
    }

    fn read_cache(&self) -> CachedHashes {
        self.cache()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn load_cached_hash(&self, operation: &str, version: &str) -> Option<String> {
        let entry = self.read_cache().entries.remove(operation)?;
        (entry.version == version && entry.hash.len() == 64).then_some(entry.hash)
    }

    fn save_cached_hash(&self, operation: &str, hash: &str, version: &str) {
        let Some(path) = self.cache() else { return };
        let mut cache = self.read_cache();
        cache.entries.insert(
            operation.to_string(),
            CachedHash {
                version: version.to_string(),
                hash: hash.to_string(),
            },
        );
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string_pretty(&cache) {
            let _ = std::fs::write(path, raw);
        }
    }

    fn forget_cached_hash(&self, operation: &str) {
        let Some(path) = self.cache() else { return };
        let mut cache = self.read_cache();
        cache.entries.remove(operation);
        if let Ok(raw) = serde_json::to_string_pretty(&cache) {
            let _ = std::fs::write(path, raw);
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct CachedHashes {
    #[serde(default)]
    entries: BTreeMap<String, CachedHash>,
}

#[derive(Serialize, Deserialize)]
struct CachedHash {
    version: String,
    hash: String,
}

pub struct Builder {
    http: Option<reqwest::blocking::Client>,
    timeout: Duration,
    user_agent: String,
    secrets: BTreeMap<u32, Vec<u8>>,
    secrets_url: Option<String>,
    cache_path: Option<PathBuf>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {
            http: None,
            timeout: Duration::from_secs(30),
            user_agent: USER_AGENT.to_string(),
            secrets: BUNDLED_SECRETS
                .iter()
                .map(|(version, cipher)| (*version, cipher.to_vec()))
                .collect(),
            secrets_url: Some(auth::SECRETS_URL.to_string()),
            cache_path: default_cache_path(),
        }
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// Supply a caller-built HTTP client (proxies, custom TLS, ...).
    pub fn http_client(mut self, http: reqwest::blocking::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Replace the TOTP secret table entirely, e.g.
    /// `Builder::totp_secrets([(61, vec![...])])`.
    pub fn totp_secrets(
        mut self,
        secrets: impl IntoIterator<Item = (u32, Vec<u8>)>,
    ) -> Self {
        self.secrets = secrets.into_iter().collect();
        self
    }

    /// Where to refresh the rotating TOTP secrets from; `None` keeps only the
    /// bundled ones.
    pub fn secrets_url(mut self, url: Option<impl Into<String>>) -> Self {
        self.secrets_url = url.map(Into::into);
        self
    }

    /// Cache file for resolved persisted-query hashes; `None` disables caching.
    pub fn cache_path(mut self, path: Option<PathBuf>) -> Self {
        self.cache_path = path;
        self
    }

    pub fn build(self) -> Result<Spotify> {
        let http = match self.http {
            Some(http) => http,
            None => reqwest::blocking::Client::builder()
                .timeout(self.timeout)
                .user_agent(&self.user_agent)
                .build()?,
        };
        let mut auth = Auth::new(http.clone(), self.secrets, self.secrets_url);
        auth.refresh_secrets()?;
        Ok(Spotify {
            http,
            auth,
            hashes: Mutex::new(BTreeMap::new()),
            cache_path: self.cache_path,
        })
    }
}

fn default_cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join(DEFAULT_CACHE_FILE))
}

/// Minimal percent-encoding for query values (unreserved characters pass through).
pub(crate) fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ---- GraphQL response shapes for searchTracks ----

#[derive(Deserialize)]
struct SearchRoot {
    data: Option<SearchData>,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(rename = "searchV2")]
    search_v2: Option<SearchV2>,
}

#[derive(Deserialize)]
struct SearchV2 {
    #[serde(rename = "tracksV2")]
    tracks_v2: Option<Page>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    item: Wrapper,
}

#[derive(Deserialize)]
struct Wrapper {
    data: SearchTrack,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchTrack {
    #[serde(default)]
    id: String,
    #[serde(default)]
    uri: String,
    #[serde(default)]
    name: String,
    duration: Option<Duration2>,
    album_of_track: Option<AlbumOfTrack>,
    artists: Option<Artists>,
    playability: Option<Playability>,
    content_rating: Option<ContentRating>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Duration2 {
    #[serde(default)]
    total_milliseconds: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlbumOfTrack {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    cover_art: Option<CoverArt>,
}

#[derive(Deserialize)]
struct CoverArt {
    #[serde(default)]
    sources: Vec<Source>,
}

#[derive(Deserialize)]
struct Source {
    #[serde(default)]
    url: String,
    #[serde(default)]
    width: i32,
}

#[derive(Deserialize)]
struct Artists {
    #[serde(default)]
    items: Vec<ArtistItem>,
}

#[derive(Deserialize)]
struct ArtistItem {
    profile: Option<Profile>,
}

#[derive(Deserialize)]
struct Profile {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Playability {
    #[serde(default)]
    playable: bool,
}

#[derive(Deserialize)]
struct ContentRating {
    #[serde(default)]
    label: String,
}

impl SearchTrack {
    fn into_hit(self) -> TrackHit {
        let cover_url = self
            .album_of_track
            .as_ref()
            .and_then(|a| a.cover_art.as_ref())
            .and_then(|c| c.sources.iter().max_by_key(|s| s.width))
            .map(|s| s.url.clone());
        TrackHit {
            id: self.id,
            uri: self.uri,
            name: self.name,
            artists: self
                .artists
                .map(|a| {
                    a.items
                        .into_iter()
                        .filter_map(|i| i.profile.map(|p| p.name))
                        .collect()
                })
                .unwrap_or_default(),
            album: self.album_of_track.as_ref().map(|a| a.name.clone()),
            album_id: self.album_of_track.map(|a| a.id),
            duration_ms: self.duration.map(|d| d.total_milliseconds),
            cover_url,
            explicit: self
                .content_rating
                .map(|c| c.label.eq_ignore_ascii_case("EXPLICIT"))
                .unwrap_or(false),
            playable: self.playability.map(|p| p.playable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(urlencode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(urlencode("\"get lucky\""), "%22get%20lucky%22");
        assert_eq!(urlencode("{\"a\":1}"), "%7B%22a%22%3A1%7D");
    }

    #[test]
    fn parses_search_payload() {
        let json = r#"{"data":{"searchV2":{"tracksV2":{"items":[{"item":{"__typename":"TrackResponseWrapper","data":{"id":"2c2zC1S8akQlTZpvR3o8Ih","uri":"spotify:track:2c2zC1S8akQlTZpvR3o8Ih","name":"Get Lucky","duration":{"totalMilliseconds":248000},"contentRating":{"label":"NONE"},"playability":{"playable":true},"artists":{"items":[{"profile":{"name":"Daft Punk"}},{"profile":{"name":"Pharrell Williams"}}]},"albumOfTrack":{"id":"4m2880jivSbbyEGAKfITCa","name":"Random Access Memories","coverArt":{"sources":[{"url":"https://i.scdn.co/image/small","width":64},{"url":"https://i.scdn.co/image/big","width":640}]}}}}}]}}}}"#;
        let root: SearchRoot = serde_json::from_str(json).unwrap();
        let items = root.data.unwrap().search_v2.unwrap().tracks_v2.unwrap().items;
        let hit = items.into_iter().next().unwrap().item.data.into_hit();
        assert_eq!(hit.name, "Get Lucky");
        assert_eq!(hit.artists, vec!["Daft Punk", "Pharrell Williams"]);
        assert_eq!(hit.album.as_deref(), Some("Random Access Memories"));
        assert_eq!(hit.album_id.as_deref(), Some("4m2880jivSbbyEGAKfITCa"));
        assert_eq!(hit.duration_ms, Some(248_000));
        assert_eq!(hit.cover_url.as_deref(), Some("https://i.scdn.co/image/big"));
        assert_eq!(hit.playable, Some(true));
        assert!(!hit.explicit);
    }
}
