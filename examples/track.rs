//! `cargo run --example track -- 5ThumMdlPp3BWqSMdLefea 4m2880jivSbbyEGAKfITCa`

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let ids = if ids.is_empty() {
        vec!["5ThumMdlPp3BWqSMdLefea".to_string()]
    } else {
        ids
    };

    let spotify = spotify_anon::Spotify::new()?;
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();

    for track in spotify.tracks(&refs)? {
        println!("{:#?}", track);
        println!(
            "  -> id={:?} isrc={:?} album_id={:?} cover={:?}\n",
            track.id(),
            track.isrc(),
            track.album_id(),
            track.cover_url("DEFAULT")
        );
    }
    Ok(())
}
