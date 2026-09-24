use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use futures::{StreamExt, stream::FuturesUnordered};
use reqwest::Client;
use tokio::sync::{OnceCell, mpsc, oneshot};
use youtubei::{Engine, FetchFunction, Innertube, Player, SessionOptions, UniversalCache};

use super::{
    MediaRequest, Snapshot, YouTube, channel, listing, media, parse_publication_date, transport,
    valid_id,
};

pub(super) enum Request {
    Resolve {
        url: String,
        reply: oneshot::Sender<Result<String>>,
    },
    Snapshot {
        kind: String,
        id: String,
        reply: oneshot::Sender<Result<Snapshot>>,
    },
    Media {
        id: String,
        reply: oneshot::Sender<Result<Option<MediaRequest>>>,
    },
}

struct State {
    engine: Engine,
    fetch: FetchFunction,
    client: Client,
    cookie: Arc<dyn Fn() -> String + Send + Sync>,
    yt: OnceCell<Innertube>,
    player: OnceCell<Player>,
}

impl State {
    async fn new(client: Client, cookie: Arc<dyn Fn() -> String + Send + Sync>) -> Result<Self> {
        let engine = Engine::new().await?;
        let transport = client.clone();
        let fetch = engine
            .fetch_with(move |request| {
                let client = transport.clone();
                async move {
                    transport::fetch(client, request)
                        .await
                        .map_err(|e| youtubei::Error::new(crate::redact(&e.to_string())))
                }
            })
            .await?;
        Ok(Self {
            engine,
            fetch,
            client,
            cookie,
            yt: OnceCell::new(),
            player: OnceCell::new(),
        })
    }

    async fn session(&self) -> Result<&Innertube> {
        self.yt
            .get_or_try_init(async || {
                let cache = UniversalCache::new(&self.engine, false, None).await?;
                let cookie = (self.cookie)();
                Ok(Innertube::create_in(
                    &self.engine,
                    SessionOptions {
                        lang: Some("en".into()),
                        location: Some("US".into()),
                        retrieve_player: Some(false),
                        cookie: (!cookie.is_empty()).then_some(cookie),
                        fetch: Some(self.fetch.clone()),
                        cache: Some(cache.as_cache()),
                        ..Default::default()
                    },
                )
                .await?)
            })
            .await
    }
}

async fn handle(state: &Result<State>, request: Request) {
    match request {
        Request::Resolve { url, reply } => {
            if reply.is_closed() {
                return;
            }
            let result = bounded(async {
                let state = state.as_ref().map_err(|e| anyhow!(e.to_string()))?;
                channel::resolve(&state.client, &url).await
            })
            .await;
            let _ = reply.send(result);
        }
        Request::Snapshot { kind, id, reply } => {
            if reply.is_closed() {
                return;
            }
            let result = async {
                let state = state.as_ref().map_err(|e| anyhow!(e.to_string()))?;
                let yt = bounded(state.session()).await?;
                let playlist = if kind == "channel" {
                    format!("UU{}", &id[2..])
                } else {
                    id.clone()
                };
                // Each scan owns its continuation chain, even for the same playlist.
                let mut source = listing::Live {
                    yt,
                    id: playlist,
                    previous: None,
                };
                listing::snapshot(&mut source, &kind, &id).await
            }
            .await
            .map_err(|e: anyhow::Error| anyhow!(crate::redact(&e.to_string())));
            let _ = reply.send(result);
        }
        Request::Media { id, reply } => {
            if reply.is_closed() {
                return;
            }
            let result = bounded(async {
                let state = state.as_ref().map_err(|e| anyhow!(e.to_string()))?;
                media::resolve(state.session().await?, &id, &state.player).await
            })
            .await;
            let _ = reply.send(result);
        }
    }
}

pub(super) async fn bounded<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(180), future)
        .await
        .context("YouTube operation timed out")?
        .map_err(|e| anyhow!(crate::redact(&e.to_string())))
}

impl YouTube {
    pub fn start(client: Client, cookie: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        Self::with_worker(move || State::new(client, cookie))
    }

    fn with_worker<F, Fut>(initialize: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<State>>,
    {
        let (sender, mut receiver) = mpsc::channel(32);
        std::thread::Builder::new()
            .name("youtube".into())
            .spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("YouTube executor")
                    .block_on(async move {
                        let state = initialize().await;
                        let mut active = FuturesUnordered::new();
                        loop {
                            tokio::select! {
                                request = receiver.recv(), if active.len() < 16 => {
                                    match request {
                                        Some(request) => active.push(handle(&state, request)),
                                        None => break,
                                    }
                                }
                                Some(()) = active.next(), if !active.is_empty() => {}
                            }
                        }
                        while active.next().await.is_some() {}
                    });
            })
            .expect("Start YouTube worker");
        Self { sender }
    }

    async fn receive<T>(
        &self,
        request: Request,
        receive: oneshot::Receiver<Result<T>>,
    ) -> Result<T> {
        let paginated = matches!(&request, Request::Snapshot { .. });
        self.sender.send(request).await?;
        // A large scan may exceed three minutes in total; each page is bounded.
        if paginated {
            return receive.await?;
        }
        tokio::time::timeout(Duration::from_secs(180), receive)
            .await
            .context("YouTube operation timed out")??
    }

    pub async fn resolve(&self, text: &str) -> Result<(String, String, String)> {
        let url = url::Url::parse(text.trim()).context("Enter a YouTube URL")?;
        ensure!(
            ["www.youtube.com", "youtube.com", "m.youtube.com"]
                .contains(&url.host_str().unwrap_or("")),
            "Enter a YouTube playlist or channel URL"
        );
        if let Some((_, id)) = url.query_pairs().find(|(key, _)| key == "list") {
            ensure!(
                id.starts_with("PL") || id.starts_with("UU") || id.starts_with("OLAK"),
                "Only saved playlists are supported"
            );
            ensure!(valid_id(&id), "Invalid playlist identifier");
            return Ok((
                "playlist".into(),
                id.to_string(),
                format!("https://www.youtube.com/playlist?list={id}"),
            ));
        }
        let (reply, receive) = oneshot::channel();
        let id = self
            .receive(
                Request::Resolve {
                    url: url.into(),
                    reply,
                },
                receive,
            )
            .await?;
        ensure!(
            id.starts_with("UC") && id.len() == 24 && valid_id(&id),
            "Not a channel URL"
        );
        Ok((
            "channel".into(),
            id.clone(),
            format!("https://www.youtube.com/channel/{id}"),
        ))
    }

    pub async fn snapshot(&self, kind: &str, id: &str) -> Result<Snapshot> {
        ensure!(
            valid_id(id) && (kind != "channel" || id.starts_with("UC") && id.len() > 2),
            "Invalid YouTube identifier"
        );
        let (reply, receive) = oneshot::channel();
        self.receive(
            Request::Snapshot {
                kind: kind.into(),
                id: id.into(),
                reply,
            },
            receive,
        )
        .await
    }

    pub async fn media(&self, id: &str) -> Result<Option<MediaRequest>> {
        let (reply, receive) = oneshot::channel();
        let Some(mut media) = self
            .receive(
                Request::Media {
                    id: id.into(),
                    reply,
                },
                receive,
            )
            .await?
        else {
            return Ok(None);
        };
        media.published = media
            .published
            .as_deref()
            .and_then(parse_publication_date)
            .map(|date| date.to_rfc3339());
        Ok(Some(media))
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
