//! Flat playlist enumeration using only Rust and the upstream library API.
use youtubei::{BrowseOptions, Innertube, Playlist, SessionOptions, models::PlaylistItem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let id = std::env::args()
        .nth(1)
        .ok_or("usage: playlist PLAYLIST_ID")?;
    let yt = Innertube::create(SessionOptions::local()).await?;
    let actions = yt.actions().await?;
    let response = actions
        .browse(BrowseOptions {
            browse_id: format!("VL{id}"),
            params: Some("wgYCCAA=".into()),
        })
        .await?;
    let mut page = Playlist::new(&actions, &response, false).await?;
    let mut ids = Vec::<String>::new();
    loop {
        let data = page.data().await?;
        for node in data.items {
            ids.push(match node {
                PlaylistItem::PlaylistVideo(video) => video.id,
                PlaylistItem::LockupView(video) => video.content_id,
                PlaylistItem::Other { .. } => return Err("unsupported playlist node".into()),
            });
        }
        if !data.has_continuation {
            break;
        }
        page = page.get_continuation().await?;
    }
    println!("{}", serde_json::to_string(&ids)?);
    Ok(())
}
