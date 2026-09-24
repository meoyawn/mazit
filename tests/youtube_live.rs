use mazit::engine::Core;
use serde::Deserialize;

#[derive(Deserialize)]
struct Listing {
    url: String,
    ids: Vec<String>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live YouTube/yt-dlp comparison; run bun run scripts/test-youtube-live.ts after task prepare"]
async fn app_subscriptions_and_all_flat_pages_match_ytdlp() -> anyhow::Result<()> {
    let fixtures = std::env::var("MAZIT_YOUTUBE_EXPECTED_LISTINGS")?;
    let listings: Vec<Listing> = serde_json::from_slice(&std::fs::read(fixtures)?)?;
    assert!(!listings.is_empty());
    let directory = tempfile::tempdir()?;
    let core = Core::new(directory.path().into(), None)?;
    for listing in listings {
        let id = core.add(&listing.url).await?;
        assert_eq!(core.add(&listing.url).await?, id);
        let source = core.db.source(&id)?;
        let snapshot = core
            .youtube
            .snapshot(&source.kind, &source.youtube_id)
            .await?;
        assert_eq!(
            snapshot
                .videos
                .iter()
                .map(|v| v.id.as_str())
                .collect::<Vec<_>>(),
            listing.ids
        );
        assert!(!snapshot.title.is_empty());
        assert!(snapshot.cover_url.is_some());
        core.db.snapshot(&id, &snapshot)?;
        assert_eq!(core.db.episodes(&id)?.len(), snapshot.videos.len());
        eprintln!(
            "App subscription {}: all {} entries match yt-dlp in order",
            source.youtube_id,
            snapshot.videos.len()
        );
    }
    Ok(())
}
