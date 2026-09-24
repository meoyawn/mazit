use super::{channel, listing, media};
use serde_json::{Value, json};
use std::{collections::VecDeque, sync::Arc};
use youtubei::{
    Engine, FetchRequest, FetchResponse, Format, Innertube, SessionOptions, VideoInfo, YTNode,
};

async fn client(responses: Vec<Value>) -> (Innertube, Arc<parking_lot::Mutex<Vec<FetchRequest>>>) {
    let engine = Engine::new().await.unwrap();
    let requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let responses = parking_lot::Mutex::new(VecDeque::from(responses));
    let fetch = engine
        .fetch_with(move |request| {
            captured.lock().push(request);
            let response = responses.lock().pop_front().expect("unexpected request");
            async move {
                Ok(FetchResponse {
                    status: 200,
                    headers: vec![],
                    body: response.to_string().into_bytes(),
                })
            }
        })
        .await
        .unwrap();
    let yt = Innertube::create_in(
        &engine,
        SessionOptions {
            fetch: Some(fetch),
            ..SessionOptions::local()
        },
    )
    .await
    .unwrap();
    (yt, requests)
}

#[test]
fn channel_html_resolution_preserves_canonical_and_consent_rules() {
    let id = "UCCsdwE2z_kRL3vfVsd5OGyA";
    for url in [
        "https://www.youtube.com/@RyanFleury/videos",
        "https://www.youtube.com/@RyanFleury/",
        "https://www.youtube.com/channel/UCCsdwE2z_kRL3vfVsd5OGyA",
        "https://www.youtube.com/c/RyanFleury/videos",
        "https://www.youtube.com/user/RyanFleury/",
    ] {
        assert_eq!(channel::page_url(url).unwrap().as_str(), url);
    }
    assert_eq!(
        channel::page_url("http://m.youtube.com/@RyanFleury/")
            .unwrap()
            .as_str(),
        "https://www.youtube.com/@RyanFleury/"
    );
    for url in [
        "https://www.youtube.com/watch?v=abcdefghijk",
        "https://www.youtube.com/playlist?list=PLexample",
        "https://www.youtube.com/",
        "https://www.youtube.com/@/",
        "https://example.com/@RyanFleury/",
        "ftp://www.youtube.com/@RyanFleury/",
    ] {
        assert!(channel::page_url(url).is_err());
    }
    for html in [
        format!(
            r#"<a href="/channel/UCabcdefghijklmnopqrstuv">Other</a><link rel="canonical" href="https://www.youtube.com/channel/{id}">"#
        ),
        format!("<link href = 'https://www.youtube.com/channel/{id}/'\nrel = 'canonical' />"),
    ] {
        assert_eq!(channel::from_html(&html).unwrap(), id);
    }
    for html in [
        "<html>no channel</html>",
        "<a href='https://www.youtube.com/channel/UCCsdwE2z_kRL3vfVsd5OGyA'>Other</a>",
        "<link rel='alternate' href='https://www.youtube.com/channel/UCCsdwE2z_kRL3vfVsd5OGyA'>",
        "<link rel='canonical' href='https://www.youtube.com/watch?v=abcdefghijk'>",
        "<link rel='canonical' href='https://www.youtube.com/channel/UCtoo_short'>",
        "<link rel='canonical' href='https://example.com/channel/UCCsdwE2z_kRL3vfVsd5OGyA'>",
    ] {
        assert!(channel::from_html(html).is_err());
    }
    assert!(
        channel::from_html("<form action='https://consent.youtube.com/save'>")
            .unwrap_err()
            .to_string()
            .contains("consent page")
    );
}

#[tokio::test]
async fn classic_and_modern_listings_preserve_dates_and_live_placeholders() {
    let engine = Engine::new().await.unwrap();
    for status in ["LIVE", "UPCOMING", "scheduled", "DEFAULT"] {
        let mut raw = json!({"videoId":"abcdefghijk", "title":{"simpleText":"Episode", "accessibility":{"accessibilityData":{"label":"Episode"}}},
            "isPlayable":true,"lengthSeconds":"300","videoInfo":{"simpleText":"50K views • 10 years ago"},
            "thumbnailOverlays":[{"thumbnailOverlayTimeStatusRenderer":{"style":status}}]});
        if status == "scheduled" {
            raw["upcomingEventData"] = json!({"startTime":"1790812800"});
        }
        let node = YTNode::new(&engine, "PlaylistVideo", raw).await.unwrap();
        let entry = listing::entry(node.as_value().read().await.unwrap()).unwrap();
        assert_eq!(entry.video.available, status == "DEFAULT");
        assert_eq!(entry.video.duration, 300.0);
        assert_eq!(
            entry.published_text.as_deref(),
            Some("50K views • 10 years ago")
        );
    }
    for overlay in [
        "thumbnailBottomOverlayViewModel",
        "thumbnailOverlayBadgeViewModel",
    ] {
        for (text, style, available) in [
            ("Upcoming", "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT", false),
            ("NA ŻYWO", "THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE", false),
            ("1:02:03", "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT", true),
        ] {
            let key = if overlay == "thumbnailBottomOverlayViewModel" {
                "badges"
            } else {
                "thumbnailBadges"
            };
            let node = YTNode::new(&engine, "LockupView", json!({"contentId":"abcdefghijk", "contentType":"LOCKUP_CONTENT_TYPE_VIDEO",
                "contentImage":{"thumbnailViewModel":{"image":{"sources":[]},"overlays":[{overlay:{key:[{"thumbnailBadgeViewModel":{"text":text,"badgeStyle":style}}]}}]}},
                "metadata":{"lockupMetadataViewModel":{"title":{"content":"Episode"}, "metadata":{"contentMetadataViewModel":{"metadataRows":[
                    {"metadataParts":[{"text":{"content":"2 days ago"}}]},
                    {"metadataParts":[{"text":{"content":"7.3K"}},{"text":{"content":"15y ago"}}]}
                ]}}}}
            })).await.unwrap();
            let entry = listing::entry(node.as_value().read().await.unwrap()).unwrap();
            assert_eq!(entry.video.available, available);
            assert_eq!(entry.published_text.as_deref(), Some("15y ago"));
        }
    }
    let node = YTNode::new(
        &engine,
        "LockupView",
        json!({"contentId":"abcdefghijk", "contentType":"LOCKUP_CONTENT_TYPE_VIDEO"}),
    )
    .await
    .unwrap();
    let entry = listing::entry(node.as_value().read().await.unwrap()).unwrap();
    assert!(!entry.video.available);
    assert!(entry.published_text.is_none());
}

fn audio() -> Value {
    json!({"itag":140,"mimeType":"audio/mp4; codecs=\"mp4a.40.2\"","bitrate":128000,"audioQuality":"AUDIO_QUALITY_MEDIUM",
        "contentLength":"12345","approxDurationMs":"1000","lastModified":"0","url":"https://example.googlevideo.com/audio"})
}

#[tokio::test]
async fn audio_selection_keeps_aac_preference_and_rejects_invalid_formats() {
    let engine = Engine::new().await.unwrap();
    let mut opus = audio();
    opus["mimeType"] = json!("audio/webm; codecs=\"opus\"");
    opus["itag"] = json!(251);
    opus["bitrate"] = json!(300000);
    let aac = Format::new(&engine, audio()).await.unwrap();
    let opus = Format::new(&engine, opus).await.unwrap();
    assert_eq!(
        media::select_format(vec![opus.clone(), aac])
            .unwrap()
            .info()
            .itag,
        140
    );
    assert_eq!(media::select_format(vec![opus]).unwrap().info().itag, 251);
    for (key, value) in [
        ("qualityLabel", json!("360p")),
        ("mimeType", json!("video/mp4; codecs=\"mp4a.40.2\"")),
        ("audioQuality", Value::Null),
        ("drmFamilies", json!(["WIDEVINE"])),
        ("type", json!("FORMAT_STREAM_TYPE_OTF")),
        ("url", Value::Null),
        ("contentLength", Value::Null),
        ("contentLength", json!("0")),
        ("contentLength", json!("8589934593")),
        ("contentLength", json!("NaN")),
    ] {
        let mut raw = audio();
        if value.is_null() {
            raw.as_object_mut().unwrap().remove(key);
        } else {
            raw[key] = value;
        }
        let format = Format::new(&engine, raw).await.unwrap();
        assert!(media::select_format(vec![format]).is_err(), "{key}");
    }
}

#[tokio::test]
async fn playback_defers_live_and_upcoming_but_accepts_archived_streams() {
    let (yt, _) = client(vec![]).await;
    let actions = yt.actions().await.unwrap();
    for (details, status) in [
        (json!({"isLive":true}), "OK"),
        (json!({"isUpcoming":true}), "LIVE_STREAM_OFFLINE"),
        (json!({}), "LIVE_STREAM_OFFLINE"),
    ] {
        let info = VideoInfo::new(&actions, json!([{"success":true,"status_code":200,"data":{"videoDetails":details,"playabilityStatus":{"status":status}}}]), "test").await.unwrap();
        assert!(
            media::select(&info, &info.data().await.unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }
    let info = VideoInfo::new(&actions, json!([{"success":true,"status_code":200,"data":{"videoDetails":{"isLiveContent":true},"playabilityStatus":{"status":"OK"},
        "streamingData":{"expiresInSeconds":"3600","formats":[],"adaptiveFormats":[audio()]}}}]), "test").await.unwrap();
    assert_eq!(
        media::select(&info, &info.data().await.unwrap())
            .await
            .unwrap()
            .unwrap()
            .info()
            .itag,
        140
    );
    let info = VideoInfo::new(&actions, json!([{"success":true,"status_code":200,"data":{"videoDetails":{},"playabilityStatus":{"status":"UNPLAYABLE","reason":"Unavailable"}}}]), "test").await.unwrap();
    assert!(
        media::select(&info, &info.data().await.unwrap())
            .await
            .unwrap_err()
            .to_string()
            .contains("Unavailable")
    );
}

#[tokio::test]
async fn media_resolution_uses_native_transport_and_preserves_download_metadata() {
    for (publish, upload, expected) in [
        (Some("2020-02-03"), "2019-01-02", "2020-02-03"),
        (None, "2019-01-02", "2019-01-02"),
    ] {
        let mut format = audio();
        format["url"] = json!("https://example.googlevideo.com/audio?keep=yes&cpn=old");
        let (yt, requests) = client(vec![json!({
            "videoDetails":{"videoId":"abcdefghijk","title":"Episode","shortDescription":"Description","lengthSeconds":"1"},
            "playabilityStatus":{"status":"OK"},
            "microformat":{"playerMicroformatRenderer":{"publishDate":publish,"uploadDate":upload}},
            "streamingData":{"expiresInSeconds":"3600","formats":[],"adaptiveFormats":[format]}
        })]).await;
        let player = tokio::sync::OnceCell::new();
        let media = media::resolve(&yt, "abcdefghijk", &player)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(media.title, "Episode");
        assert_eq!(media.description, "Description");
        assert_eq!(media.duration, 1.0);
        assert_eq!(media.published.as_deref(), Some(expected));
        assert_eq!(media.bytes, 12345);
        assert_eq!(media.itag, 140);
        let expected_agent: String = yt
            .engine()
            .export(&["Constants", "CLIENTS", "VISIONOS", "USER_AGENT"])
            .await
            .unwrap()
            .read()
            .await
            .unwrap();
        assert_eq!(media.user_agent, expected_agent);
        assert!(
            player.get().is_none(),
            "unciphered URL does not need a player fetch"
        );
        let url = url::Url::parse(&media.url).unwrap();
        let cpns: Vec<_> = url.query_pairs().filter(|(key, _)| key == "cpn").collect();
        assert_eq!(cpns.len(), 1);
        assert_ne!(cpns[0].1, "old");
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "keep" && value == "yes")
        );
        let requests = requests.lock();
        assert_eq!(
            requests.len(),
            1,
            "audio lookup must not request WEB metadata"
        );
        assert!(requests[0].url.contains("/youtubei/v1/player?"));
        let request: Value = serde_json::from_slice(requests[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(request["context"]["client"]["clientName"], "VISIONOS");
    }
}

#[tokio::test]
async fn native_fetch_pages_include_unavailable_entries_without_video_requests() {
    let first = json!({"metadata":{"playlistMetadataRenderer":{"title":"Interviews","description":"Expert interviews"}},
    "sidebar":{"playlistSidebarRenderer":{"items":[{"playlistSidebarPrimaryInfoRenderer":{"stats":[{"simpleText":"3 episodes"}]}}]}},
    "alerts":[{"alertWithButtonRenderer":{"type":"INFO","text":{"simpleText":"Unavailable videos"}}}],
    "contents":{"itemSectionRenderer":{"contents":[{"messageRenderer":{"text":{"simpleText":"Expert interviews"}}},{"playlistVideoListRenderer":{"contents":[
        {"lockupViewModel":{"contentId":"abcdefghijk","contentType":"LOCKUP_CONTENT_TYPE_VIDEO","metadata":{"lockupMetadataViewModel":{"title":{"content":"First"}}}}},
        {"continuationItemRenderer":{"continuationEndpoint":{"commandMetadata":{"webCommandMetadata":{"apiUrl":"/youtubei/v1/browse","sendPost":true}},"continuationCommand":{"token":"next-page","request":"CONTINUATION_REQUEST_TYPE_BROWSE"}}}}
    ]}}]}}});
    let next = json!({"onResponseReceivedActions":[{"appendContinuationItemsAction":{"continuationItems":[
        {"lockupViewModel":{"contentId":"T6juU_4UqKI","contentType":"LOCKUP_CONTENT_TYPE_VIDEO"}},
        {"lockupViewModel":{"contentId":"lmnopqrstuv","contentType":"LOCKUP_CONTENT_TYPE_VIDEO","metadata":{"lockupMetadataViewModel":{"title":{"content":"Last"}}}}}
    ]}}]});
    let (yt, requests) = client(vec![first, next]).await;
    let mut source = listing::Live {
        yt: &yt,
        id: "PLtest".into(),
        previous: None,
    };
    let snapshot = listing::snapshot(&mut source, "playlist", "PLtest")
        .await
        .unwrap();
    assert_eq!(snapshot.title, "Interviews");
    assert_eq!(snapshot.videos.len(), 3);
    assert!(!snapshot.videos[1].available);
    let requests = requests.lock();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.url.contains("/youtubei/v1/browse?") && r.method == "POST")
    );
    let first: Value = serde_json::from_slice(requests[0].body.as_ref().unwrap()).unwrap();
    let next: Value = serde_json::from_slice(requests[1].body.as_ref().unwrap()).unwrap();
    assert_eq!(first["params"], "wgYCCAA=");
    assert_eq!(next["continuation"], "next-page");
}
