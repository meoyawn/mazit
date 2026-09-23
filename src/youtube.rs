use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::{Engine, prelude::BASE64_STANDARD};
use llrt_modules::module_builder::ModuleBuilder;
use reqwest::Client;
use rquickjs::{
    AsyncContext, AsyncRuntime, Function, Module, Promise, async_with,
    function::{Async, Func},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

mod dates;

#[derive(Clone)]
pub struct YouTube {
    sender: mpsc::Sender<Request>,
}
struct Request {
    method: String,
    args: Value,
    result: oneshot::Sender<Result<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub published: Option<String>,
    pub duration: f64,
    pub available: bool,
}

#[derive(Deserialize)]
struct ListingVideo {
    #[serde(flatten)]
    video: Video,
    published_text: Option<String>,
}

pub fn parse_publication_date(date: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(date)
        .ok()
        .map(|date| date.to_utc())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc())
        })
}
#[derive(Deserialize)]
pub struct MediaRequest {
    pub url: String,
    pub bytes: u64,
    pub itag: u32,
    pub mime_type: String,
    pub bitrate: u64,
    pub user_agent: String,
    pub title: String,
    pub description: String,
    pub duration: f64,
    pub published: Option<String>,
}
pub struct Snapshot {
    pub title: String,
    pub description: String,
    pub cover_url: Option<String>,
    pub videos: Vec<Video>,
}

impl YouTube {
    #[cfg(test)]
    pub(crate) fn with_responses(responses: Vec<(&'static str, Value, Value)>) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Request>(32);
        tokio::spawn(async move {
            for (method, args, value) in responses {
                let request = receiver.recv().await.unwrap();
                assert_eq!(request.method, method);
                assert_eq!(request.args, args);
                request.result.send(Ok(value)).unwrap();
            }
        });
        Self { sender }
    }

    pub fn start(client: Client, cookie_header: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Request>(32);
        std::thread::Builder::new().name("youtube-quickjs".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("QuickJS executor");
            runtime.block_on(async move {
                let setup = async {
                    let runtime = AsyncRuntime::new().context("Create QuickJS")?;
                    runtime.set_memory_limit(768 * 1024 * 1024).await;
                    runtime.set_max_stack_size(4 * 1024 * 1024).await;
                    let (resolver, loader, globals) = ModuleBuilder::default().build();
                    runtime.set_loader(resolver, loader).await;
                    let context = AsyncContext::full(&runtime).await.context("QuickJS context")?;
                    context.with(|ctx| {
                        use llrt_utils::primordials::Primordial;
                        llrt_utils::primordials::BasePrimordials::init(&ctx)?;
                        globals.attach(&ctx)?;
                        ctx.globals().set("structuredClone", Func::from(clone_value))?;
                        ctx.globals().set("hostCookie", Func::from(move || cookie_header()))?;
                        ctx.globals().set("hostFetch", Func::from(Async(move |request: String| {
                            let client = client.clone();
                            async move { match fetch_metadata(client, request).await { Ok(value) => value.to_string(), Err(error) => json!({"error": crate::redact(&error.to_string())}).to_string() } }
                        })))?;
                        ctx.eval::<(), _>("var console = {log(){},info(){},warn(){},error(){},debug(){}};")?;
                        load_bridge(&ctx)
                            .inspect_err(|_| { log::error!("QuickJS initialization: {:?}", ctx.catch()); })
                    }).await.context("Load embedded YouTube.js")?;
                    Ok::<_, anyhow::Error>((runtime, context))
                }.await;
                match setup {
                    Ok((_runtime, context)) => while let Some(request) = receiver.recv().await {
                        let result = async_with!(context => |ctx| {
                            let bridge: rquickjs::Object = ctx.globals().get("YouTubeBridge")?;
                            let function: Function = bridge.get("call")?;
                            let promise: Promise = function.call((request.method.as_str(), request.args.to_string()))?;
                            match promise.into_future::<String>().await {
                                Ok(json) => Ok(json),
                                Err(error) => {
                                    let exception = ctx.catch();
                                    let message = exception.as_exception().and_then(|e| e.message()).unwrap_or_else(|| error.to_string());
                                    Err(rquickjs::Error::new_from_js_message("YouTube", "Rust", crate::redact(&message)))
                                }
                            }
                        }).await.map_err(|error| anyhow!(error.to_string())).and_then(|text| Ok(serde_json::from_str(&text)?));
                        let result = result.and_then(|value: Value| {
                            if let Some(message) = value["bridgeError"].as_str() {
                                return Err(anyhow!(crate::redact(message)));
                            }
                            Ok(value)
                        });
                        let _ = request.result.send(result);
                    },
                    Err(error) => {
                        log::error!("YouTube worker initialization failed: {error:#}");
                        while let Some(request) = receiver.recv().await { let _ = request.result.send(Err(anyhow!(error.to_string()))); }
                    },
                }
            });
        }).expect("Start QuickJS thread");
        Self { sender }
    }

    pub async fn call(&self, method: &str, args: Value) -> Result<Value> {
        let (send, recv) = oneshot::channel();
        self.sender
            .send(Request {
                method: method.into(),
                args,
                result: send,
            })
            .await?;
        tokio::time::timeout(Duration::from_secs(180), recv)
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
        let result = self.call("resolve", json!({"url": url.as_str()})).await?;
        let id = result["id"].as_str().context("Channel not found")?;
        ensure!(
            id.starts_with("UC") && id.len() == 24 && valid_id(id),
            "Not a channel URL"
        );
        Ok((
            "channel".into(),
            id.into(),
            format!("https://www.youtube.com/channel/{id}"),
        ))
    }

    pub async fn snapshot(&self, kind: &str, id: &str) -> Result<Snapshot> {
        // Use one reference time for all continuation pages in this scan.
        let observed_at = chrono::Utc::now();
        let playlist = if kind == "channel" {
            format!("UU{}", &id[2..])
        } else {
            id.into()
        };
        let mut videos = Vec::new();
        let mut ids = HashSet::new();
        let mut pages = HashSet::new();
        let mut count = 0;
        let mut expected = None;
        let mut title = String::new();
        let mut description = String::new();
        let mut cover_url = None;
        loop {
            let page = self
                .call(
                    "page",
                    json!({"id": playlist, "continuation": !pages.is_empty()}),
                )
                .await?;
            ensure!(
                page["suspicious"] != true,
                "Incomplete YouTube listing; previous feed and files preserved"
            );
            if pages.is_empty() {
                title = page["title"].as_str().unwrap_or(id).into();
                description = page["description"].as_str().unwrap_or("").into();
                cover_url = page["cover_url"].as_str().map(str::to_owned);
                expected = page["count"]
                    .as_str()
                    .and_then(|text| text.split_whitespace().next())
                    .and_then(|text| text.replace(',', "").parse::<usize>().ok());
            }
            let entries: Vec<ListingVideo> = serde_json::from_value(page["videos"].clone())?;
            let fingerprint = entries
                .iter()
                .map(|v| v.video.id.as_str())
                .collect::<Vec<_>>()
                .join(",");
            ensure!(
                pages.insert(fingerprint) && pages.len() < 10_000,
                "Repeated YouTube pagination; no files removed"
            );
            count += entries.len();
            for entry in entries {
                let mut video = entry.video;
                video.published = video
                    .published
                    .as_deref()
                    .and_then(parse_publication_date)
                    .or_else(|| {
                        dates::parse_listing_date(entry.published_text.as_deref()?, observed_at)
                    })
                    .map(|date| date.to_rfc3339());
                ensure!(
                    video.id.len() == 11 && valid_id(&video.id),
                    "Invalid video identifier"
                );
                if ids.insert(video.id.clone()) {
                    videos.push(video);
                }
            }
            if page["continuation"] != true {
                break;
            }
        }
        ensure!(
            expected == Some(count),
            "Incomplete YouTube listing ({count} entries); previous feed and files preserved"
        );
        if kind == "channel" {
            let channel = self.call("channel", json!({"id": id})).await?;
            title = channel["title"].as_str().unwrap_or(&title).into();
            description = channel["description"].as_str().unwrap_or("").into();
            cover_url = channel["cover_url"].as_str().map(str::to_owned);
        }
        Ok(Snapshot {
            title,
            description,
            cover_url,
            videos,
        })
    }
    pub async fn media(&self, id: &str) -> Result<MediaRequest> {
        let mut media: MediaRequest = serde_json::from_value(
            self.call("media", json!({"id": id, "client": "VISIONOS"}))
                .await?,
        )?;
        media.published = media
            .published
            .as_deref()
            .and_then(parse_publication_date)
            .map(|date| date.to_rfc3339());
        Ok(media)
    }
}

fn load_bridge<'js>(ctx: &rquickjs::Ctx<'js>) -> rquickjs::Result<()> {
    let module = Module::declare(
        ctx.clone(),
        "youtube",
        include_str!("../generated/youtube.js"),
    )?;
    let (module, evaluated) = module.eval()?;
    evaluated.finish::<()>()?;
    ctx.globals().set("YouTubeBridge", module.namespace()?)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-')
}

fn clone_value<'js>(
    ctx: rquickjs::Ctx<'js>,
    value: rquickjs::Value<'js>,
    options: rquickjs::function::Opt<rquickjs::Object<'js>>,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    llrt_utils::clone::structured_clone(&ctx, value, options)
}

#[derive(Deserialize)]
struct Fetch {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}
async fn fetch_metadata(client: Client, json: String) -> Result<Value> {
    let input: Fetch = serde_json::from_str(&json)?;
    let url = url::Url::parse(&input.url)?;
    let host = url.host_str().unwrap_or("");
    ensure!(
        url.scheme() == "https"
            && [
                "youtube.com",
                "google.com",
                "googleapis.com",
                "googlevideo.com",
                "ytimg.com"
            ]
            .iter()
            .any(|base| host == *base || host.ends_with(&format!(".{base}"))),
        "Unexpected YouTube API host"
    );
    let mut request = client.request(input.method.parse()?, url);
    for (key, value) in input.headers {
        if !["host", "content-length", "cookie"].contains(&key.to_ascii_lowercase().as_str()) {
            request = request.header(key, value);
        }
    }
    if let Some(body) = input.body {
        request = request.body(BASE64_STANDARD.decode(body)?);
    }
    let response = request.send().await.map_err(|e| e.without_url())?;
    let status = response.status().as_u16();
    let headers: HashMap<_, _> = response
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_string(), v.to_string()))
        })
        .collect();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
        if body.len() > 32 * 1024 * 1024 {
            bail!("YouTube metadata response exceeded size limit")
        }
    }
    Ok(json!({"status": status, "headers": headers, "body": BASE64_STANDARD.encode(body)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn playlist_cover_survives_continuation_pages() {
        let youtube = YouTube::with_responses(vec![
            (
                "page",
                json!({"id": "PLtest", "continuation": false}),
                json!({
                    "title": "Playlist", "description": "Description", "count": "2 videos",
                    "cover_url": "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg",
                    "continuation": true,
                    "videos": [{"id": "abcdefghijk", "title": "First", "duration": 10, "available": true}]
                }),
            ),
            (
                "page",
                json!({"id": "PLtest", "continuation": true}),
                json!({
                    "cover_url": null, "continuation": false,
                    "videos": [{"id": "lmnopqrstuv", "title": "Second", "duration": 20, "available": true}]
                }),
            ),
        ]);
        let snapshot = youtube.snapshot("playlist", "PLtest").await.unwrap();
        assert_eq!(
            snapshot.cover_url.as_deref(),
            Some("https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg")
        );
        assert_eq!(snapshot.videos.len(), 2);
    }

    #[tokio::test]
    async fn channel_cover_replaces_uploads_playlist_cover() {
        for cover_url in [Some("https://yt3.googleusercontent.com/avatar"), None] {
            let youtube = YouTube::with_responses(vec![
                (
                    "page",
                    json!({"id": "UUtest", "continuation": false}),
                    json!({
                        "title": "Uploads", "count": "0 videos", "videos": [],
                        "cover_url": "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg"
                    }),
                ),
                (
                    "channel",
                    json!({"id": "UCtest"}),
                    json!({
                        "title": "Channel", "description": "About", "cover_url": cover_url
                    }),
                ),
            ]);
            let snapshot = youtube.snapshot("channel", "UCtest").await.unwrap();
            assert_eq!(snapshot.title, "Channel");
            assert_eq!(snapshot.cover_url.as_deref(), cover_url);
        }
    }

    #[tokio::test]
    async fn three_thousand_videos_need_only_flat_pages_for_diffing() {
        // Only page responses are supplied: any per-video call fails this scan.
        for kind in ["playlist", "channel"] {
            let id = if kind == "playlist" {
                "PLtest"
            } else {
                "UCtest"
            };
            let playlist = if kind == "playlist" {
                "PLtest"
            } else {
                "UUtest"
            };
            let mut responses = (0..30)
                .map(|page| {
                    (
                        "page",
                        json!({"id": playlist, "continuation": page != 0}),
                        json!({
                            "title": "Large playlist", "count": "3,000 videos",
                            "continuation": page < 29,
                            "videos": (page * 100..(page + 1) * 100).map(|index| json!({
                                "id": format!("{index:011}"), "title": "Video", "duration": 0,
                                "available": true, "published_text": "15y ago"
                            })).collect::<Vec<_>>()
                        }),
                    )
                })
                .collect::<Vec<_>>();
            if kind == "channel" {
                responses.push(("channel", json!({"id": id}), json!({"title": "Channel"})));
            }
            let youtube = YouTube::with_responses(responses);
            let snapshot = youtube.snapshot(kind, id).await.unwrap();
            assert_eq!(snapshot.videos.len(), 3000);
            let first = snapshot.videos[0].published.as_deref().unwrap();
            assert!(parse_publication_date(first).is_some());
            for (index, video) in snapshot.videos.iter().enumerate() {
                assert_eq!(video.id, format!("{index:011}"));
                assert_eq!(video.published.as_deref(), Some(first));
            }
        }
    }

    #[tokio::test]
    async fn missing_listing_dates_do_not_trigger_video_lookups() {
        let youtube = YouTube::with_responses(vec![(
            "page",
            json!({"id": "PLtest", "continuation": false}),
            json!({
                "count": "2 videos", "videos": [
                    {"id": "abcdefghijk", "title": "First", "duration": 0, "available": true},
                    {"id": "lmnopqrstuv", "title": "Second", "duration": 0, "available": true, "published_text": "50K views"}
                ]
            }),
        )]);
        let snapshot = youtube.snapshot("playlist", "PLtest").await.unwrap();
        assert!(
            snapshot
                .videos
                .iter()
                .all(|video| video.published.is_none())
        );
    }

    #[tokio::test]
    async fn unavailable_placeholders_count_toward_a_complete_listing() {
        let youtube = YouTube::with_responses(vec![
            (
                "page",
                json!({"id": "PLtest", "continuation": false}),
                json!({
                    "title": "Interviews", "count": "3 episodes", "continuation": true,
                    "suspicious": false,
                    "videos": [{"id": "abcdefghijk", "title": "First", "duration": 0, "available": true}]
                }),
            ),
            (
                "page",
                json!({"id": "PLtest", "continuation": true}),
                json!({
                    "continuation": false, "suspicious": false,
                    "videos": [
                        {"id": "T6juU_4UqKI", "title": "T6juU_4UqKI", "duration": 0, "available": false},
                        {"id": "lmnopqrstuv", "title": "Last", "duration": 0, "available": true}
                    ]
                }),
            ),
        ]);
        let snapshot = youtube.snapshot("playlist", "PLtest").await.unwrap();
        assert_eq!(snapshot.videos.len(), 3);
        assert!(!snapshot.videos[1].available);
        assert_eq!(
            snapshot
                .videos
                .iter()
                .filter(|video| video.available)
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn incomplete_or_suspicious_listings_are_still_rejected() {
        for (count, suspicious) in [
            (Some("2 episodes"), false),
            (None, false),
            (Some("1 episode"), true),
        ] {
            let youtube = YouTube::with_responses(vec![(
                "page",
                json!({"id": "PLtest", "continuation": false}),
                json!({
                    "count": count, "suspicious": suspicious,
                    "videos": [{"id": "abcdefghijk", "title": "First", "duration": 0, "available": true}]
                }),
            )]);
            let error = youtube.snapshot("playlist", "PLtest").await.err().unwrap();
            assert!(error.to_string().contains("Incomplete YouTube listing"));
        }
    }

    #[tokio::test]
    async fn audio_resolution_keeps_visionos_and_does_not_require_web_metadata() {
        let youtube = YouTube::with_responses(vec![(
            "media",
            json!({"id": "FAaMG_3Lwug", "client": "VISIONOS"}),
            json!({
                "url": "https://example.googlevideo.com/audio", "bytes": 12,
                "itag": 140, "mime_type": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 128000,
                "user_agent": "VISIONOS", "title": "Showdown", "description": "",
                "duration": 300, "published": null
            }),
        )]);
        let media = youtube.media("FAaMG_3Lwug").await.unwrap();
        assert!(media.published.is_none());
        assert_eq!(media.user_agent, "VISIONOS");
    }

    #[tokio::test]
    async fn bun_bundle_initializes_in_quickjs() {
        let runtime = AsyncRuntime::new().unwrap();
        runtime.set_max_stack_size(4 * 1024 * 1024).await;
        let (resolver, loader, globals) = ModuleBuilder::default().build();
        runtime.set_loader(resolver, loader).await;
        let context = AsyncContext::full(&runtime).await.unwrap();
        context
            .with(|ctx| {
                use llrt_utils::primordials::Primordial;
                llrt_utils::primordials::BasePrimordials::init(&ctx)?;
                globals.attach(&ctx)?;
                load_bridge(&ctx)?;
                let bridge: rquickjs::Object = ctx.globals().get("YouTubeBridge")?;
                let _: Function = bridge.get("call")?;
                Ok::<_, rquickjs::Error>(())
            })
            .await
            .unwrap();
    }
}
