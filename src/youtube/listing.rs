use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::collections::HashSet;
use youtubei::{
    BrowseOptions, Innertube, Playlist,
    models::{
        ContentImage, LockupContentType, PlaylistAlert, PlaylistItem, Thumbnail, ThumbnailOverlay,
    },
};

use super::{ListingVideo, Snapshot, Video, dates, parse_publication_date, valid_id};

#[derive(Default, Deserialize)]
pub(super) struct Page {
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    pub cover_url: Option<String>,
    pub count: Option<String>,
    #[serde(default)]
    pub continuation: bool,
    #[serde(default)]
    pub suspicious: bool,
    pub videos: Vec<ListingVideo>,
}

#[derive(Deserialize)]
pub(super) struct Channel {
    pub title: Option<String>,
    #[serde(default)]
    pub description: String,
    pub cover_url: Option<String>,
}

pub(super) trait Source {
    async fn next_page(&mut self) -> Result<Page>;
    async fn channel(&mut self, id: &str) -> Result<Channel>;
}

pub(super) struct Live<'a> {
    pub yt: &'a Innertube,
    pub id: String,
    pub previous: Option<Playlist>,
}

impl Source for Live<'_> {
    async fn next_page(&mut self) -> Result<Page> {
        super::client::bounded(async {
            let page = if let Some(previous) = self.previous.take() {
                previous.get_continuation().await?
            } else {
                let actions = self.yt.actions().await?;
                let response = actions
                    .browse(BrowseOptions {
                        browse_id: format!("VL{}", self.id),
                        params: Some("wgYCCAA=".into()),
                    })
                    .await?;
                Playlist::new(&actions, &response, false).await?
            };
            let result = page_data(&page).await?;
            if result.continuation {
                self.previous = Some(page);
            }
            Ok(result)
        })
        .await
    }

    async fn channel(&mut self, id: &str) -> Result<Channel> {
        super::client::bounded(async {
            let metadata = self.yt.get_channel(id).await?.metadata().await?;
            Ok(Channel {
                title: metadata.title,
                description: metadata.description.unwrap_or_default(),
                cover_url: first_url(metadata.avatar).or_else(|| first_url(metadata.thumbnail)),
            })
        })
        .await
    }
}

fn first_url(thumbnails: Option<Vec<Thumbnail>>) -> Option<String> {
    thumbnails?
        .into_iter()
        .next()
        .map(|thumbnail| thumbnail.url)
}

pub(super) async fn page_data(page: &Playlist) -> Result<Page> {
    let data = page.data().await?;
    let suspicious = data.alerts.iter().any(|alert| match alert {
        PlaylistAlert::Alert(alert) | PlaylistAlert::AlertWithButton(alert) => {
            alert.alert_type != "INFO"
        }
        PlaylistAlert::Other { .. } => false,
    });
    Ok(Page {
        title: data.info.title,
        description: data.info.description.unwrap_or_default(),
        cover_url: first_url(data.info.thumbnails),
        count: data.info.total_items,
        continuation: data.has_continuation,
        suspicious,
        videos: data.items.into_iter().map(entry).collect::<Result<_>>()?,
    })
}

pub(super) fn entry(item: PlaylistItem) -> Result<ListingVideo> {
    match item {
        PlaylistItem::PlaylistVideo(item) => Ok(ListingVideo {
            video: Video {
                id: item.id,
                title: item.title.into_string(),
                description: String::new(),
                published: None,
                duration: item.duration.seconds,
                available: item.is_playable
                    && !item.is_live
                    && !item.is_upcoming
                    && item.upcoming.is_none(),
            },
            published_text: Some(item.video_info.into_string()),
        }),
        PlaylistItem::LockupView(item)
            if matches!(
                item.content_type,
                LockupContentType::Video | LockupContentType::Short
            ) =>
        {
            let mut title = None;
            let mut published_text = None;
            if let Some(metadata) = item.metadata {
                title = metadata
                    .title
                    .map(|text| text.into_string())
                    .filter(|text| !text.is_empty());
                published_text = metadata
                    .metadata
                    .and_then(|content| content.metadata_rows.into_iter().last())
                    .and_then(|row| row.metadata_parts)
                    .and_then(|parts| parts.into_iter().last())
                    .and_then(|part| part.text)
                    .map(|text| text.into_string())
                    .filter(|text| !text.is_empty());
            }
            let available = title.is_some() && !live_badge(item.content_image.as_ref());
            Ok(ListingVideo {
                video: Video {
                    title: title.unwrap_or_else(|| item.content_id.clone()),
                    id: item.content_id,
                    description: String::new(),
                    published: None,
                    duration: 0.0,
                    available,
                },
                published_text,
            })
        }
        _ => bail!("Unsupported playlist item; listing is incomplete."),
    }
}

fn live_badge(image: Option<&ContentImage>) -> bool {
    let Some(ContentImage::ThumbnailView { overlays }) = image else {
        return false;
    };
    overlays.iter().any(|overlay| {
        let badges = match overlay {
            ThumbnailOverlay::ThumbnailOverlayBadgeView { badges }
            | ThumbnailOverlay::ThumbnailBottomOverlayView { badges } => badges,
            ThumbnailOverlay::Other { .. } => return false,
        };
        badges.iter().any(|badge| {
            badge.badge_style.as_deref() == Some("THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE")
                || badge
                    .text
                    .as_deref()
                    .is_some_and(|text| text.trim().eq_ignore_ascii_case("upcoming"))
        })
    })
}

pub(super) async fn snapshot(source: &mut impl Source, kind: &str, id: &str) -> Result<Snapshot> {
    let observed_at = chrono::Utc::now();
    let mut videos = Vec::new();
    let mut ids = HashSet::new();
    let mut pages = HashSet::new();
    let mut count = 0;
    let mut expected = None;
    let mut title = String::new();
    let mut description = String::new();
    let mut cover_url = None;
    loop {
        let page_number = pages.len() + 1;
        log::info!("Scanning page source={kind}:{id} page={page_number}");
        let page = source
            .next_page()
            .await
            .with_context(|| format!("Scan source={kind}:{id} page={page_number} failed"))?;
        ensure!(
            !page.suspicious,
            "Incomplete YouTube listing; previous feed and files preserved"
        );
        if pages.is_empty() {
            title = page.title.unwrap_or_else(|| id.into());
            description = page.description;
            cover_url = page.cover_url;
            expected = page
                .count
                .as_deref()
                .and_then(|text| text.split_whitespace().next())
                .and_then(|text| text.replace(',', "").parse::<usize>().ok());
        }
        let fingerprint = page
            .videos
            .iter()
            .map(|v| v.video.id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        ensure!(
            pages.insert(fingerprint) && pages.len() < 10_000,
            "Repeated YouTube pagination; no files removed"
        );
        count += page.videos.len();
        log::info!(
            "Scanned page source={kind}:{id} page={page_number} entries={} total={count} continuation={}",
            page.videos.len(),
            page.continuation
        );
        for entry in page.videos {
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
        if !page.continuation {
            break;
        }
    }
    ensure!(
        expected == Some(count),
        "Incomplete YouTube listing ({count} entries); previous feed and files preserved"
    );
    if kind == "channel" {
        let channel = source.channel(id).await?;
        title = channel.title.unwrap_or(title);
        description = channel.description;
        cover_url = channel.cover_url;
    }
    Ok(Snapshot {
        title,
        description,
        cover_url,
        videos,
    })
}
