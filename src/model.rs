use serde::{Deserialize, Serialize};

/// Spotify ids are base62 of a 128 bit gid. Accepts a bare id, a `spotify:kind:id`
/// URI or an `open.spotify.com/...` URL and returns the bare id.
pub fn parse_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.starts_with("spotify:") || trimmed.contains("://") {
        let without_query = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
        without_query
            .trim_end_matches('/')
            .rsplit([':', '/'])
            .find(|part| part.len() == 22)?
    } else {
        trimmed
    };
    if candidate.len() == 22 && candidate.bytes().all(|b| B62.contains(&b)) {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// What kind of thing a Spotify id refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Track,
    Album,
    Artist,
    Playlist,
    Episode,
    Show,
    Unknown,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Artist => "artist",
            Kind::Playlist => "playlist",
            Kind::Episode => "episode",
            Kind::Show => "show",
            Kind::Unknown => "unknown",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "track" => Kind::Track,
            "album" => Kind::Album,
            "artist" => Kind::Artist,
            "playlist" => Kind::Playlist,
            "episode" => Kind::Episode,
            "show" => Kind::Show,
            _ => return None,
        })
    }
}

/// Split any accepted link form into its kind and bare id.
///
/// Accepts `spotify:track:<id>`, `https://open.spotify.com/track/<id>?si=...`,
/// locale-prefixed URLs (`/intl-de/track/<id>`) and a bare id (kind
/// [`Kind::Unknown`], since a bare id carries no type).
pub fn parse_link(input: &str) -> Option<(Kind, String)> {
    let trimmed = input.trim();
    let without_query = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let id = parse_id(trimmed)?;
    let kind = without_query
        .trim_end_matches('/')
        .rsplit([':', '/'])
        .skip_while(|part| *part != id)
        .skip(1)
        .find_map(Kind::from_token)
        .unwrap_or(Kind::Unknown);
    Some((kind, id))
}

const B62: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// base62 id -> 32 char hex gid (what `spclient` metadata endpoints want).
pub fn id_to_gid(id: &str) -> Option<String> {
    let mut value: u128 = 0;
    for byte in id.bytes() {
        let digit = B62.iter().position(|&b| b == byte)? as u128;
        value = value.checked_mul(62)?.checked_add(digit)?;
    }
    Some(format!("{value:032x}"))
}

/// 32 char hex gid -> base62 id.
pub fn gid_to_id(gid: &str) -> Option<String> {
    let mut value = u128::from_str_radix(gid, 16).ok()?;
    let mut out = Vec::new();
    while value > 0 {
        out.push(B62[(value % 62) as usize]);
        value /= 62;
    }
    out.reverse();
    String::from_utf8(out).ok()
}

/// `spotify:track:<id>` for a bare id or any accepted id form.
pub fn to_uri(kind: &str, input: &str) -> Option<String> {
    Some(format!("spotify:{kind}:{}", parse_id(input)?))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtistRef {
    pub gid: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalId {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Image {
    #[serde(default)]
    pub file_id: String,
    #[serde(default)]
    pub size: String,
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoverGroup {
    #[serde(default)]
    pub image: Vec<Image>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Date {
    #[serde(default)]
    pub year: i32,
    #[serde(default)]
    pub month: i32,
    #[serde(default)]
    pub day: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlbumRef {
    #[serde(default)]
    pub gid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Vec<ArtistRef>,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub date: Date,
    #[serde(default)]
    pub cover_group: CoverGroup,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Track {
    pub gid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Vec<ArtistRef>,
    #[serde(default)]
    pub album: Option<AlbumRef>,
    #[serde(default, rename = "duration")]
    pub duration_ms: u32,
    #[serde(default)]
    pub number: i32,
    #[serde(default)]
    pub disc_number: i32,
    #[serde(default)]
    pub popularity: i32,
    #[serde(default)]
    pub external_id: Vec<ExternalId>,
    #[serde(default)]
    pub canonical_uri: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
}

impl Track {
    pub fn id(&self) -> Option<String> {
        gid_to_id(&self.gid)
    }

    pub fn uri(&self) -> String {
        self.canonical_uri
            .clone()
            .unwrap_or_else(|| format!("spotify:track:{}", self.id().unwrap_or_default()))
    }

    pub fn artists(&self) -> Vec<String> {
        self.artist.iter().map(|a| a.name.clone()).collect()
    }

    pub fn album_name(&self) -> Option<String> {
        self.album.as_ref().map(|a| a.name.clone())
    }

    pub fn album_id(&self) -> Option<String> {
        self.album.as_ref().and_then(|a| gid_to_id(&a.gid))
    }

    pub fn isrc(&self) -> Option<String> {
        self.external_id
            .iter()
            .find(|e| e.kind.eq_ignore_ascii_case("isrc"))
            .map(|e| e.id.clone())
    }

    /// Cover art URL. `size` is one of DEFAULT, SMALL, LARGE, XLARGE.
    pub fn cover_url(&self, size: &str) -> Option<String> {
        let images = &self.album.as_ref()?.cover_group.image;
        images
            .iter()
            .find(|i| i.size.eq_ignore_ascii_case(size))
            .or_else(|| images.iter().max_by_key(|i| i.width))
            .map(|i| format!("https://i.scdn.co/image/{}", i.file_id))
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_ms as f64 / 1000.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GidRef {
    #[serde(default)]
    pub gid: String,
}

/// Entries of `Album::disc[].track` only carry a gid (plus optional metadata on
/// some albums), so name/artist may be empty — resolve them with
/// [`crate::Spotify::track`] if you need them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlbumTrack {
    #[serde(default)]
    pub gid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Vec<ArtistRef>,
    #[serde(default)]
    pub number: i32,
    #[serde(default)]
    pub duration: u32,
}

impl AlbumTrack {
    pub fn id(&self) -> Option<String> {
        gid_to_id(&self.gid)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Disc {
    #[serde(default)]
    pub number: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub track: Vec<AlbumTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Copyright {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Album {
    pub gid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: Vec<ArtistRef>,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub date: Date,
    #[serde(default)]
    pub popularity: i32,
    #[serde(default)]
    pub external_id: Vec<ExternalId>,
    #[serde(default)]
    pub disc: Vec<Disc>,
    #[serde(default)]
    pub copyright: Vec<Copyright>,
    #[serde(default)]
    pub cover_group: CoverGroup,
    #[serde(default)]
    pub canonical_uri: Option<String>,
}

impl Album {
    pub fn id(&self) -> Option<String> {
        gid_to_id(&self.gid)
    }

    pub fn upc(&self) -> Option<String> {
        self.external_id
            .iter()
            .find(|e| e.kind.eq_ignore_ascii_case("upc"))
            .map(|e| e.id.clone())
    }

    pub fn cover_url(&self, size: &str) -> Option<String> {
        let images = &self.cover_group.image;
        images
            .iter()
            .find(|i| i.size.eq_ignore_ascii_case(size))
            .or_else(|| images.iter().max_by_key(|i| i.width))
            .map(|i| format!("https://i.scdn.co/image/{}", i.file_id))
    }

    pub fn track_count(&self) -> usize {
        self.disc.iter().map(|d| d.track.len()).sum()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Biography {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub portraits: serde_json::Value,
}

/// An entry of `artist.album_group` / `single_group` / ... — a wrapper holding
/// the album gids in that group.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtistGroup {
    #[serde(default)]
    pub album: Vec<GidRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Artist {
    pub gid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub popularity: i32,
    #[serde(default)]
    pub biography: Vec<Biography>,
    #[serde(default)]
    pub portrait_group: CoverGroup,
    #[serde(default)]
    pub album_group: Vec<ArtistGroup>,
    #[serde(default)]
    pub single_group: Vec<ArtistGroup>,
    #[serde(default)]
    pub compilation_group: Vec<ArtistGroup>,
    #[serde(default)]
    pub appears_on_group: Vec<ArtistGroup>,
}

impl Artist {
    pub fn id(&self) -> Option<String> {
        gid_to_id(&self.gid)
    }

    pub fn bio_text(&self) -> Option<&str> {
        self.biography
            .iter()
            .map(|b| b.text.as_str())
            .find(|text| !text.is_empty())
    }

    pub fn album_ids(&self) -> Vec<String> {
        self.album_group
            .iter()
            .filter_map(|group| group.album.first())
            .filter_map(|album| gid_to_id(&album.gid))
            .collect()
    }

    pub fn portrait_url(&self, size: &str) -> Option<String> {
        let images = &self.portrait_group.image;
        images
            .iter()
            .find(|i| i.size.eq_ignore_ascii_case(size))
            .or_else(|| images.iter().max_by_key(|i| i.width))
            .map(|i| format!("https://i.scdn.co/image/{}", i.file_id))
    }
}

/// A track as returned by search: lighter than [`Track`] but carries cover art
/// and album ids, so it is enough to pick a match and then fetch it in full.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackHit {
    pub id: String,
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u32>,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub playable: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_gid_roundtrip_matches_spotify() {
        assert_eq!(
            id_to_gid("5ThumMdlPp3BWqSMdLefea").as_deref(),
            Some("c194bb60a279473baa7eaaab041dfe8a")
        );
        assert_eq!(
            gid_to_id("c194bb60a279473baa7eaaab041dfe8a").as_deref(),
            Some("5ThumMdlPp3BWqSMdLefea")
        );
        assert!(gid_to_id("4m2880jivSbbyEGAKfITCa").is_none(), "gid_to_id takes hex gids");
        assert!(id_to_gid("4m2880jivSbbyEGAKfITCa").is_some());
    }

    #[test]
    fn accepts_uri_and_url_id_forms() {
        let id = "5ThumMdlPp3BWqSMdLefea";
        assert_eq!(parse_id(id).as_deref(), Some(id));
        assert_eq!(parse_id(&format!("spotify:track:{id}")).as_deref(), Some(id));
        assert_eq!(
            parse_id(&format!("https://open.spotify.com/track/{id}?si=abc")).as_deref(),
            Some(id)
        );
        assert_eq!(parse_id("nope").as_deref(), None);
    }

    #[test]
    fn parse_link_yields_kind_and_id() {
        let id = "5ThumMdlPp3BWqSMdLefea";
        for (input, expected) in [
            (format!("spotify:track:{id}"), Kind::Track),
            (format!("https://open.spotify.com/track/{id}?si=abc"), Kind::Track),
            (format!("https://open.spotify.com/intl-de/album/{id}"), Kind::Album),
            (format!("https://open.spotify.com/playlist/{id}"), Kind::Playlist),
            (id.to_string(), Kind::Unknown),
        ] {
            let (kind, parsed) = parse_link(&input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(kind, expected, "{input}");
            assert_eq!(parsed, id, "{input}");
        }
        assert!(parse_link("not a link").is_none());
        assert_eq!(Kind::Track.as_str(), "track");
    }

    #[test]
    fn parses_a_real_track_payload() {
        let json = r#"{"gid":"c194bb60a279473baa7eaaab041dfe8a","name":"Yare Dabestanie Man",
            "artist":[{"gid":"3772273517b44b9ba868d37ff6be82ce","name":"Fereydoon Foroughi"}],
            "duration":216137,"number":8,"external_id":[{"type":"isrc","id":"DEBL60547570"}],
            "album":{"gid":"352800867c08433d867a446c8464129f","name":"Fetneh Chakmehpoosh",
            "date":{"year":2013,"month":1,"day":1},
            "cover_group":{"image":[{"file_id":"ab67616d00001e0235ae2a933aff7694677513ff","size":"DEFAULT","width":300,"height":300}]}}}"#;
        let track: Track = serde_json::from_str(json).unwrap();
        assert_eq!(track.id().as_deref(), Some("5ThumMdlPp3BWqSMdLefea"));
        assert_eq!(track.artists(), vec!["Fereydoon Foroughi"]);
        assert_eq!(track.isrc().as_deref(), Some("DEBL60547570"));
        assert_eq!(track.album_name().as_deref(), Some("Fetneh Chakmehpoosh"));
        assert_eq!(track.duration_secs().round() as u32, 216);
        assert!(track.cover_url("DEFAULT").unwrap().starts_with("https://i.scdn.co/image/"));
    }
}
