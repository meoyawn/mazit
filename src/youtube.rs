use serde::{Deserialize, Serialize};

mod channel;
mod client;
mod dates;
#[cfg(test)]
mod fixtures;
mod listing;
mod media;
#[cfg(test)]
mod native_tests;
mod transport;

#[derive(Clone)]
pub struct YouTube {
    sender: youtubei::Worker<client::Request>,
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

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    #[ignore = "live YouTube check using the application's add and flat listing paths"]
    async fn live_handle_subscription_through_app_client() {
        let directory = tempfile::tempdir().unwrap();
        let core = crate::engine::Core::new(directory.path().to_path_buf(), None).unwrap();
        for url in [
            "https://www.youtube.com/@RyanFleury/videos",
            "https://www.youtube.com/@RyanFleury/",
        ] {
            let id = core.add(url).await.unwrap();
            assert_eq!(id, "channel:UCCsdwE2z_kRL3vfVsd5OGyA");
        }
        let sources = core.db.sources().unwrap();
        assert_eq!(sources.len(), 1);
        let source = &sources[0];
        assert_eq!(source.kind, "channel");
        assert_eq!(source.youtube_id, "UCCsdwE2z_kRL3vfVsd5OGyA");
        assert_eq!(
            source.url,
            "https://www.youtube.com/channel/UCCsdwE2z_kRL3vfVsd5OGyA"
        );
        let snapshot = core
            .youtube
            .snapshot(&source.kind, &source.youtube_id)
            .await
            .unwrap();
        assert_eq!(snapshot.title, "Ryan Fleury");
        assert!(!snapshot.videos.is_empty());
        eprintln!("Live channel listing: {} videos", snapshot.videos.len());
        for video in snapshot.videos.iter().filter(|video| !video.available) {
            eprintln!("Skipped in live listing: {} ({})", video.title, video.id);
        }
    }

    #[tokio::test]
    async fn handle_urls_share_the_canonical_channel_identity() {
        let id = "UCCsdwE2z_kRL3vfVsd5OGyA";
        let urls = [
            "https://www.youtube.com/@RyanFleury/videos",
            "https://www.youtube.com/@RyanFleury/",
        ];
        let youtube = YouTube::with_responses(
            urls.iter()
                .map(|url| ("resolve", json!({"url": url}), json!({"id": id})))
                .collect(),
        );
        for url in urls {
            assert_eq!(
                youtube.resolve(url).await.unwrap(),
                (
                    "channel".into(),
                    id.into(),
                    format!("https://www.youtube.com/channel/{id}")
                )
            );
        }
    }

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
        let media = youtube.media("FAaMG_3Lwug").await.unwrap().unwrap();
        assert!(media.published.is_none());
        assert_eq!(media.user_agent, "VISIONOS");
    }
}
