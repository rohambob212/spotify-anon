//! Where the time goes: `cargo run --release --example timing`
//!
//! Run it with the cache dir removed for a cold measurement, then again for warm.

use std::time::Instant;

fn main() -> Result<(), spotify_anon::Error> {
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|base| base.join("spotify-anon/queries.json"))
        .ok();
    println!(
        "query-hash cache: {}",
        match &cache {
            Some(path) if path.exists() => format!("{} (present)", path.display()),
            Some(path) => format!("{} (absent -> cold)", path.display()),
            None => "disabled".into(),
        }
    );

    let spotify = spotify_anon::Spotify::new()?;

    let t = Instant::now();
    let version = spotify.client_version()?;
    println!(
        "session (home page + TOTP + token + client token): {:?}  [{version}]",
        t.elapsed()
    );

    let t = Instant::now();
    let track = spotify.track("45bsaWbpYBd7VdNxBwY3qe")?;
    println!(
        "track by id (no query hash needed):                  {:?}  [{}]",
        t.elapsed(),
        track.name
    );

    let t = Instant::now();
    let hits = spotify.search_tracks("googoosh makhlough", 1)?;
    println!(
        "search #1 (may resolve the query hash):             {:?}  [{} hits]",
        t.elapsed(),
        hits.len()
    );

    let t = Instant::now();
    let _ = spotify.search_tracks("daft punk", 1)?;
    println!(
        "search #2 (same session):                           {:?}",
        t.elapsed()
    );

    Ok(())
}
