use crate::database::{Episode, Source};
use anyhow::{Context, Result};

fn xml(text: &str) -> String {
    text.chars()
        .filter(|c| *c >= ' ' || ['\n', '\r', '\t'].contains(c))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn render(
    source: &Source,
    episodes: &[Episode],
    feed_url: &str,
    cover_url: Option<&str>,
) -> Result<String> {
    let mut feed = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\"><channel><title>{}</title><link>{}</link><description>{}</description><language>en</language><ttl>60</ttl><lastBuildDate>{}</lastBuildDate><atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>",
        xml(&source.title),
        xml(&source.url),
        xml(&source.description),
        chrono::Utc::now().to_rfc2822(),
        xml(feed_url)
    );
    feed.push_str("<itunes:type>episodic</itunes:type>");
    if let Some(cover_url) = cover_url {
        feed.push_str(&format!(
            "<image><url>{}</url><title>{}</title><link>{}</link></image><itunes:image href=\"{}\"/>",
            xml(cover_url),
            xml(&source.title),
            xml(&source.url),
            xml(cover_url),
        ));
    }
    let mut episodes: Vec<_> = episodes
        .iter()
        .filter(|e| e.present && e.state == "uploaded")
        .map(|episode| {
            let date = episode
                .video
                .published
                .as_deref()
                .and_then(crate::youtube::parse_publication_date)
                .with_context(|| format!("Missing publication date for {}", episode.video.id))?;
            Ok((episode, date))
        })
        .collect::<Result<_>>()?;
    episodes.sort_by(|(left, left_date), (right, right_date)| {
        right_date
            .cmp(left_date)
            .then_with(|| left.position.cmp(&right.position))
            .then_with(|| left.video.id.cmp(&right.video.id))
    });
    for (episode, date) in episodes {
        let video = &episode.video;
        feed.push_str(&format!("<item><title>{}</title><description>{}</description><guid isPermaLink=\"false\">{}/{}</guid><link>https://www.youtube.com/watch?v={}</link><pubDate>{}</pubDate><itunes:duration>{}</itunes:duration><enclosure url=\"{}\" length=\"{}\" type=\"audio/mp4\"/></item>",xml(&video.title),xml(&video.description),xml(&source.id),xml(&video.id),xml(&video.id),date.to_rfc2822(),video.duration.round() as u64,xml(episode.public_url.as_deref().unwrap_or("")),episode.bytes));
    }
    feed.push_str("</channel></rss>\n");
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::Database, youtube::Video};
    use std::path::Path;

    fn source(kind: &str) -> Source {
        let db = Database::open(Path::new(":memory:")).unwrap();
        let id = db
            .add(kind, "test", "https://www.youtube.com/playlist?list=test")
            .unwrap();
        db.source(&id).unwrap()
    }

    fn episode(id: &str, position: usize, date: &str) -> Episode {
        Episode {
            video: Video {
                id: id.into(),
                title: id.into(),
                description: "Cover & song".into(),
                published: Some(date.into()),
                duration: 300.0,
                available: true,
            },
            present: true,
            state: "uploaded".into(),
            bytes: 123,
            public_url: Some(format!("https://audio.example.com/{id}.m4a")),
            position,
        }
    }

    #[test]
    fn feeds_are_newest_first_by_timestamp_without_seasons_or_episode_numbers() {
        let mut skipped = episode("skipped", 1, "invalid");
        skipped.state = "skipped".into();
        let mut removed = episode("removed", 2, "invalid");
        removed.present = false;
        for kind in ["playlist", "channel"] {
            let feed = render(
                &source(kind),
                &[
                    episode("oldest", 0, "2010-02-22"),
                    skipped.clone(),
                    removed.clone(),
                    episode("morning", 3, "2011-02-22T10:00:00Z"),
                    episode("newest", 5, "2012-01-11"),
                    episode("afternoon", 4, "2011-02-22T05:00:00-08:00"),
                ],
                "https://audio.example.com/rss.xml",
                None,
            )
            .unwrap();
            assert!(feed.contains("<itunes:type>episodic</itunes:type>"));
            assert!(!feed.contains("<itunes:season>"));
            assert!(!feed.contains("<itunes:episode>"));
            let items: Vec<_> = feed.split("<item>").skip(1).collect();
            assert_eq!(items.len(), 4);
            for (item, title) in items
                .iter()
                .zip(["newest", "afternoon", "morning", "oldest"])
            {
                assert!(item.starts_with(&format!("<title>{title}</title>")));
            }
            assert!(items[1].contains("<pubDate>Tue, 22 Feb 2011 13:00:00 +0000</pubDate>"));
            assert!(items[2].contains("<pubDate>Tue, 22 Feb 2011 10:00:00 +0000</pubDate>"));
            assert!(items[3].contains("<pubDate>Mon, 22 Feb 2010 00:00:00 +0000</pubDate>"));
        }
    }

    #[test]
    fn playlist_reordering_preserves_episode_metadata_and_identity() {
        let source = source("playlist");
        let mut episode = episode("FAaMG_3Lwug", 0, "2011-01-11");
        let first = render(
            &source,
            &[episode.clone()],
            "https://audio.example.com/rss.xml",
            None,
        )
        .unwrap();
        episode.position = 7;
        let reordered = render(
            &source,
            &[episode],
            "https://audio.example.com/rss.xml",
            None,
        )
        .unwrap();
        assert_eq!(
            first.split_once("<item>").unwrap().1,
            reordered.split_once("<item>").unwrap().1
        );
        assert!(reordered.contains("<guid isPermaLink=\"false\">playlist:test/FAaMG_3Lwug</guid>"));
    }

    #[test]
    fn channels_use_real_dates_and_episodic_ordering() {
        let feed = render(
            &source("channel"),
            &[episode("latest", 0, "2026-09-23")],
            "https://audio.example.com/rss.xml",
            None,
        )
        .unwrap();
        assert!(feed.contains("<itunes:type>episodic</itunes:type>"));
        assert!(!feed.contains("<itunes:episode>"));
        assert!(!feed.contains("<itunes:season>"));
        assert!(feed.contains("<pubDate>Wed, 23 Sep 2026 00:00:00 +0000</pubDate>"));
    }

    #[test]
    fn refuses_to_emit_unknown_or_invalid_publication_dates() {
        for date in [None, Some("unknown"), Some("2011-02-30")] {
            let mut episode = episode("missing", 0, "2011-01-11");
            episode.video.published = date.map(str::to_owned);
            assert!(
                render(
                    &source("playlist"),
                    &[episode],
                    "https://audio.example.com/rss.xml",
                    None
                )
                .is_err()
            );
        }
    }
}
