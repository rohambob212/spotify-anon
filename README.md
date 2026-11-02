# spotify-anon

Keyless Spotify metadata client. Search tracks and fetch full track / album /
artist metadata **without a client id, client secret, or user login** — it does
what Spotify's own web player does for every anonymous visitor.

## Install

From GitHub (works now — no crates.io release needed):

```toml
[dependencies]
spotify-anon = { git = "https://github.com/rohambob212/spotify-anon", tag = "v0.1.0" }
```

Pin `rev = "<commit>"` instead of `tag` if you want a frozen build. Once the
crate is published, this becomes `spotify-anon = "0.1"`.

```rust
let spotify = spotify_anon::Spotify::new()?;              // no credentials anywhere

let hits = spotify.search_tracks("daft punk get lucky", 5)?;
let track = spotify.track("5ThumMdlPp3BWqSMdLefea")?;     // id, URI or URL
println!("{} — {}", track.name, track.artists().join(", "));
```

## API

| Call | Returns |
| --- | --- |
| `search_tracks(query, limit)` | `Vec<TrackHit>` — id, URI, name, artists, album, duration, cover, explicit, playable |
| `search_tracks_page(query, limit, offset)` | same, for paging (limit ≤ 50) |
| `track(id)` | `Track` — full metadata incl. ISRC, popularity, track/disc number, album (label, release date, cover) |
| `tracks(&[id])` | `Vec<Track>` |
| `album(id)` | `Album` — label, UPC, copyright, discs, track list, cover |
| `artist(id)` | `Artist` — popularity, biography, portraits, album/single group |
| `client_version()` | web player build the session came from |

Ids are accepted as a bare id (`5ThumMdlPp3BWqSMdLefea`), a URI
(`spotify:track:…`) or a URL (`https://open.spotify.com/track/…`).
`parse_link(input)` returns `(Kind, id)` when you need to know *what* the link
points at; `parse_id(input)` when you only need the id.

Handy accessors: `Track::id/uri/isrc/album_id/album_name/artists/cover_url(size)`
(with `gid_to_id`/`id_to_gid` for the hex gids `spclient` uses).

## Async

Enable the `async` feature to get the same client as an awaited handle:

```toml
spotify-anon = { git = "https://github.com/rohambob212/spotify-anon", tag = "v0.2.0", features = ["async"] }
```

```rust
let spotify = spotify_anon::AsyncSpotify::new();       // pool of min(cores, 4)
let hits = spotify.search_tracks("daft punk", 5).await?;
let track = spotify.track("5ThumMdlPp3BWqSMdLefea").await?;
```

It is deliberately *not* a second implementation of the protocol. Requests run
on a small pool of dedicated OS threads around the verified blocking client,
because `reqwest::blocking` owns a tokio runtime and dropping one inside an async
context panics ("Cannot drop a runtime in a context where blocking is not
allowed"). Consequences, so they are not surprises:

- concurrency is bounded by the pool — size it with
  `AsyncSpotify::with_pool(n, factory)`, calls beyond that queue
- each worker holds its own anonymous session (one extra token call per worker)
- `tokio::join!` over a cloned handle really does run in parallel
- if you ever need thousands of concurrent requests, write a native
  `reqwest::Client` implementation rather than enlarging this pool

The **sync** client must not be built or called from async code — reqwest's
blocking client panics in debug builds ("Cannot drop a runtime in a context
where blocking is not allowed") and blocks the executor in release. Use
`AsyncSpotify` for that, or keep the whole lifecycle inside a blocking task:

```rust
let track = tokio::task::spawn_blocking(move || {
    let spotify = spotify_anon::Spotify::new()?;   // built inside the task
    spotify.track("5ThumMdlPp3BWqSMdLefea")
})
.await??;
```

One trap *is* fixed inside the crate: a client built on a plain thread (or inside
`spawn_blocking`) can be **dropped** from async code. Without that, dropping it in
an async context panicked on reqwest's runtime shutdown even when it had been
built and used correctly.

## How it works

Three keyless calls, all of them things the browser does when you open
open.spotify.com:

1. **Anonymous session** — the web player's TOTP secret list is used to generate
   a code for `open.spotify.com/api/token`, which returns an anonymous access
   token and client id; that is traded for a *client token* at
   `clienttoken.spotify.com`. Spotify rotates the secret list, so the bundled
   table is refreshed from Spotify's published mirror on first use
   (`Builder::secrets_url`, `Builder::totp_secrets` to override).
2. **Search** — `api-partner.spotify.com/pathfinder/v1/query` with the
   `searchTracks` persisted query. Its `sha256Hash` changes with every web
   player build, so it is resolved from the shipped JS at runtime and cached
   (`Builder::cache_path`, default `$XDG_CACHE_HOME/spotify-anon/queries.json`).
3. **Entities** — `spclient.wg.spotify.com/metadata/4/{track,album,artist}/{gid}`.

Notes learned the hard way, both encoded in the crate:

- `clienttoken.spotify.com` answers **200 with an empty body** unless
  `Accept: application/json` is sent.
- `spclient` serves **protobuf** unless `Accept: application/json` is sent.

## Build & test

```bash
cargo build
cargo test                      # offline unit tests (TOTP vector, base62/gid, parsers)
cargo test --test live -- --ignored   # live network tests against Spotify
cargo run --example search -- "get lucky" 5
cargo run --example track -- 5ThumMdlPp3BWqSMdLefea
```

Blocking API on `reqwest` (rustls), no async runtime — it drops into CLIs
directly.

## Caveats

- This is Spotify's **private** web API, not the documented Web API: it can
  break without notice, and the anonymous session is rate limited. Pin a
  version and expect to update.
- The anonymous session can only reach public catalogue data. No user library,
  no playlists, no audio.
- `api.spotify.com/v1/*` with an anonymous token returns `429` — that is why
  this crate uses pathfinder and spclient.
- Subject to Spotify's terms; use the official Web API with credentials where
  the contract matters.
