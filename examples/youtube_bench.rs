//! Shared harness, copied unchanged to the baseline checkout by the Bun runner.
//! Calls the application's public YouTube API; never downloads media.
use anyhow::{Context, Result, ensure};
use mazit::{
    network,
    youtube::{MediaRequest, Snapshot, YouTube},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

enum Output {
    Playlist(Snapshot),
    Media(Vec<Option<MediaRequest>>),
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).context("playlist|media")?;
    let id = args.get(2).context("playlist or video ID")?;
    let samples: usize = args.get(3).context("sample count")?.parse()?;
    let concurrency: usize = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1);
    ensure!(
        (1..=100).contains(&samples) && (1..=16).contains(&concurrency),
        "invalid sample/concurrency count"
    );
    ensure!(mode == "playlist" || mode == "media", "unknown workload");
    ensure!(
        mode != "playlist" || concurrency == 1,
        "baseline continuation state does not support concurrent scans of the same playlist"
    );
    let (mut client, jar) = network::client(None)?;
    let fixture = std::env::var_os("MAZIT_BENCH_CA").is_some();
    if let Ok(certificate) = std::env::var("MAZIT_BENCH_CA") {
        let address: std::net::SocketAddr = std::env::var("MAZIT_BENCH_ADDRESS")?.parse()?;
        ensure!(address.ip().is_loopback(), "fixture server must be local");
        client = reqwest::Client::builder()
            .no_proxy()
            .cookie_provider(jar.clone())
            .user_agent("Mozilla/5.0")
            .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(
                certificate,
            )?)?)
            .resolve("www.youtube.com", address)
            .resolve("youtubei.googleapis.com", address)
            .timeout(Duration::from_secs(30))
            .build()?;
    }
    let cold_start = Instant::now();
    let youtube = YouTube::start(client, Arc::new(move || network::youtube_cookie(&jar)));
    let mut results = Vec::new();
    for sample in 0..=samples {
        let start = if sample == 0 {
            cold_start
        } else {
            Instant::now()
        };
        let output = if mode == "playlist" {
            Output::Playlist(youtube.snapshot("playlist", id).await?)
        } else {
            let requests = (0..concurrency).map(|_| youtube.media(id));
            Output::Media(futures::future::try_join_all(requests).await?)
        };
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Validation/serialization is outside the timed operation.
        let mut ids_digest = None;
        let mut field_digests = None;
        let values: Vec<Value> = match output {
            Output::Playlist(snapshot) => {
                let ids: Vec<_> = snapshot.videos.iter().map(|video| &video.id).collect();
                ids_digest = Some(hash(&json!(ids))?);
                field_digests = Some(json!({
                    "title": hash(&json!(snapshot.title))?,
                    "description": hash(&json!(snapshot.description))?,
                    "cover": hash(&json!(snapshot.cover_url))?,
                    "published": hash(&json!(snapshot.videos.iter().map(|v| &v.published).collect::<Vec<_>>()))?,
                }));
                let videos: Vec<_> = snapshot
                    .videos
                    .into_iter()
                    .map(|video| {
                        if fixture {
                            json!(video)
                        } else {
                            // Live age labels depend on the observation time.
                            json!({"id":video.id,"title":video.title,"duration":video.duration,
                            "available":video.available,"has_date":video.published.is_some()})
                        }
                    })
                    .collect();
                vec![json!({
                    "title":snapshot.title, "description":snapshot.description,
                    "cover_url": if fixture { snapshot.cover_url } else { None }, "videos":videos
                })]
            }
            Output::Media(media) => media
                .into_iter()
                .map(|media| {
                    let media = media.context("video was deferred instead of resolved")?;
                    ensure!(!media.url.is_empty(), "missing audio URL");
                    // CPN, signed URLs and visitor data are deliberately excluded.
                    Ok(json!({"title":media.title, "description":media.description,
                    "duration":media.duration, "published":media.published,
                    "bytes":media.bytes, "itag":media.itag, "mime_type":media.mime_type,
                    "bitrate":media.bitrate, "user_agent":media.user_agent}))
                })
                .collect::<Result<_>>()?,
        };
        let count = if mode == "playlist" {
            values[0]["videos"].as_array().unwrap().len()
        } else {
            values.len()
        };
        let digest = hash(&json!(values))?;
        results.push(
            json!({"cold":sample == 0, "ms":elapsed_ms, "count":count, "digest":digest,
            "ids_digest":ids_digest,"field_digests":field_digests}),
        );
    }
    println!(
        "{}",
        json!({"mode":mode,"concurrency":concurrency,"samples":results})
    );
    Ok(())
}

fn hash(value: &Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
