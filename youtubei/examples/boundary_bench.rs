//! Compare identical owned snapshots of a parsed 100-item classic playlist.
//! No network, startup or parsing in the timed region. Run in release mode.
use std::{hint::black_box, time::Instant};
use youtubei::{
    Innertube, Playlist, Result, SessionOptions, json,
    models::{PlaylistData, PlaylistItem, PlaylistVideo},
};

async fn field_by_field(page: &Playlist) -> Result<PlaylistData> {
    let mut items = Vec::new();
    for node in page.as_value().get("items").await?.elements().await? {
        let kind: String = node.get("type").await?.read().await?;
        assert_eq!(kind, "PlaylistVideo");
        items.push(PlaylistItem::PlaylistVideo(PlaylistVideo {
            id: node.get("id").await?.read().await?,
            title: node.get("title").await?.read().await?,
            video_info: node.get("video_info").await?.read().await?,
            duration: node.get("duration").await?.read().await?,
            is_playable: node.get("is_playable").await?.read().await?,
            is_live: node.get("is_live").await?.read().await?,
            is_upcoming: node.get("is_upcoming").await?.read().await?,
            upcoming: node.get("upcoming").await?.read().await?,
        }));
    }
    Ok(PlaylistData {
        info: page.info().await?,
        alerts: page.alerts().await?,
        has_continuation: page.has_continuation().await?,
        items,
    })
}

async fn sample(page: &Playlist, batched: bool) -> Result<f64> {
    let start = Instant::now();
    for _ in 0..20 {
        black_box(if batched {
            page.data().await?
        } else {
            field_by_field(page).await?
        });
    }
    Ok(start.elapsed().as_secs_f64() * 1_000_000.0 / 20.0)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let yt = Innertube::create(SessionOptions::local()).await?;
    let contents: Vec<_> = (0..100)
        .map(|i| {
            json!({"playlistVideoRenderer": {
                "videoId": format!("{i:011}"), "isPlayable": true, "lengthSeconds": "300",
                "title": {"runs": [{"text": "Episode "}, {"text": i.to_string()}],
                    "accessibility": {"accessibilityData": {"label": "Episode"}}},
                "videoInfo": {"simpleText": "50K views • 2 days ago"}
            }})
        })
        .collect();
    let page = Playlist::new(&yt.actions().await?, json!({
        "success": true, "status_code": 200, "data": {
            "metadata": {"playlistMetadataRenderer": {"title": "Benchmark", "description": "Fixture"}},
            "contents": {"playlistVideoListRenderer": {"contents": contents}}
        }
    }), false).await?;
    assert_eq!(page.data().await?, field_by_field(&page).await?);
    // Warm both paths; alternate order to reduce order-dependent bias.
    sample(&page, false).await?;
    sample(&page, true).await?;
    let mut old = Vec::new();
    let mut batched = Vec::new();
    for i in 0..31 {
        if i % 2 == 0 {
            old.push(sample(&page, false).await?);
            batched.push(sample(&page, true).await?);
        } else {
            batched.push(sample(&page, true).await?);
            old.push(sample(&page, false).await?);
        }
    }
    old.sort_by(f64::total_cmp);
    batched.sort_by(f64::total_cmp);
    println!("100 items, median of 31 samples × 20 reads (µs/page)");
    println!(
        "field-by-field: {:.1}; batched: {:.1}; ratio: {:.2}x",
        old[15],
        batched[15],
        old[15] / batched[15]
    );
    Ok(())
}
