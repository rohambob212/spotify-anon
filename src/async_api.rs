//! Async facade (feature `async`).
//!
//! Requests run on a small pool of dedicated OS threads, each owning its own
//! [`Spotify`] client, and the caller awaits the answer through a oneshot
//! channel. There is exactly one implementation of the Spotify protocol to keep
//! working against their private API — the verified sync one — and everything
//! here is transport.
//!
//! Why owner threads rather than one `tokio::task::spawn_blocking` per call:
//! `reqwest::blocking` creates and owns a tokio runtime, and dropping one from
//! inside an async context panics with *"Cannot drop a runtime in a context
//! where blocking is not allowed"*. Keeping each client's whole lifecycle on a
//! thread we own removes that failure mode (workers exit and drop their clients
//! on their own threads, never on the executor).
//!
//! Trade-offs, stated plainly:
//! - Concurrency is bounded by the pool size (default: cores, capped at 4).
//!   Calls beyond that queue.
//! - Each worker holds its own anonymous session, so a pool of 4 mints 4
//!   sessions — one token call each, cheaper than sharing one behind a mutex.
//! - Need thousands of concurrent requests? Write a native `reqwest::Client`
//!   implementation instead of enlarging this pool.
//!
//! ```no_run
//! # async fn demo() -> Result<(), spotify_anon::Error> {
//! let spotify = spotify_anon::AsyncSpotify::new();
//! let hits = spotify.search_tracks("daft punk", 5).await?;
//! let track = spotify.track("5ThumMdlPp3BWqSMdLefea").await?;
//! println!("{} / {}", hits.len(), track.name);
//! # Ok(()) }
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::{Album, Artist, Error, Result, Spotify, Track, TrackHit};

type Job = Box<dyn FnOnce(std::result::Result<&Spotify, &Error>) + Send>;

struct Worker {
    jobs: std::sync::mpsc::Sender<Job>,
}

struct Pool {
    workers: Vec<Worker>,
    next: AtomicUsize,
}

/// Cheap to clone; clones share the same worker pool.
#[derive(Clone)]
pub struct AsyncSpotify {
    pool: Arc<Pool>,
}

impl Default for AsyncSpotify {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncSpotify {
    /// Pool of `min(cores, 4)` workers, each creating its own client lazily on
    /// its own thread.
    pub fn new() -> Self {
        Self::with_pool(default_pool_size(), Spotify::new)
    }

    /// Pool with an explicit size and client factory — the factory runs once per
    /// worker, on that worker's thread, so the blocking client is never created
    /// (or dropped) inside an async context.
    pub fn with_pool<F>(size: usize, factory: F) -> Self
    where
        F: Fn() -> Result<Spotify> + Send + Sync + 'static,
    {
        let size = size.max(1);
        let factory = Arc::new(factory);
        let workers = (0..size)
            .map(|index| spawn_worker(index, Arc::clone(&factory)))
            .collect();
        Self {
            pool: Arc::new(Pool { workers, next: AtomicUsize::new(0) }),
        }
    }

    /// Worker threads in this pool.
    pub fn pool_size(&self) -> usize {
        self.pool.workers.len()
    }

    pub async fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<TrackHit>> {
        self.search_tracks_page(query, limit, 0).await
    }

    pub async fn search_tracks_page(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<TrackHit>> {
        let query = query.to_string();
        self.call(move |spotify| spotify.search_tracks_page(&query, limit, offset))
            .await
    }

    /// Full track metadata. Accepts a bare id, a `spotify:track:` URI or a URL.
    pub async fn track(&self, id: &str) -> Result<Track> {
        let id = id.to_string();
        self.call(move |spotify| spotify.track(&id)).await
    }

    /// Full metadata for several tracks, resolved one at a time.
    pub async fn tracks(&self, ids: Vec<String>) -> Result<Vec<Track>> {
        self.call(move |spotify| {
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            spotify.tracks(&refs)
        })
        .await
    }

    pub async fn album(&self, id: &str) -> Result<Album> {
        let id = id.to_string();
        self.call(move |spotify| spotify.album(&id)).await
    }

    pub async fn artist(&self, id: &str) -> Result<Artist> {
        let id = id.to_string();
        self.call(move |spotify| spotify.artist(&id)).await
    }

    pub async fn client_version(&self) -> Result<String> {
        self.call(|spotify| spotify.client_version()).await
    }

    /// Drop every worker's cached session; the next call re-mints one.
    pub fn invalidate(&self) {
        for worker in &self.pool.workers {
            let _ = worker.jobs.send(Box::new(|client| {
                if let Ok(spotify) = client {
                    spotify.invalidate();
                }
            }));
        }
    }

    async fn call<T, F>(&self, job: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Spotify) -> Result<T> + Send + 'static,
    {
        let (reply, answer) = tokio::sync::oneshot::channel();
        let index = self.pool.next.fetch_add(1, Ordering::Relaxed) % self.pool.workers.len();
        self.pool.workers[index]
            .jobs
            .send(Box::new(move |client| {
                let result = match client {
                    Ok(spotify) => job(spotify),
                    Err(err) => Err(Error::Protocol(err.to_string())),
                };
                let _ = reply.send(result);
            }))
            .map_err(|_| Error::Protocol("spotify worker thread is gone".into()))?;
        answer
            .await
            .map_err(|_| Error::Protocol("spotify worker dropped the request".into()))?
    }
}

impl std::fmt::Debug for AsyncSpotify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncSpotify")
            .field("pool_size", &self.pool.workers.len())
            .finish_non_exhaustive()
    }
}

fn default_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(2)
        .clamp(1, 4)
}

fn spawn_worker<F>(index: usize, factory: Arc<F>) -> Worker
where
    F: Fn() -> Result<Spotify> + Send + Sync + 'static,
{
    let (jobs, queue) = std::sync::mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name(format!("spotify-anon-{index}"))
        .spawn(move || {
            // created and dropped on this thread, never on an async runtime
            let client = factory();
            while let Ok(job) = queue.recv() {
                job(client.as_ref());
            }
        })
        .expect("spawn spotify worker thread");
    Worker { jobs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync + Clone>() {}

    #[test]
    fn handle_is_shareable_and_pool_is_bounded() {
        assert_send_sync::<AsyncSpotify>();
        let spotify = AsyncSpotify::new();
        assert!((1..=4).contains(&spotify.pool_size()), "{}", spotify.pool_size());
    }

    #[tokio::test]
    async fn bad_ids_fail_without_touching_the_network() {
        let spotify = AsyncSpotify::new();
        assert!(matches!(spotify.track("nope").await, Err(Error::BadId(_))));
        assert!(matches!(spotify.tracks(vec!["x".into()]).await, Err(Error::BadId(_))));
        assert!(matches!(spotify.album("").await, Err(Error::BadId(_))));
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn search_and_track_work_from_async() {
        let spotify = AsyncSpotify::new();
        let hits = spotify.search_tracks("daft punk get lucky", 3).await.unwrap();
        assert!(!hits.is_empty());

        let track = spotify.track("5ThumMdlPp3BWqSMdLefea").await.unwrap();
        assert_eq!(track.name, "Yare Dabestanie Man");

        // concurrent calls through cloned handles share the pool
        let other = spotify.clone();
        let (a, b) = tokio::join!(
            spotify.track("45bsaWbpYBd7VdNxBwY3qe"),
            other.album("4m2880jivSbbyEGAKfITCa"),
        );
        assert_eq!(a.unwrap().name, "Makhlough");
        assert_eq!(b.unwrap().name, "Random Access Memories");

        assert!(!spotify.client_version().await.unwrap().is_empty());
    }
}
