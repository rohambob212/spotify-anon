//! Parsing of the web player's JavaScript bundles, used to discover the
//! `sha256Hash` of a persisted GraphQL query (the names/hashes rotate with every
//! Spotify build, so they are resolved at runtime instead of being hardcoded).

/// All `{ 123: "text", 456: "text" }` literal objects in `js` with at least
/// `min_entries` pairs. Hand-rolled instead of pulling in a regex engine.
pub fn numeric_string_maps(js: &str, min_entries: usize) -> Vec<Vec<(i64, String)>> {
    let bytes = js.as_bytes();
    let mut maps = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some((pairs, end)) = parse_map(js, i) {
                if pairs.len() >= min_entries {
                    maps.push(pairs);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    maps
}

fn parse_map(js: &str, start: usize) -> Option<(Vec<(i64, String)>, usize)> {
    let bytes = js.as_bytes();
    let mut i = start + 1;
    let mut pairs = Vec::new();
    loop {
        i = skip_ws(bytes, i);
        let key_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == key_start {
            return None;
        }
        let key: i64 = js[key_start..i].parse().ok()?;
        i = skip_ws(bytes, i);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(bytes, i + 1);
        if bytes.get(i) != Some(&b'"') {
            return None;
        }
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != b'"' {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        let value = js[value_start..i].to_string();
        i = skip_ws(bytes, i + 1);
        pairs.push((key, value));
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return Some((pairs, i + 1)),
            _ => return None,
        }
    }
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    i
}

/// Chunk file names (`name.hash.js`) for the webpack lazy chunks. The hash map
/// (all values are 8 hex chars) is followed by its matching name map in the
/// bundle; keys pair the two. Search-ish chunks come first so a caller hunting
/// for a query hash usually stops after one fetch.
pub fn chunk_files(js: &str) -> Vec<String> {
    let maps = numeric_string_maps(js, 8);
    let Some((hash_idx, hash_map)) = maps
        .iter()
        .enumerate()
        .filter(|(_, m)| m.iter().all(|(_, v)| is_hash(v)))
        .max_by_key(|(_, m)| m.len())
    else {
        return Vec::new();
    };

    let shared_with = |m: &Vec<(i64, String)>| m.iter().filter(|(k, _)| hash_map.iter().any(|(hk, _)| hk == k)).count();

    // webpack emits the name map directly after the hash map; fall back to the
    // non-hash map sharing the most keys if the layout ever changes.
    let name_map = maps
        .get(hash_idx + 1)
        .filter(|m| m.iter().all(|(_, v)| !is_hash(v)) && shared_with(m) >= 8)
        .or_else(|| {
            maps.iter()
                .enumerate()
                .filter(|(i, m)| *i != hash_idx && m.iter().all(|(_, v)| !is_hash(v)))
                .max_by_key(|(_, m)| shared_with(m))
                .map(|(_, m)| m)
                .filter(|m| shared_with(m) >= 8)
        });

    let Some(name_map) = name_map else {
        return Vec::new();
    };
    let mut out: Vec<String> = name_map
        .iter()
        .filter_map(|(k, n)| {
            hash_map
                .iter()
                .find(|(hk, _)| hk == k)
                .map(|(_, h)| format!("{n}.{h}.js"))
        })
        .collect();
    out.sort_by_key(|f| if f.contains("search") { 0 } else { 1 });
    out.dedup();
    out
}

fn is_hash(v: &str) -> bool {
    v.len() == 8 && v.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Find the persisted-query hash for `operation` in a chunk body.
pub fn operation_hash(js: &str, operation: &str) -> Option<String> {
    for kind in ["query", "mutation"] {
        let needle = format!("\"{operation}\",\"{kind}\",\"");
        if let Some(idx) = js.find(&needle) {
            let start = idx + needle.len();
            if let Some(end) = js[start..].find('"') {
                let hash = &js[start..start + end];
                if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Some(hash.to_string());
                }
            }
        }
    }
    None
}

/// Value of `<script id="appServerConfig" type="text/plain">` decoded to JSON.
pub fn app_server_config(html: &str) -> Option<serde_json::Value> {
    let open = "<script id=\"appServerConfig\" type=\"text/plain\">";
    let idx = html.find(open)? + open.len();
    let end = html[idx..].find("</script>")?;
    let raw = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, html[idx..idx + end].trim()).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// First `src="....js"` link that contains `needle`.
pub fn script_src<'a>(html: &'a str, needle: &str) -> Option<&'a str> {
    let mut rest = html;
    while let Some(idx) = rest.find("src=\"") {
        let start = idx + 5;
        rest = &rest[start..];
        if let Some(end) = rest.find('"') {
            let url = &rest[..end];
            if url.contains(needle) && url.ends_with(".js") {
                return Some(url);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_maps_and_chunk_names() {
        // shape of the real bundle: a hash map, then its name map right after it,
        // pairing on shared keys
        let js = "x={100:\"aaaaaaaa\",101:\"bbbbbbbb\",102:\"cccccccc\",103:\"dddddddd\",104:\"eeeeeeee\",105:\"ffffffff\",106:\"11112222\",107:\"33334444\"}\
                  y={100:\"xpui-routes-search\",101:\"xpui-pip-mini-player\",102:\"chunk-a\",103:\"chunk-b\",104:\"chunk-c\",105:\"chunk-d\",106:\"chunk-e\",107:\"chunk-f\"}";
        let chunks = chunk_files(js);
        assert_eq!(chunks.len(), 8);
        assert_eq!(chunks[0], "xpui-routes-search.aaaaaaaa.js");
        assert_eq!(chunks[1], "xpui-pip-mini-player.bbbbbbbb.js");

        // no usable pair -> no chunks, never a panic
        assert!(chunk_files(r#"a={1:"x"}"#).is_empty());
    }

    #[test]
    fn reads_operation_hash() {
        let js = r#"a"searchTracks","query","b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751"b"#;
        assert_eq!(
            operation_hash(js, "searchTracks").as_deref(),
            Some("b02683192a98dde7966b5e6655a79eeb62713eab703eda9902c932818dd52751")
        );
        assert!(operation_hash(js, "getTrack").is_none());
    }

    #[test]
    fn finds_pack_script() {
        let html = r#"<script src="/a.js"></script><script src="https://open.spotifycdn.com/cdn/build/web-player/web-player.f954508a.js"></script>"#;
        assert_eq!(
            script_src(html, "web-player/web-player"),
            Some("https://open.spotifycdn.com/cdn/build/web-player/web-player.f954508a.js")
        );
    }
}
