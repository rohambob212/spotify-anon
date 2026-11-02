//! `cargo run --example search -- "daft punk get lucky"`

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "get lucky".to_string());
    let limit = std::env::args()
        .nth(2)
        .and_then(|n| n.parse().ok())
        .unwrap_or(5);

    let spotify = spotify_anon::Spotify::new()?;
    eprintln!("web player build: {}", spotify.client_version()?);

    for hit in spotify.search_tracks(&query, limit)? {
        println!(
            "{}\n  {} — {}\n  album: {} ({})\n  {} ms | explicit: {} | playable: {:?}\n  {}",
            hit.name,
            hit.artists.join(", "),
            hit.uri,
            hit.album.clone().unwrap_or_default(),
            hit.album_id.clone().unwrap_or_default(),
            hit.duration_ms.unwrap_or(0),
            hit.explicit,
            hit.playable,
            hit.cover_url.clone().unwrap_or_default()
        );
    }
    Ok(())
}
