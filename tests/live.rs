//! Live tests against Spotify. Ignored by default because they need network and
//! depend on Spotify's current build: `cargo test --test live -- --ignored`.

use spotify_anon::{Spotify, Track};

#[test]
#[ignore = "network"]
fn search_returns_playable_tracks() {
    let spotify = Spotify::new().unwrap();
    let hits = spotify.search_tracks("daft punk get lucky", 5).unwrap();
    assert!(!hits.is_empty(), "search returned nothing");
    let first = &hits[0];
    assert_eq!(first.id.len(), 22);
    assert!(first.uri.starts_with("spotify:track:"));
    assert!(!first.name.is_empty());
    assert!(!first.artists.is_empty());
    assert!(first.duration_ms.unwrap_or(0) > 10_000);
    assert!(first
        .cover_url
        .as_deref()
        .unwrap_or_default()
        .starts_with("https://"));
    assert!(spotify.search_tracks("get lucky", 1).unwrap().len() <= 1);
}

#[test]
#[ignore = "network"]
fn track_by_id_has_full_metadata() {
    let spotify = Spotify::new().unwrap();
    let track: Track = spotify.track("5ThumMdlPp3BWqSMdLefea").unwrap();
    assert_eq!(track.name, "Yare Dabestanie Man");
    assert_eq!(track.artists(), vec!["Fereydoon Foroughi"]);
    assert_eq!(track.id().as_deref(), Some("5ThumMdlPp3BWqSMdLefea"));
    assert_eq!(track.isrc().as_deref(), Some("DEBL60547570"));
    assert_eq!(track.album_name().as_deref(), Some("Fetneh Chakmehpoosh (Persian Music)"));
    assert_eq!(track.duration_ms, 216_137);
    assert!(track.cover_url("DEFAULT").is_some());

    // every accepted id form resolves to the same track
    for form in [
        "spotify:track:5ThumMdlPp3BWqSMdLefea",
        "https://open.spotify.com/track/5ThumMdlPp3BWqSMdLefea?si=abc",
    ] {
        assert_eq!(spotify.track(form).unwrap().gid, track.gid, "{form}");
    }
}

#[test]
#[ignore = "network"]
fn batch_tracks_and_entities() {
    let spotify = Spotify::new().unwrap();
    let ids = ["5ThumMdlPp3BWqSMdLefea", "45bsaWbpYBd7VdNxBwY3qe", "2goEbU50JgAVpLVhTDLo36"];
    let tracks = spotify.tracks(&ids).unwrap();
    assert_eq!(tracks.len(), 3);
    for track in &tracks {
        assert!(!track.name.is_empty(), "{:?}", track.gid);
        assert!(track.duration_ms > 0);
    }

    let album = spotify.album("4m2880jivSbbyEGAKfITCa").unwrap();
    assert_eq!(album.name, "Random Access Memories");
    assert_eq!(album.label, "Columbia");
    assert!(album.track_count() > 0);
    assert!(album.upc().is_some());
    assert!(album.copyright.iter().any(|c| !c.text.is_empty()));
    assert!(album.cover_url("LARGE").is_some());
    assert!(album.disc[0].track[0].id().is_some());

    let artist = spotify.artist("4tZwfgrHOc3mvqYlEYSvVi").unwrap();
    assert_eq!(artist.name, "Daft Punk");
    assert!(!artist.album_ids().is_empty());
    assert!(artist.portrait_url("DEFAULT").is_some());
}

#[test]
#[ignore = "network"]
fn search_head_can_be_fetched_in_full() {
    let spotify = Spotify::new().unwrap();
    let hit = spotify.search_tracks("random access memories get lucky", 1).unwrap().remove(0);
    let track = spotify.track(&hit.id).unwrap();
    assert_eq!(track.name, hit.name);
    assert_eq!(track.id().as_deref(), Some(hit.id.as_str()));
}

#[test]
#[ignore = "network"]
fn rejects_garbage_ids() {
    let spotify = Spotify::new().unwrap();
    assert!(spotify.track("not-a-track-id").is_err());
}
