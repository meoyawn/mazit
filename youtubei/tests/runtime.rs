use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Duration};
use youtubei::rquickjs::{self, ArrayBuffer, Ctx, Function, TypedArray, function::Async};
use youtubei::{
    BrowseOptions, Client, Engine, FetchFunction, FetchResponse, Format, GetVideoInfoOptions,
    Innertube, JsValue, Json, Player, Playlist, SessionOptions, Text, UniversalCache, YTNode, json,
    models::PlaylistItem,
};

static_assertions::assert_not_impl_any!(Engine: Send, Sync);
static_assertions::assert_not_impl_any!(Innertube: Send, Sync);
static_assertions::assert_not_impl_any!(JsValue: Send, Sync);
static_assertions::assert_impl_all!(youtubei::models::PlaylistData: Send, Sync);
static_assertions::assert_impl_all!(youtubei::models::VideoInfoData: Send, Sync);
static_assertions::assert_impl_all!(youtubei::models::FormatInfo: Send, Sync);

#[tokio::test(flavor = "current_thread")]
async fn typed_snapshots_preserve_dates_nan_and_unknown_nodes() -> youtubei::Result<()> {
    use youtubei::models::{LockupContentType, PlaylistItem};
    let engine = Engine::new().await?;
    let node = YTNode::new(
        &engine,
        "PlaylistVideo",
        json!({
            "videoId":"abcdefghijk", "isPlayable":true,
            "title":{"simpleText":"", "accessibility":{"accessibilityData":{"label":""}}},
            "upcomingEventData":{"startTime":"1790812800"}
        }),
    )
    .await?;
    let PlaylistItem::PlaylistVideo(video) = node.as_value().read().await? else {
        panic!("classic item expected")
    };
    assert_eq!(
        video.title.as_str(),
        "N/A",
        "use upstream Text.toString semantics"
    );
    assert!(video.duration.seconds.is_nan());
    assert_eq!(
        video.upcoming.unwrap().timestamp_millis,
        1_790_812_800_000.0
    );
    let lockup = YTNode::new(
        &engine,
        "LockupView",
        json!({
            "contentId":"abcdefghijk", "contentType":"LOCKUP_CONTENT_TYPE_FUTURE_KIND"
        }),
    )
    .await?;
    let PlaylistItem::LockupView(item) = lockup.as_value().read().await? else {
        panic!("lockup expected")
    };
    assert_eq!(
        item.content_type,
        LockupContentType::Other("FUTURE_KIND".into())
    );
    assert!(item.metadata.is_none());
    assert!(item.content_image.is_none());
    let message = YTNode::new(
        &engine,
        "Message",
        json!({"text":{"simpleText":"Description"}}),
    )
    .await?;
    assert_eq!(
        message.as_value().read::<PlaylistItem>().await?,
        PlaylistItem::Other {
            node_type: "Message".into()
        }
    );
    let mut raw = audio_format();
    raw["contentLength"] = json!("NaN");
    let format = Format::new(&engine, raw).await?;
    assert!(
        format.info().content_length.unwrap().is_nan(),
        "NaN must not be serialized as null"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn typed_cache_and_player_keep_format_deciphering_in_upstream() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let cache = UniversalCache::new(&engine, false, None).await?;
    // A known extractor output: exercise upstream Player + Format and the
    // native evaluator without depending on YouTube's changing player source.
    let source = r#"
        const exportedVars = { nsigFunction: (url, sp, sig) => {
            const values = new URL(url).searchParams;
            return new class {
                transform() { values.set('n', 'decoded_n'); values.set(sp, sig.split('').reverse().join('')); }
                get(key) { return values.get(key); }
            };
        }};
    "#;
    let player = Player::new(&engine, "fixture", 1234, json!({"output":source})).await?;
    player.as_value().call("cache", &[(&cache).into()]).await?;
    let fetch = engine
        .fetch_with(|_| async { Err(youtubei::Error::new("cache hit must not fetch")) })
        .await?;
    let yt = Innertube::create_in(
        &engine,
        SessionOptions {
            cache: Some(cache.as_cache()),
            fetch: Some(fetch),
            ..SessionOptions::local()
        },
    )
    .await?;
    let session = yt.session().await?;
    let cached = Player::create(
        &engine,
        session.cache().await?.as_ref(),
        Some(&session.fetch().await?),
        None,
        Some("fixture"),
    )
    .await?;
    session.set_player(&cached).await?;
    assert_eq!(cached.signature_timestamp().await?, 1234);
    let mut raw = audio_format();
    raw.as_object_mut().unwrap().remove("url");
    raw["signatureCipher"] =
        json!("url=https%3A%2F%2Fexample.googlevideo.com%2Faudio%3Fn%3Doriginal&sp=sig&s=abc");
    let format = Format::new(&engine, raw).await?;
    let url = format.decipher(session.player().await?.as_ref()).await?;
    assert!(url.contains("n=decoded_n"), "{url}");
    assert!(url.contains("sig=cba"), "{url}");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn instances_reuse_engine_and_keep_session_and_objects_alive() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let yt = Innertube::create_in(&engine, SessionOptions::local()).await?;
    let session = yt.session().await?;
    assert_eq!(session.client_name().await?, "WEB");
    assert!(
        session
            .as_value()
            .same_identity(yt.session().await?.as_value())
            .await?
    );
    let second = Innertube::new(&session).await?;
    assert!(
        session
            .as_value()
            .same_identity(second.session().await?.as_value())
            .await?
    );
    let independent = Innertube::create_in(&engine, SessionOptions::local()).await?;
    assert!(
        !session
            .as_value()
            .same_identity(independent.session().await?.as_value())
            .await?
    );
    let text = Text::new(
        &engine,
        json!({"runs": [{"text": "one"}, {"text": " two"}]}),
    )
    .await?;
    drop(engine);
    drop(yt);
    drop(second);
    drop(independent);
    text.engine().run_gc().await;
    assert_eq!(text.to_string().await?, "one two");
    assert_eq!(session.client_name().await?, "WEB");
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn native_callbacks_properties_errors_and_runtime_boundaries() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let text = Text::new(&engine, json!({"simpleText": "retained"})).await?;
    let callback = engine
        .value_with(|ctx| {
            Ok(Function::new(ctx, |value: String| format!("hello {value}"))?.into_value())
        })
        .await?;
    assert_eq!(
        callback
            .apply(None, &["world".into()])
            .await?
            .read::<String>()
            .await?,
        "hello world"
    );
    let object = engine.value(json!({})).await?;
    object.set("text", text.as_value()).await?;
    assert!(
        text.as_value()
            .same_identity(&object.get("text").await?)
            .await?
    );
    assert!(object.get("missing").await?.is_nullish().await?);

    let throw = engine
        .value_with(|ctx| {
            Ok(Function::new(ctx, |ctx: Ctx| -> rquickjs::Result<()> {
                Err(rquickjs::Exception::throw_type(&ctx, "fixture failure"))
            })?
            .into_value())
        })
        .await?;
    let error = throw.apply(None, &[]).await.unwrap_err();
    assert!(error.message.contains("fixture failure"), "{error}");
    assert_eq!(text.to_string().await?, "retained");
    let other = Engine::new().await?;
    assert!(other.value(text.as_value()).await.is_err());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rust_fetch_preserves_binary_bodies_headers_status_and_errors() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let fetch = engine
        .fetch_with(|request| async move {
            if request.url.ends_with("/error") {
                return Err(youtubei::Error::new("transport unavailable"));
            }
            if request.method == "HEAD" {
                assert!(request.body.is_none());
                return Ok(FetchResponse {
                    status: 204,
                    headers: vec![],
                    body: vec![],
                });
            }
            assert_eq!(request.method, "POST");
            assert_eq!(request.body.as_deref(), Some([0, 128, 255].as_slice()));
            assert!(
                request
                    .headers
                    .contains(&("x-request".into(), "present".into()))
            );
            Ok(FetchResponse {
                status: 206,
                headers: vec![("x-response".into(), "present".into())],
                body: vec![255, 128, 0],
            })
        })
        .await?;
    let init = engine
        .value(json!({"method":"POST", "headers":{"x-request":"present"}}))
        .await?;
    let bytes = engine
        .value_with(|ctx| Ok(TypedArray::<u8>::new(ctx, vec![0, 128, 255])?.into_value()))
        .await?;
    init.set("body", bytes).await?;
    let response = fetch
        .as_value()
        .apply(None, &["https://example.com/echo".into(), init.into()])
        .await?;
    assert_eq!(response.get("status").await?.read::<u16>().await?, 206);
    assert_eq!(
        response
            .get("headers")
            .await?
            .call("get", &["x-response".into()])
            .await?
            .read::<String>()
            .await?,
        "present"
    );
    let body = response
        .call("arrayBuffer", &[])
        .await?
        .with(|ctx, value| {
            use rquickjs::FromJs;
            Ok(ArrayBuffer::from_js(&ctx, value)?
                .as_bytes()
                .unwrap()
                .to_vec())
        })
        .await?;
    assert_eq!(body, [255, 128, 0]);
    let empty = fetch
        .as_value()
        .apply(
            None,
            &[
                "https://example.com/empty".into(),
                json!({"method":"HEAD"}).into(),
            ],
        )
        .await?;
    assert_eq!(empty.get("status").await?.read::<u16>().await?, 204);
    assert_eq!(empty.call("text", &[]).await?.read::<String>().await?, "");
    assert!(
        fetch
            .as_value()
            .apply(None, &["https://example.com/error".into()])
            .await
            .unwrap_err()
            .message
            .contains("transport unavailable")
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn library_node_checks_and_format_methods_preserve_prototypes() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let node = YTNode::new(&engine, "PlaylistVideo", json!({
        "videoId": "abcdefghijk", "title": {"simpleText": "Episode", "accessibility": {"accessibilityData": {"label": "Episode"}}},
        "isPlayable": true, "lengthSeconds": "300",
        "videoInfo": {"simpleText": "50K views • 10 years ago"}
    })).await?;
    assert!(node.is(&["PlaylistVideo"]).await?);
    assert!(!node.is(&["LockupView", "Alert"]).await?);
    assert_eq!(
        node.text("video_info").await?.to_string().await?,
        "50K views • 10 years ago"
    );
    assert_eq!(
        node.as_value()
            .get("duration")
            .await?
            .get("seconds")
            .await?
            .read::<u32>()
            .await?,
        300
    );
    let format = Format::new(&engine, audio_format()).await?;
    assert_eq!(format.info().itag, 140);
    assert!(format.info().has_audio);
    assert!(!format.info().has_video);
    assert_eq!(format.info().content_length, Some(12345.0));
    assert_eq!(
        format.decipher(None).await?,
        "https://example.googlevideo.com/audio"
    );
    Ok(())
}

fn audio_format() -> Json {
    json!({"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 128000,
        "audioQuality": "AUDIO_QUALITY_MEDIUM", "contentLength": "12345",
        "approxDurationMs": "1000", "lastModified": "0", "url": "https://example.googlevideo.com/audio"})
}

type Requests = Rc<RefCell<Vec<(String, Json)>>>;

async fn fixture_fetch(
    engine: &Engine,
    responses: Vec<Json>,
) -> youtubei::Result<(FetchFunction, Requests)> {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let captured = requests.clone();
    let responses = RefCell::new(VecDeque::from(responses));
    let fetch = engine
        .fetch_with(move |request| {
            captured.borrow_mut().push((
                request.url,
                serde_json::from_slice(request.body.as_deref().unwrap_or(b"{}")).unwrap(),
            ));
            let response = responses
                .borrow_mut()
                .pop_front()
                .expect("unexpected request");
            async move {
                Ok(FetchResponse {
                    status: 200,
                    headers: vec![],
                    body: response.to_string().into_bytes(),
                })
            }
        })
        .await?;
    Ok((fetch, requests))
}

#[tokio::test(flavor = "current_thread")]
async fn actual_flat_playlist_continuations_use_only_browse_requests() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let first = json!({
        "metadata": {"playlistMetadataRenderer": {"title": "Interviews", "description": "Expert interviews"}},
        "sidebar": {"playlistSidebarRenderer": {"items": [{"playlistSidebarPrimaryInfoRenderer": {"stats": [{"simpleText": "3 episodes"}]}}]}},
        "contents": {"playlistVideoListRenderer": {"contents": [
            playlist_video("abcdefghijk", "First", true),
            {"continuationItemRenderer": {"continuationEndpoint": {
                "commandMetadata": {"webCommandMetadata": {"apiUrl": "/youtubei/v1/browse", "sendPost": true}},
                "continuationCommand": {"token": "next-page", "request": "CONTINUATION_REQUEST_TYPE_BROWSE"}
            }}}
        ]}}
    });
    let next = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [
        playlist_video("T6juU_4UqKI", "Unavailable", false),
        playlist_video("lmnopqrstuv", "Last", true)
    ]}}]});
    let (fetch, requests) = fixture_fetch(&engine, vec![first, next]).await?;
    let yt = Innertube::create_in(
        &engine,
        SessionOptions {
            fetch: Some(fetch),
            ..SessionOptions::local()
        },
    )
    .await?;
    let actions = yt.actions().await?;
    let response = actions
        .browse(BrowseOptions {
            browse_id: "VLPLtest".into(),
            params: Some("wgYCCAA=".into()),
        })
        .await?;
    let page = Playlist::new(&actions, &response, false).await?;
    assert_eq!(page.info().await?.title.as_deref(), Some("Interviews"));
    assert!(page.has_continuation().await?);
    assert_eq!(page.items().await?.len(), 1);
    let continued = page.get_continuation().await?;
    assert!(!continued.has_continuation().await?);
    let nodes = continued.items().await?;
    assert_eq!(nodes.len(), 2);
    let PlaylistItem::PlaylistVideo(video) = &nodes[0] else {
        panic!("expected classic item")
    };
    assert_eq!(video.id, "T6juU_4UqKI");
    assert!(!video.is_playable);
    let requests = requests.borrow();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|(url, _)| url.contains("/youtubei/v1/browse?"))
    );
    assert_eq!(requests[0].1["params"], "wgYCCAA=");
    assert_eq!(requests[1].1["continuation"], "next-page");
    Ok(())
}

fn playlist_video(id: &str, title: &str, playable: bool) -> Json {
    json!({"playlistVideoRenderer":{"videoId":id, "isPlayable":playable,"lengthSeconds":"1",
        "title":{"simpleText":title,"accessibility":{"accessibilityData":{"label":title}}}}})
}

#[tokio::test(flavor = "current_thread")]
async fn basic_info_uses_upstream_client_and_returns_live_format_handles() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let (fetch, requests) = fixture_fetch(&engine, vec![json!({
        "videoDetails": {"videoId": "abcdefghijk", "title": "Episode", "lengthSeconds": "1"},
        "playabilityStatus": {"status": "OK"},
        "streamingData": {"expiresInSeconds": "3600", "formats": [], "adaptiveFormats": [audio_format()]}
    })]).await?;
    let yt = Innertube::create_in(
        &engine,
        SessionOptions {
            fetch: Some(fetch),
            ..SessionOptions::local()
        },
    )
    .await?;
    let info = yt
        .get_basic_info(
            "abcdefghijk",
            GetVideoInfoOptions {
                client: Some(Client::VisionOs),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(info.basic_info().await?.title.as_deref(), Some("Episode"));
    assert_eq!(info.adaptive_formats().await?[0].info().itag, 140);
    assert_eq!(
        requests.borrow()[0].1["context"]["client"]["clientName"],
        "VISIONOS"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn native_platform_cache_hash_and_player_evaluator() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let cache = UniversalCache::new(&engine, false, None).await?;
    let bytes = engine
        .value_with(|ctx| Ok(ArrayBuffer::new(ctx, vec![0u8, 1, 255])?.into_value()))
        .await?;
    cache
        .as_value()
        .call("set", &["key".into(), bytes.into()])
        .await?;
    let actual = cache
        .as_value()
        .call("get", &["key".into()])
        .await?
        .with(|ctx, value| {
            use rquickjs::FromJs;
            Ok(ArrayBuffer::from_js(&ctx, value)?
                .as_bytes()
                .unwrap()
                .to_vec())
        })
        .await?;
    assert_eq!(actual, [0, 1, 255]);
    cache.as_value().call("remove", &["key".into()]).await?;
    assert!(
        cache
            .as_value()
            .call("get", &["key".into()])
            .await?
            .is_nullish()
            .await?
    );
    let shim = engine.export(&["Platform", "shim"]).await?;
    assert_eq!(
        shim.call("sha1Hash", &["abc".into()])
            .await?
            .read::<String>()
            .await?,
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    let evaluated = shim
        .call(
            "eval",
            &[
                json!({"output": "return { doubled: n * 2 };"}).into(),
                json!({"n": 21}).into(),
            ],
        )
        .await?;
    assert_eq!(evaluated.get("doubled").await?.read::<u32>().await?, 42);
    let player = Player::new(&engine, "fixture", 1234, json!({"output": ""})).await?;
    let yt = Innertube::create_in(&engine, SessionOptions::local()).await?;
    yt.session().await?.set_player(&player).await?;
    assert!(
        player
            .as_value()
            .same_identity(yt.session().await?.player().await?.unwrap().as_value())
            .await?
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn asynchronous_calls_overlap_in_one_engine() -> youtubei::Result<()> {
    let engine = Engine::new().await?;
    let barrier = Rc::new(tokio::sync::Barrier::new(2));
    let function = engine
        .value_with(|ctx| {
            Ok(Function::new(
                ctx,
                Async(move |id: u32| {
                    let barrier = barrier.clone();
                    async move {
                        barrier.wait().await;
                        Ok::<_, rquickjs::Error>(id)
                    }
                }),
            )?
            .into_value())
        })
        .await?;
    let (one, two) = tokio::time::timeout(Duration::from_secs(5), async {
        let one = [1u32.into()];
        let two = [2u32.into()];
        tokio::join!(function.apply(None, &one), function.apply(None, &two))
    })
    .await
    .expect("calls must overlap; serial execution would deadlock at the barrier");
    assert_eq!(one?.read::<u32>().await?, 1);
    assert_eq!(two?.read::<u32>().await?, 2);
    Ok(())
}

#[test]
fn independent_worker_threads_each_own_a_reusable_engine() {
    let workers: Vec<_> = (0..3)
        .map(|_| {
            std::thread::spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let yt = Innertube::create(SessionOptions::local()).await.unwrap();
                        for _ in 0..3 {
                            assert_eq!(
                                yt.session().await.unwrap().client_name().await.unwrap(),
                                "WEB"
                            );
                        }
                    });
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}
