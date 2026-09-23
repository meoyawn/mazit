use super::*;
use serde_json::json;

fn deletion_storage(reject: Arc<AtomicBool>) -> MockStorage {
    MockStorage::with_handler(move |request| {
        let target = request.headers.split_whitespace().nth(1).unwrap();
        let url = url::Url::parse(&format!("http://localhost{target}")).unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        let body = if request.headers.starts_with("GET ") {
            assert_eq!(query["prefix"], "library/playlist-test/");
            assert!(!query.contains_key("delimiter"));
            if query.contains_key("uploads") {
                if query.contains_key("key-marker") {
                    assert_eq!(query["key-marker"], "library/playlist-test/audio.m4a");
                    assert_eq!(query["upload-id-marker"], "first");
                    "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated><Upload><Key>library/playlist-test/audio.m4a</Key><UploadId>second</UploadId></Upload></ListMultipartUploadsResult>".into()
                } else {
                    "<ListMultipartUploadsResult><IsTruncated>true</IsTruncated><NextKeyMarker>library/playlist-test/audio.m4a</NextKeyMarker><NextUploadIdMarker>first</NextUploadIdMarker><Upload><Key>library/playlist-test/audio.m4a</Key><UploadId>first</UploadId></Upload></ListMultipartUploadsResult>".into()
                }
            } else if query.contains_key("continuation-token") {
                assert_eq!(query["continuation-token"], "next-page");
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>library/playlist-test/rss.xml</Key></Contents><Contents><Key>library/playlist-test/cover.jpg</Key></Contents><Contents><Key>library/playlist-test/</Key></Contents></ListBucketResult>".into()
            } else {
                let objects: String = (0..1000)
                    .map(|i| {
                        format!("<Contents><Key>library/playlist-test/{i}.m4a</Key></Contents>")
                    })
                    .collect();
                format!(
                    "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>next-page</NextContinuationToken>{objects}</ListBucketResult>"
                )
            }
        } else if request.headers.starts_with("DELETE ") {
            assert_eq!(request.path(), "/podcasts/library/playlist-test/audio.m4a");
            assert!(matches!(query["uploadId"].as_str(), "first" | "second"));
            String::new()
        } else {
            assert!(request.headers.starts_with("POST "));
            assert!(query.contains_key("delete"));
            let body = std::str::from_utf8(&request.body).unwrap();
            assert_eq!(
                body.matches("<Key>").count(),
                body.matches("<Key>library/playlist-test/").count()
            );
            if reject.load(Ordering::Relaxed) {
                "<DeleteResult><Error><Key>library/playlist-test/0.m4a</Key><Code>AccessDenied</Code></Error></DeleteResult>".into()
            } else {
                "<DeleteResult/>".into()
            }
        };
        ("200 OK", body)
    })
}

fn core(directory: &Path, youtube: YouTube) -> Core {
    Core {
        db: Database::open(&directory.join("mazit.sqlite")).unwrap(),
        youtube,
        client: reqwest::Client::new(),
        directory: directory.to_owned(),
        covers: RwLock::default(),
        downloads: DownloadManager::default(),
        _lock: std::fs::File::create(directory.join("mazit.lock")).unwrap(),
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Operation did not finish");
}

#[tokio::test]
async fn deletes_all_remote_pages_and_local_files_then_sqlite_without_touching_other_sources() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path(), YouTube::with_responses(Vec::new()));
    let id = core
        .db
        .add("playlist", "test", "https://www.youtube.com")
        .unwrap();
    let other = core
        .db
        .add("playlist", "test-other", "https://www.youtube.com")
        .unwrap();
    let cover = Arc::new(Cover::from_bytes(b"\xff\xd8\xff\xe0".to_vec()).unwrap());
    for id in [&id, &other] {
        let source = core.db.source(id).unwrap();
        cache_cover(directory.path(), &source, Some(&cover)).unwrap();
        core.covers.write().insert(id.clone(), cover.clone());
        let transfers = core.transfer_directory(&source);
        std::fs::create_dir_all(&transfers).unwrap();
        std::fs::write(transfers.join("partial"), b"partial audio").unwrap();
        core.downloads
            .enqueue(id, "Podcast", &[("audio".into(), "Audio".into())]);
        core.db
            .snapshot(
                id,
                &crate::youtube::Snapshot {
                    title: "Podcast".into(),
                    description: String::new(),
                    cover_url: None,
                    videos: vec![crate::youtube::Video {
                        id: "audio".into(),
                        title: "Audio".into(),
                        description: String::new(),
                        published: None,
                        duration: 1.,
                        available: true,
                    }],
                },
            )
            .unwrap();
    }
    let mock = deletion_storage(Arc::new(AtomicBool::new(false)));
    core.delete(&id, Some(&mock.storage)).await.unwrap();
    assert!(core.db.source(&id).is_err());
    assert!(core.db.episodes(&id).unwrap().is_empty());
    assert_eq!(core.db.episodes(&other).unwrap().len(), 1);
    assert!(!directory.path().join("covers/playlist-test").exists());
    assert!(
        !directory
            .path()
            .join("transfers/mazit-playlist-test")
            .exists()
    );
    assert!(directory.path().join("covers/playlist-test-other").exists());
    assert!(
        directory
            .path()
            .join("transfers/mazit-playlist-test-other/partial")
            .exists()
    );
    assert!(!core.covers.read().contains_key(&id));
    assert_eq!(core.downloads.snapshot().items.len(), 1);
    assert_eq!(core.downloads.snapshot().items[0].source_id, other);
    let requests = mock.uploads.lock();
    let batches: Vec<_> = requests
        .iter()
        .filter(|r| r.headers.starts_with("POST "))
        .map(|r| {
            std::str::from_utf8(&r.body)
                .unwrap()
                .matches("<Key>")
                .count()
        })
        .collect();
    assert_eq!(batches, [1000, 3]);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.headers.starts_with("DELETE "))
            .count(),
        2
    );
}

#[tokio::test]
async fn worker_cancels_a_sync_with_queued_downloads_and_retries_partial_s3_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let youtube = YouTube::with_responses(
        ["other", "test"]
            .into_iter()
            .map(|id| {
                (
                    "page",
                    json!({"id":id, "continuation":false}),
                    json!({"title":"Podcast", "count":"1 video", "continuation":false,
            "videos":[{"id":"abcdefghijk", "title":"Audio", "duration":1, "available":true}]}),
                )
            })
            .collect(),
    );
    let core = core(directory.path(), youtube);
    let id = core
        .db
        .add("playlist", "test", "https://www.youtube.com")
        .unwrap();
    let other = core
        .db
        .add("playlist", "other", "https://www.youtube.com")
        .unwrap();
    let db = core.db.clone();
    let downloads = core.downloads.clone();
    downloads.set_paused(true);
    let reject = Arc::new(AtomicBool::new(true));
    let mock = deletion_storage(reject.clone());
    let state = Arc::new(RwLock::new(ViewState::default()));
    let (sender, receiver) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run_worker(
        core,
        Some(mock.storage.clone()),
        directory.path().join("config.toml"),
        state.clone(),
        receiver,
    ));
    until(|| downloads.snapshot().items.len() == 2).await;
    sender.send(Command::Delete(id.clone())).unwrap();
    until(|| db.source(&id).unwrap().phase == "delete_error").await;
    assert_eq!(downloads.snapshot().items.len(), 1);
    assert_eq!(downloads.snapshot().items[0].source_id, other);
    assert_eq!(db.episodes(&id).unwrap().len(), 1);
    sender.send(Command::Refresh(None)).unwrap();
    assert_eq!(db.source(&id).unwrap().phase, "delete_error");
    reject.store(false, Ordering::Relaxed);
    sender.send(Command::Delete(id.clone())).unwrap();
    until(|| db.source(&id).is_err()).await;
    assert_eq!(db.source(&other).unwrap().phase, "downloading");
    assert_eq!(downloads.snapshot().items.len(), 1);
    assert!(db.episodes(&id).unwrap().is_empty());
    assert!(
        mock.uploads
            .lock()
            .iter()
            .all(|r| !r.headers.starts_with("PUT "))
    );
    drop(sender);
    worker.await.unwrap();
}

#[tokio::test]
async fn interrupted_deletion_resumes_on_startup_without_resyncing() {
    let directory = tempfile::tempdir().unwrap();
    let core = core(directory.path(), YouTube::with_responses(Vec::new()));
    let id = core
        .db
        .add("playlist", "test", "https://www.youtube.com")
        .unwrap();
    core.db.phase(&id, "deleting", None).unwrap();
    let db = core.db.clone();
    let mock = deletion_storage(Arc::new(AtomicBool::new(false)));
    let (sender, receiver) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run_worker(
        core,
        Some(mock.storage.clone()),
        directory.path().join("config.toml"),
        Arc::default(),
        receiver,
    ));
    until(|| db.source(&id).is_err()).await;
    assert!(
        mock.uploads
            .lock()
            .iter()
            .all(|r| !r.headers.starts_with("PUT "))
    );
    drop(sender);
    worker.await.unwrap();
}

#[tokio::test]
async fn cancelling_an_upload_prevents_subsequent_writes() {
    let mock = MockStorage::new(false);
    let cancelled = CancellationToken::new();
    let storage = mock.storage.with_write_cancellation(cancelled.clone());
    cancelled.cancel();
    assert!(
        storage
            .put_bytes("playlist-test/rss.xml", b"feed".to_vec(), "application/xml")
            .await
            .is_err()
    );
    assert!(mock.uploads.lock().is_empty());
}

#[tokio::test]
async fn deletion_waits_for_an_inflight_publication_before_removing_files() {
    let directory = tempfile::tempdir().unwrap();
    let youtube = YouTube::with_responses(vec![(
        "page",
        json!({"id":"test", "continuation":false}),
        json!({
            "title":"Podcast", "count":"0 videos", "continuation":false, "videos":[]
        }),
    )]);
    let core = core(directory.path(), youtube);
    let id = core
        .db
        .add("playlist", "test", "https://www.youtube.com")
        .unwrap();
    let source = core.db.source(&id).unwrap();
    let db = core.db.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let started = parking_lot::Mutex::new(Some(started));
    let (release, wait) = std::sync::mpsc::channel();
    let mock = MockStorage::with_handler(move |request| {
        if request.headers.starts_with("PUT ") {
            started.lock().take().unwrap().send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            ("200 OK", String::new())
        } else if request.headers.contains("uploads") {
            ("200 OK", "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>".into())
        } else if request.headers.starts_with("GET ") {
            ("200 OK", "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>library/playlist-test/rss.xml</Key></Contents></ListBucketResult>".into())
        } else {
            assert!(request.headers.starts_with("POST "));
            ("200 OK", "<DeleteResult/>".into())
        }
    });
    let (sender, receiver) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run_worker(
        core,
        Some(mock.storage.clone()),
        directory.path().join("config.toml"),
        Arc::default(),
        receiver,
    ));
    ready.await.unwrap();
    let cover = Cover::from_bytes(b"\xff\xd8\xff\xe0".to_vec()).unwrap();
    cache_cover(directory.path(), &source, Some(&cover)).unwrap();
    sender.send(Command::Delete(id.clone())).unwrap();
    until(|| db.source(&id).unwrap().phase == "deleting").await;
    assert!(directory.path().join("covers/playlist-test").exists());
    release.send(()).unwrap();
    until(|| db.source(&id).is_err()).await;
    assert!(!directory.path().join("covers/playlist-test").exists());
    assert_eq!(mock.uploads.lock().len(), 4);
    drop(sender);
    worker.await.unwrap();
}

#[tokio::test]
async fn cancelled_audio_conversion_stops_and_removes_its_output() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("silence.aac");
    let output = directory.path().join("audio.m4a");
    // Repeat synthetic AAC silence so remuxing remains active until cancelled.
    let mut audio = std::fs::File::create(&input).unwrap();
    for _ in 0..10_000 {
        audio
            .write_all(include_bytes!("fixtures/silence.aac"))
            .unwrap();
    }
    drop(audio);
    let cancelled = CancellationToken::new();
    let worker_cancelled = cancelled.clone();
    let worker_output = output.clone();
    let worker = tokio::task::spawn_blocking(move || {
        crate::audio::prepare_m4a(&input, &worker_output, &worker_cancelled)
    });
    until(|| output.exists() || worker.is_finished()).await;
    cancelled.cancel();
    let result = tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(!output.exists());
}
