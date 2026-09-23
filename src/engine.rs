use crate::{
    config,
    database::{Database, Episode, Source},
    storage::Storage,
    youtube::YouTube,
};
use anyhow::{Context, Result, ensure};
use futures::{StreamExt, stream};
use parking_lot::RwLock;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ViewState {
    pub sources: Vec<Source>,
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
    sender: mpsc::UnboundedSender<Command>,
}

pub struct Core {
    pub db: Database,
    pub youtube: YouTube,
    pub client: reqwest::Client,
    pub directory: PathBuf,
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
        std::fs::create_dir_all(&transfers)?;
        // Owned temporary directories are removed on drop; leftovers are safe to clean on next launch.
        for entry in std::fs::read_dir(&transfers)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with("mazit-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let db = Database::open(&directory.join("mazit.sqlite"))?;
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
            _lock: lock,
        })
    }
    pub async fn add(&self, url: &str) -> Result<String> {
        let (kind, id, url) = self.youtube.resolve(url).await?;
        log::info!("Adding subscription {kind}:{id}");
        self.db.add(&kind, &id, &url)
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
        self.db.phase(id, "downloading", None)?;
        changed();
        let pending: Vec<_> = self
            .db
            .episodes(id)?
            .into_iter()
            .filter(|e| e.present && e.video.available && e.state != "uploaded")
            .collect();
        log::info!("Transferring source={id} pending={}", pending.len());
        let source_ref = &source;
        let outcomes = stream::iter(pending)
            .map(|episode| async move {
                let outcome = retry(
                    &format!("Transfer source={id} video={}", episode.video.id),
                    || self.transfer(source_ref, &episode, storage),
                )
                .await;
                if let Err(error) = &outcome {
                    log::error!(
                        "Transfer failed source={id} video={}: {error:#}",
                        episode.video.id
                    );
                }
                changed();
                outcome
            })
            .buffer_unordered(5)
            .collect::<Vec<_>>()
            .await;
        for outcome in outcomes {
            outcome?;
        }
        self.db.phase(id, "publishing", None)?;
        log::info!("Publishing RSS source={id}");
        changed();
        let key = format!("{}/rss.xml", source.folder);
        let feed_url = storage.url(&key)?;
        let feed = crate::rss::render(&self.db.source(id)?, &self.db.episodes(id)?, &feed_url);
        // Use the generic XML MIME type so browsers display the feed in their XML viewer.
        retry(&format!("Publish RSS source={id}"), || {
            storage.put_text(&key, feed.clone(), "application/xml; charset=utf-8")
        })
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
    async fn transfer(&self, source: &Source, episode: &Episode, storage: &Storage) -> Result<()> {
        let started = Instant::now();
        let id = &episode.video.id;
        log::info!("Resolving audio source={} video={id}", source.id);
        let request = self.youtube.media(id).await.context("Resolve audio")?;
        let temp = tempfile::Builder::new()
            .prefix("mazit-")
            .tempdir_in(self.directory.join("transfers"))?;
        let input = temp.path().join("download");
        let output = temp.path().join("audio.m4a");
        log::info!("Downloading audio video={id} bytes={}", request.bytes);
        crate::network::download(&self.client, &request, &input)
            .await
            .context("Download audio")?;
        log::info!("Preparing M4A video={id}");
        let audio = output.clone();
        let bytes = tokio::task::spawn_blocking(move || crate::audio::prepare_m4a(&input, &audio))
            .await
            .context("Audio worker stopped")?
            .context("Prepare M4A")?;
        let key = format!("{}/{}.m4a", source.folder, episode.video.id);
        log::info!("Uploading audio video={id} bytes={bytes}");
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
    }
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
    pub fn start(core: Core, runtime: &tokio::runtime::Handle) -> Result<Self> {
        let state = Arc::new(RwLock::new(ViewState::default()));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let engine = Self {
            state: state.clone(),
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
                core.db.bind_storage(&settings.identity())?;
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
        runtime.spawn(async move {
            let changed = || {
                if let Ok(sources) = core.db.sources() {
                    state.write().sources = sources;
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
                            core.db.bind_storage(&settings.identity())?;
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
                        for id in ids {
                            if let Err(error) = core.sync(&id, storage, &changed).await {
                                state.write().message = crate::redact(&error.to_string());
                            }
                        }
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
