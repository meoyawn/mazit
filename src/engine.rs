use crate::{
    config,
    database::{Database, Episode, Source},
    downloads::{DownloadManager, MAX_TRANSFERS, Phase, Transfer},
    network::Cover,
    storage::Storage,
    youtube::YouTube,
};
use anyhow::{Context, Result, ensure};
use futures::{StreamExt, stream};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ViewState {
    pub sources: Vec<Source>,
    pub covers: HashMap<String, Arc<Cover>>,
    pub busy: bool,
    pub configured: bool,
    pub message: String,
}
pub enum Command {
    Add(String),
    Refresh(Option<String>),
    ReloadConfig,
}
#[derive(Clone)]
pub struct Engine {
    pub state: Arc<RwLock<ViewState>>,
    pub downloads: DownloadManager,
    sender: mpsc::UnboundedSender<Command>,
}

pub struct Core {
    pub db: Database,
    pub youtube: YouTube,
    pub client: reqwest::Client,
    pub directory: PathBuf,
    covers: RwLock<HashMap<String, Arc<Cover>>>,
    downloads: DownloadManager,
    _lock: std::fs::File,
}
impl Core {
    pub fn new(directory: PathBuf, cookies: Option<&Path>) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("mazit.lock"))?;
        lock.try_lock()
            .context("Another Mazit process is using this library")?;
        let transfers = directory.join("transfers");
        cleanup_transfers(&transfers)?;
        config::export_legacy_storage_binding(&directory)?;
        let db = Database::open(&directory.join("mazit.sqlite"))?;
        let covers = RwLock::new(load_covers(&directory, &db.sources()?));
        let (client, jar) = crate::network::client(cookies)?;
        let youtube = YouTube::start(
            client.clone(),
            Arc::new(move || crate::network::youtube_cookie(&jar)),
        );
        log::info!("Library opened");
        Ok(Self {
            db,
            youtube,
            client,
            directory,
            covers,
            downloads: DownloadManager::default(),
            _lock: lock,
        })
    }
    pub async fn add(&self, url: &str) -> Result<String> {
        let (kind, id, url) = self.youtube.resolve(url).await?;
        log::info!("Adding subscription {kind}:{id}");
        self.db.add(&kind, &id, &url)
    }
    pub fn bind_storage(&self, identity: &str) -> Result<()> {
        config::bind_storage(&self.directory, identity, !self.db.sources()?.is_empty())
    }
    pub async fn sync(
        &self,
        id: &str,
        storage: &Storage,
        changed: &(dyn Fn() + Sync),
    ) -> Result<()> {
        let started = Instant::now();
        log::info!("Sync started source={id}");
        let result = self.sync_inner(id, storage, changed).await;
        if let Err(error) = &result {
            log::error!(
                "Sync failed source={id} elapsed={:.1}s: {error:#}",
                started.elapsed().as_secs_f64()
            );
            self.db
                .phase(id, "error", Some(&crate::redact(&format!("{error:#}"))))?;
            self.db.postpone(id)?;
        } else {
            log::info!(
                "Sync completed source={id} elapsed={:.1}s",
                started.elapsed().as_secs_f64()
            );
        }
        changed();
        result
    }
    async fn sync_inner(
        &self,
        id: &str,
        storage: &Storage,
        changed: &(dyn Fn() + Sync),
    ) -> Result<()> {
        let source = self.db.source(id)?;
        log::info!("Scanning source={id}");
        self.db.phase(id, "scanning", None)?;
        changed();
        let snapshot = retry(&format!("Scan source={id}"), || {
            self.youtube.snapshot(&source.kind, &source.youtube_id)
        })
        .await?;
        log::info!(
            "Scan completed source={id} episodes={}",
            snapshot.videos.len()
        );
        self.db.snapshot(id, &snapshot)?;
        let source = self.db.source(id)?;
        self.db.phase(id, "downloading", None)?;
        changed();
        let cover = if let Some(url) = &snapshot.cover_url {
            log::info!("Downloading cover source={id}");
            Some(Arc::new(
                retry(&format!("Download cover source={id}"), || {
                    crate::network::download_cover(&self.client, url)
                })
                .await?,
            ))
        } else {
            None
        };
        cache_cover(&self.directory, &source, cover.as_deref())?;
        if let Some(cover) = &cover {
            self.covers.write().insert(id.into(), cover.clone());
        } else {
            self.covers.write().remove(id);
        }
        changed();
        let pending = self.db.pending_episodes(id)?;
        log::info!("Transferring source={id} pending={}", pending.len());
        let transfers = self.downloads.enqueue(
            id,
            &source.title,
            &pending
                .iter()
                .map(|episode| (episode.video.id.clone(), episode.video.title.clone()))
                .collect::<Vec<_>>(),
        );
        let source_ref = &source;
        let outcomes = stream::iter(pending.into_iter().zip(transfers))
            .map(|(episode, transfer)| async move {
                let active = transfer.acquire().await;
                let mut outcome = Ok(());
                for attempt in 1..=3 {
                    transfer.attempt(attempt);
                    outcome = self
                        .transfer(source_ref, &episode, storage, &transfer)
                        .await;
                    if let Err(error) = &outcome {
                        transfer.error(error);
                        if attempt < 3 {
                            transfer.phase(Phase::Retrying);
                            tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))).await;
                        }
                    } else {
                        break;
                    }
                }
                active.finish(&outcome);
                if let Err(error) = &outcome {
                    log::error!(
                        "Transfer failed source={id} video={}: {error:#}",
                        episode.video.id
                    );
                }
                changed();
                outcome
            })
            // Keep one admission waiting even at the cap so its worker timer can
            // detect a stalled connection when there are no progress callbacks.
            .buffer_unordered(MAX_TRANSFERS + 1)
            .collect::<Vec<_>>()
            .await;
        for outcome in outcomes {
            outcome?;
        }
        self.db.phase(id, "publishing", None)?;
        log::info!("Publishing RSS source={id}");
        changed();
        let feed_url = publish(
            &self.db.source(id)?,
            &self.db.episodes(id)?,
            storage,
            cover.as_deref(),
        )
        .await?;
        self.db.published(id, &feed_url)?;
        log::info!("RSS published source={id}");
        // Publication happens before deletion, so a failed upload never breaks the previous feed.
        self.db.phase(id, "cleaning", None)?;
        changed();
        for episode in self.db.episodes(id)?.into_iter().filter(|e| !e.present) {
            log::info!(
                "Removing obsolete audio source={id} video={}",
                episode.video.id
            );
            storage
                .delete(&format!("{}/{}.m4a", source.folder, episode.video.id))
                .await?;
            self.db.forget(id, &episode.video.id)?;
        }
        self.db.phase(id, "idle", None)?;
        changed();
        Ok(())
    }
    async fn transfer(
        &self,
        source: &Source,
        episode: &Episode,
        storage: &Storage,
        transfer: &Transfer,
    ) -> Result<()> {
        let started = Instant::now();
        let id = &episode.video.id;
        log::info!("Resolving audio source={} video={id}", source.id);
        let request = self.youtube.media(id).await.context("Resolve audio")?;
        with_transfer_directory(&self.directory.join("transfers"), |temp| async move {
            let input = temp.path().join("download");
            let output = temp.path().join("audio.m4a");
            log::info!(
                "Downloading audio video={id} itag={} mime={} bitrate={} bytes={}",
                request.itag,
                request.mime_type,
                request.bitrate,
                request.bytes
            );
            let download_started = Instant::now();
            crate::network::download(&self.client, &request, &input, Some(transfer))
                .await
                .context("Download audio")?;
            log::info!(
                "Downloaded audio video={id} elapsed={:.1}s speed={:.2} MiB/s",
                download_started.elapsed().as_secs_f64(),
                request.bytes as f64 / 1048576.0 / download_started.elapsed().as_secs_f64()
            );
            log::info!("Preparing M4A video={id}");
            transfer.phase(Phase::Preparing);
            let audio = output.clone();
            // Keep ownership in the blocking worker too: cancellation cannot orphan its output.
            let worker_directory = temp.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                let _directory = worker_directory;
                crate::audio::prepare_m4a(&input, &audio)
            })
            .await
            .context("Audio worker stopped")?
            .context("Prepare M4A")?;
            let key = format!("{}/{}.m4a", source.folder, episode.video.id);
            log::info!("Uploading audio video={id} bytes={bytes}");
            transfer.phase(Phase::Uploading);
            storage
                .put_file(&key, &output)
                .await
                .context("Upload audio")?;
            let mut video = episode.video.clone();
            video.title = request.title;
            video.description = request.description;
            video.duration = request.duration;
            video.published = request.published;
            self.db
                .uploaded(&source.id, &video, bytes, &storage.url(&key)?)?;
            log::info!(
                "Transfer completed source={} video={id} bytes={bytes} elapsed={:.1}s",
                source.id,
                started.elapsed().as_secs_f64()
            );
            Ok(())
        })
        .await
    }
}

fn cleanup_transfers(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("mazit-") {
            std::fs::remove_dir_all(entry.path()).context("Remove interrupted audio transfer")?;
        }
    }
    Ok(())
}

async fn with_transfer_directory<T, F, Fut>(directory: &Path, operation: F) -> Result<T>
where
    F: FnOnce(Arc<tempfile::TempDir>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let temp = Arc::new(
        tempfile::Builder::new()
            .prefix("mazit-")
            .tempdir_in(directory)?,
    );
    let result = operation(temp.clone()).await;
    let cleanup = Arc::try_unwrap(temp)
        .map_err(|_| anyhow::anyhow!("Audio worker still owns temporary files"))?
        .close()
        .context("Remove local audio files");
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("Local audio cleanup also failed: {cleanup:#}")))
        }
    }
}

fn load_covers(directory: &Path, sources: &[Source]) -> HashMap<String, Arc<Cover>> {
    sources
        .iter()
        .filter_map(|source| {
            let path = directory.join("covers").join(&source.folder);
            let cover = std::fs::read(path)
                .ok()
                .and_then(|bytes| Cover::from_bytes(bytes).ok())?;
            Some((source.id.clone(), Arc::new(cover)))
        })
        .collect()
}

fn cache_cover(directory: &Path, source: &Source, cover: Option<&Cover>) -> Result<()> {
    let directory = directory.join("covers");
    let path = directory.join(&source.folder);
    if let Some(cover) = cover {
        use std::io::Write;
        std::fs::create_dir_all(&directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
        temporary.write_all(&cover.bytes)?;
        temporary.persist(path).context("Save cover image")?;
    } else if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error).context("Remove cover image");
    }
    Ok(())
}

async fn publish(
    source: &Source,
    episodes: &[Episode],
    storage: &Storage,
    cover: Option<&Cover>,
) -> Result<String> {
    let cover_url = if let Some(cover) = cover {
        let key = format!("{}/cover.{}", source.folder, cover.extension);
        log::info!(
            "Uploading cover source={} bytes={}",
            source.id,
            cover.bytes.len()
        );
        retry(&format!("Upload cover source={}", source.id), || {
            storage.put_bytes(&key, cover.bytes.clone(), cover.mime)
        })
        .await?;
        let mut url = url::Url::parse(&storage.url(&key)?)?;
        // Podcast clients need a changed artwork URL to refresh their cached image.
        url.query_pairs_mut()
            .append_pair("v", &format!("{:x}", Sha256::digest(&cover.bytes)));
        Some(url.to_string())
    } else {
        None
    };
    let key = format!("{}/rss.xml", source.folder);
    let feed_url = storage.url(&key)?;
    let feed = crate::rss::render(source, episodes, &feed_url, cover_url.as_deref())?;
    // Use the generic XML MIME type so browsers display the feed in their XML viewer.
    retry(&format!("Publish RSS source={}", source.id), || {
        storage.put_text(&key, feed.clone(), "application/xml; charset=utf-8")
    })
    .await?;
    Ok(feed_url)
}

pub async fn retry<T, F, Fut>(label: &str, mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for attempt in 0..3 {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt == 2 => return Err(error).with_context(|| label.to_string()),
            Err(error) => {
                let delay = 1 << attempt;
                log::warn!(
                    "{label} attempt={}/3 failed; retry in {delay}s: {error:#}",
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
        }
    }
    unreachable!()
}

impl Engine {
    #[cfg(all(test, feature = "ui-tests"))]
    pub(crate) fn for_test(state: ViewState) -> (Self, mpsc::UnboundedReceiver<Command>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                state: Arc::new(RwLock::new(state)),
                downloads: DownloadManager::default(),
                sender,
            },
            receiver,
        )
    }

    pub fn start(core: Core, runtime: &tokio::runtime::Handle) -> Result<Self> {
        let state = Arc::new(RwLock::new(ViewState::default()));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let engine = Self {
            state: state.clone(),
            downloads: core.downloads.clone(),
            sender,
        };
        let config_path = config::path()?;
        let initial_text = match config::read_text(&config_path) {
            Ok(text) => text,
            Err(error) => {
                log::error!("Read configuration: {error:#}");
                state.write().message = crate::redact(&error.to_string());
                None
            }
        };
        let mut storage = None;
        if let Some(text) = &initial_text {
            let loaded = (|| -> Result<Storage> {
                let settings = config::parse(text)?;
                let candidate = Storage::new(settings.clone())?;
                core.bind_storage(&settings.identity())?;
                Ok(candidate)
            })();
            match loaded {
                Ok(candidate) => {
                    log::info!("Storage configuration loaded");
                    storage = Some(candidate);
                }
                Err(error) => {
                    log::error!("Load configuration: {error:#}");
                    state.write().message = crate::redact(&error.to_string());
                }
            }
        }
        state.write().configured = storage.is_some();
        state.write().sources = core.db.sources()?;
        state.write().covers = core.covers.read().clone();
        runtime.spawn(async move {
            let changed = || {
                if let Ok(sources) = core.db.sources() {
                    let mut state = state.write();
                    state.sources = sources;
                    state.covers = core.covers.read().clone();
                }
            };
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let command = tokio::select! {
                    command = receiver.recv() => {
                        let Some(command) = command else { break };
                        Some(command)
                    },
                    _ = interval.tick() => None,
                };
                let mut ids = Vec::new();
                state.write().busy = true;
                let operation: Result<()> = async {
                    match command {
                        Some(Command::Add(url)) => {
                            ensure!(storage.is_some(), "Fill in config.toml first");
                            ids.push(core.add(&url).await?);
                        }
                        Some(Command::Refresh(selected)) => {
                            ids = core
                                .db
                                .sources()?
                                .into_iter()
                                .filter(|source| {
                                    selected.as_ref().is_none_or(|id| id == &source.id)
                                })
                                .map(|s| s.id)
                                .collect();
                        }
                        Some(Command::ReloadConfig) => {
                            let settings = config::load(&config_path)?;
                            let candidate = Storage::new(settings.clone())?;
                            core.bind_storage(&settings.identity())?;
                            candidate.verify().await?;
                            log::info!("Storage configuration reloaded and verified");
                            storage = Some(candidate);
                            state.write().configured = true;
                            state.write().message = "Storage config loaded and verified".into();
                            ids = core
                                .db
                                .sources()?
                                .into_iter()
                                .map(|source| source.id)
                                .collect();
                        }
                        None => {
                            ids = core
                                .db
                                .sources()?
                                .into_iter()
                                .filter(|s| s.next_sync <= chrono::Utc::now().timestamp())
                                .map(|s| s.id)
                                .collect();
                        }
                    }
                    if let Some(storage) = &storage {
                        // Multiple podcasts can progress, sharing the same transfer budget.
                        stream::iter(ids)
                            .for_each_concurrent(4, |id| {
                                let core = &core;
                                let changed = &changed;
                                let state = &state;
                                async move {
                                    if let Err(error) = core.sync(&id, storage, changed).await {
                                        state.write().message = crate::redact(&error.to_string());
                                    }
                                }
                            })
                            .await;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = operation {
                    log::error!("Engine operation failed: {error:#}");
                    state.write().message = crate::redact(&error.to_string());
                }
                changed();
                state.write().busy = false;
            }
            log::info!("Sync worker stopped");
        });
        Ok(engine)
    }
    pub fn command(&self, command: Command) {
        let name = match &command {
            Command::Add(_) => "add subscription",
            Command::Refresh(_) => "refresh",
            Command::ReloadConfig => "reload configuration",
        };
        log::info!("Command requested: {name}");
        if self.sender.send(command).is_err() {
            log::error!("Sync worker is unavailable; command={name}");
            let mut state = self.state.write();
            state.busy = false;
            state.message =
                "Sync worker stopped. Open logs for details, then restart Mazit.".into();
        }
    }
}
pub fn data_directory() -> Result<PathBuf> {
    Ok(directories::BaseDirs::new()
        .context("Find application support directory")?
        .data_dir()
        .join("Mazit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StorageConfig;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, Ordering},
        thread::JoinHandle,
    };

    struct Upload {
        headers: String,
        body: Vec<u8>,
    }

    impl Upload {
        fn path(&self) -> &str {
            self.headers
                .split_whitespace()
                .nth(1)
                .unwrap()
                .split('?')
                .next()
                .unwrap()
        }
    }

    struct MockStorage {
        storage: Storage,
        uploads: Arc<parking_lot::Mutex<Vec<Upload>>>,
        stop: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl MockStorage {
        fn new(reject_covers: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let storage = Storage::new(StorageConfig::S3 {
                endpoint: format!("http://{}", listener.local_addr().unwrap()),
                region: "test".into(),
                bucket: "podcasts".into(),
                root: "library".into(),
                public_base_url: "https://audio.example.com/public".into(),
                access_key_id: "test".into(),
                secret_access_key: "test".into(),
            })
            .unwrap();
            let uploads = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let received = uploads.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let worker = std::thread::spawn(move || {
                while !stopping.load(Ordering::Relaxed) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 4096];
                    let header_end = loop {
                        let count = socket.read(&mut buffer).unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&buffer[..count]);
                        if let Some(index) = request.windows(4).position(|part| part == b"\r\n\r\n")
                        {
                            break index + 4;
                        }
                    };
                    let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
                    assert!(headers.starts_with("PUT "));
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    while request.len() < header_end + length {
                        let count = socket.read(&mut buffer).unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&buffer[..count]);
                    }
                    let reject =
                        reject_covers && headers.lines().next().unwrap().contains("/cover.");
                    received.lock().push(Upload {
                        headers,
                        body: request[header_end..].to_vec(),
                    });
                    let status = if reject { "403 Forbidden" } else { "200 OK" };
                    write!(
                        socket,
                        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .unwrap();
                }
            });
            Self {
                storage,
                uploads,
                stop,
                worker: Some(worker),
            }
        }
    }

    impl Drop for MockStorage {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let result = self.worker.take().unwrap().join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }

    fn source(kind: &str) -> Source {
        let db = Database::open(Path::new(":memory:")).unwrap();
        let id = db
            .add(
                kind,
                "test",
                "https://www.youtube.com/playlist?list=test&view=1",
            )
            .unwrap();
        db.snapshot(
            &id,
            &crate::youtube::Snapshot {
                title: "Arts & <Crafts>".into(),
                description: "Description".into(),
                cover_url: None,
                videos: Vec::new(),
            },
        )
        .unwrap();
        db.source(&id).unwrap()
    }

    #[tokio::test]
    async fn local_audio_is_removed_after_s3_upload_and_on_failure() {
        let directory = tempfile::tempdir().unwrap();
        let mock = MockStorage::new(false);
        let storage = &mock.storage;
        with_transfer_directory(directory.path(), |temp| async move {
            let input = temp.path().join("download");
            let output = temp.path().join("audio.m4a");
            std::fs::write(&input, b"original audio")?;
            std::fs::write(&output, b"prepared audio")?;
            storage.put_file("episode.m4a", &output).await?;
            assert!(input.exists() && output.exists());
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(mock.uploads.lock()[0].body, b"prepared audio");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

        // Download, conversion and upload errors all leave the same owned scope.
        for stage in ["download", "conversion", "upload"] {
            let result: Result<()> = with_transfer_directory(directory.path(), |temp| async move {
                std::fs::write(temp.path().join("download"), b"partial audio")?;
                std::fs::write(temp.path().join("audio.m4a"), b"partial output")?;
                anyhow::bail!("{stage} failed")
            })
            .await;
            assert!(result.is_err());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }

        let rejecting = MockStorage::new(true);
        let storage = &rejecting.storage;
        let result = with_transfer_directory(directory.path(), |temp| async move {
            let output = temp.path().join("audio.m4a");
            std::fs::write(&output, b"prepared audio")?;
            storage.put_file("cover.m4a", &output).await
        })
        .await;
        assert!(result.is_err());
        assert!(!rejecting.uploads.lock().is_empty());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancelled_transfer_cleans_local_files_and_restart_cleans_crash_leftovers() {
        let directory = tempfile::tempdir().unwrap();
        let mut transfer = Box::pin(with_transfer_directory(
            directory.path(),
            |temp| async move {
                std::fs::write(temp.path().join("download"), b"partial audio")?;
                std::future::pending::<()>().await;
                Ok(())
            },
        ));
        assert!(futures::poll!(&mut transfer).is_pending());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        drop(transfer);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

        let interrupted = directory.path().join("mazit-interrupted");
        std::fs::create_dir(&interrupted).unwrap();
        std::fs::write(interrupted.join("audio.m4a"), b"leftover").unwrap();
        let other = directory.path().join("unrelated");
        std::fs::write(&other, b"keep").unwrap();
        cleanup_transfers(directory.path()).unwrap();
        assert!(!interrupted.exists());
        assert!(other.exists());
    }

    #[tokio::test]
    async fn cancelled_conversion_retains_directory_until_worker_finishes_then_removes_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().to_path_buf();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let transfer = tokio::spawn(async move {
            with_transfer_directory(&path, |temp| async move {
                tokio::task::spawn_blocking(move || {
                    std::fs::write(temp.path().join("download"), b"audio").unwrap();
                    started.send(()).unwrap();
                    wait.recv().unwrap();
                    std::fs::write(temp.path().join("audio.m4a"), b"output").unwrap();
                    drop(temp);
                    finished.send(()).unwrap();
                })
                .await?;
                Ok(())
            })
            .await
        });
        ready.await.unwrap();
        transfer.abort();
        assert!(transfer.await.unwrap_err().is_cancelled());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        release.send(()).unwrap();
        done.await.unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn missing_date_never_overwrites_the_published_feed() {
        let mock = MockStorage::new(false);
        let episode = Episode {
            video: crate::youtube::Video {
                id: "FAaMG_3Lwug".into(),
                title: "Showdown".into(),
                description: String::new(),
                published: None,
                duration: 300.0,
                available: true,
            },
            present: true,
            state: "uploaded".into(),
            bytes: 123,
            public_url: Some("https://audio.example.com/existing.m4a".into()),
            position: 0,
        };
        assert!(
            publish(&source("playlist"), &[episode], &mock.storage, None)
                .await
                .is_err()
        );
        assert!(mock.uploads.lock().is_empty());
    }

    #[tokio::test]
    async fn publishes_cover_before_rss_with_public_artwork_urls() {
        let mock = MockStorage::new(false);
        for (kind, bytes, mime, extension) in [
            (
                "playlist",
                b"\xff\xd8\xff\xe0".to_vec(),
                "image/jpeg",
                "jpg",
            ),
            ("channel", b"\x89PNG\r\n\x1a\n".to_vec(), "image/png", "png"),
        ] {
            let source = source(kind);
            let cover = Cover {
                bytes,
                mime,
                extension,
            };
            let feed_url = publish(&source, &[], &mock.storage, Some(&cover))
                .await
                .unwrap();
            assert_eq!(
                feed_url,
                format!("https://audio.example.com/public/library/{kind}-test/rss.xml")
            );
            let uploads = mock.uploads.lock();
            let image = &uploads[uploads.len() - 2];
            let rss = &uploads[uploads.len() - 1];
            assert_eq!(
                image.path(),
                format!("/podcasts/library/{kind}-test/cover.{extension}")
            );
            assert!(
                image
                    .headers
                    .to_ascii_lowercase()
                    .contains(&format!("content-type: {mime}\r\n"))
            );
            assert_eq!(image.body, cover.bytes);
            assert_eq!(rss.path(), format!("/podcasts/library/{kind}-test/rss.xml"));
            assert!(
                rss.headers
                    .to_ascii_lowercase()
                    .contains("content-type: application/xml; charset=utf-8\r\n")
            );
            let feed = std::str::from_utf8(&rss.body).unwrap();
            let cover_url = format!(
                "https://audio.example.com/public/library/{kind}-test/cover.{extension}?v={:x}",
                Sha256::digest(&cover.bytes)
            );
            assert!(feed.contains(&format!("<image><url>{cover_url}</url><title>Arts &amp; &lt;Crafts&gt;</title><link>https://www.youtube.com/playlist?list=test&amp;view=1</link></image>")));
            assert!(feed.contains(&format!("<itunes:image href=\"{cover_url}\"/>")));
        }
    }

    #[test]
    fn cover_cache_survives_restart_and_tracks_replacements_and_removal() {
        let directory = tempfile::tempdir().unwrap();
        let source = source("playlist");
        let sources = std::slice::from_ref(&source);
        assert!(load_covers(directory.path(), sources).is_empty());
        for bytes in [b"\xff\xd8\xff\xe0".to_vec(), b"\x89PNG\r\n\x1a\n".to_vec()] {
            let cover = Cover::from_bytes(bytes).unwrap();
            cache_cover(directory.path(), &source, Some(&cover)).unwrap();
            let restored = load_covers(directory.path(), sources);
            assert_eq!(restored[&source.id].bytes, cover.bytes);
            assert_eq!(restored[&source.id].mime, cover.mime);
        }
        cache_cover(directory.path(), &source, None).unwrap();
        assert!(load_covers(directory.path(), sources).is_empty());
        cache_cover(directory.path(), &source, None).unwrap();
    }

    #[tokio::test]
    async fn failed_cover_upload_does_not_publish_rss() {
        let mock = MockStorage::new(true);
        let cover = Cover {
            bytes: b"\xff\xd8\xff\xe0".to_vec(),
            mime: "image/jpeg",
            extension: "jpg",
        };
        assert!(
            publish(&source("playlist"), &[], &mock.storage, Some(&cover))
                .await
                .is_err()
        );
        let uploads = mock.uploads.lock();
        assert_eq!(uploads.len(), 3);
        assert!(
            uploads
                .iter()
                .all(|upload| upload.path() == "/podcasts/library/playlist-test/cover.jpg")
        );
    }

    #[tokio::test]
    async fn source_without_cover_publishes_rss_without_artwork_tags() {
        let mock = MockStorage::new(false);
        publish(&source("playlist"), &[], &mock.storage, None)
            .await
            .unwrap();
        let uploads = mock.uploads.lock();
        assert_eq!(uploads.len(), 1);
        let feed = std::str::from_utf8(&uploads[0].body).unwrap();
        assert!(!feed.contains("<image>"));
        assert!(!feed.contains("<itunes:image"));
    }
}
