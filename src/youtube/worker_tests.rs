use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use youtubei::FetchResponse;

fn video(id: &str) -> Value {
    json!({"lockupViewModel":{"contentId":id,"contentType":"LOCKUP_CONTENT_TYPE_VIDEO",
        "metadata":{"lockupMetadataViewModel":{"title":{"content":id}}}}})
}

fn first_page(index: usize) -> Value {
    json!({"metadata":{"playlistMetadataRenderer":{"title":format!("Scan {index}")}},
    "sidebar":{"playlistSidebarRenderer":{"items":[{"playlistSidebarPrimaryInfoRenderer":{"stats":[{"simpleText":"2 videos"}]}}]}},
    "contents":{"playlistVideoListRenderer":{"contents":[video(&format!("s{index:010}")),
        {"continuationItemRenderer":{"continuationEndpoint":{
            "commandMetadata":{"webCommandMetadata":{"apiUrl":"/youtubei/v1/browse","sendPost":true}},
            "continuationCommand":{"token":index.to_string(),"request":"CONTINUATION_REQUEST_TYPE_BROWSE"}
        }}}
    ]}}})
}

fn session_response() -> Vec<u8> {
    let mut device = vec![Value::Null; 108];
    for (index, value) in [
        (0, "en"),
        (1, "US"),
        (13, "fixture-visitor"),
        (16, "fixture-client"),
        (79, "UTC"),
    ] {
        device[index] = json!(value);
    }
    device[61] = json!(["fixture-install"]);
    format!(
        ")]}}'{}",
        json!([[null, null, [[device], "fixture-api-key"]]])
    )
    .into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_worker_reuses_session_and_keeps_concurrent_scans_independent() {
    let started = Arc::new(AtomicUsize::new(0));
    let sessions = Arc::new(AtomicUsize::new(0));
    let configurations = Arc::new(AtomicUsize::new(0));
    let cookie_reads = Arc::new(AtomicUsize::new(0));
    let counts = (
        started.clone(),
        sessions.clone(),
        configurations.clone(),
        cookie_reads.clone(),
    );
    let youtube = YouTube::with_worker(move || async move {
        let (started, sessions, configurations, cookie_reads) = counts;
        started.fetch_add(1, Ordering::SeqCst);
        let mut state = State::new(
            Client::new(),
            Arc::new(move || {
                cookie_reads.fetch_add(1, Ordering::SeqCst);
                String::new()
            }),
        )
        .await?;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let sequence = AtomicUsize::new(0);
        state.fetch = state.engine.fetch_with(move |request| {
            let sessions = sessions.clone();
            let configurations = configurations.clone();
            let barrier = barrier.clone();
            let body: Value = request.body.as_deref().map(serde_json::from_slice).transpose().unwrap().unwrap_or(Value::Null);
            let index = body.get("browseId").map(|_| sequence.fetch_add(1, Ordering::SeqCst));
            async move {
                let data = if request.url.contains("/sw.js_data") {
                    sessions.fetch_add(1, Ordering::SeqCst);
                    return Ok(FetchResponse { status: 200, headers: vec![], body: session_response() });
                } else if request.url.contains("/config?") {
                    configurations.fetch_add(1, Ordering::SeqCst);
                    json!({})
                } else if request.url.contains("/browse?") {
                    if let Some(index) = index {
                        assert_eq!(body["params"], "wgYCCAA=");
                        barrier.wait().await;
                        first_page(index)
                    } else {
                        let index: usize = body["continuation"].as_str().unwrap().parse().unwrap();
                        json!({"onResponseReceivedActions":[{"appendContinuationItemsAction":{"continuationItems":[video(&format!("e{index:010}"))]}}]})
                    }
                } else {
                    assert!(request.url.contains("/player?"));
                    assert_eq!(body["context"]["client"]["clientName"], "VISIONOS");
                    match body["videoId"].as_str().unwrap() {
                        "failure0000" => json!({"videoDetails":{},"playabilityStatus":{"status":"UNPLAYABLE","reason":"Fixture unavailable"}}),
                        "upcoming000" => json!({"videoDetails":{"isUpcoming":true},"playabilityStatus":{"status":"LIVE_STREAM_OFFLINE"}}),
                        _ => json!({"videoDetails":{"title":"Audio","lengthSeconds":"1"},"playabilityStatus":{"status":"OK"},
                            "streamingData":{"expiresInSeconds":"3600","formats":[],"adaptiveFormats":[{
                                "itag":140,"mimeType":"audio/mp4; codecs=\"mp4a.40.2\"","bitrate":128000,"audioQuality":"AUDIO_QUALITY_MEDIUM",
                                "contentLength":"12345","approxDurationMs":"1000","lastModified":"0","url":"https://example.googlevideo.com/audio"
                            }]}}),
                    }
                };
                Ok(FetchResponse { status: 200, headers: vec![], body: data.to_string().into_bytes() })
            }
        }).await?;
        Ok(state)
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        let first = youtube.clone();
        let second = youtube.clone();
        let audio = youtube.clone();
        let (one, two, media) = tokio::join!(
            tokio::spawn(async move { first.snapshot("playlist", "PLsame").await }),
            tokio::spawn(async move { second.snapshot("playlist", "PLsame").await }),
            tokio::spawn(async move { audio.media("abcdefghijk").await }),
        );
        let mut scans = Vec::new();
        for snapshot in [one.unwrap().unwrap(), two.unwrap().unwrap()] {
            let index: usize = snapshot
                .title
                .strip_prefix("Scan ")
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                snapshot
                    .videos
                    .iter()
                    .map(|v| v.id.clone())
                    .collect::<Vec<_>>(),
                [format!("s{index:010}"), format!("e{index:010}")]
            );
            scans.push(index);
        }
        scans.sort();
        assert_eq!(scans, [0, 1]);
        assert_eq!(media.unwrap().unwrap().unwrap().bytes, 12345);
        assert!(
            youtube
                .media("failure0000")
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("Fixture unavailable")
        );
        assert!(youtube.media("upcoming000").await.unwrap().is_none());
        assert_eq!(
            youtube.media("abcdefghijk").await.unwrap().unwrap().title,
            "Audio"
        );
    })
    .await
    .expect("worker must overlap requests and survive API errors");
    for count in [started, sessions, configurations, cookie_reads] {
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "engine/session must be reused"
        );
    }
}
