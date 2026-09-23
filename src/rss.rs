use crate::database::{Episode, Source};

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
) -> String {
    let mut feed = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\"><channel><title>{}</title><link>{}</link><description>{}</description><language>en</language><ttl>60</ttl><lastBuildDate>{}</lastBuildDate><atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>",
        xml(&source.title),
        xml(&source.url),
        xml(&source.description),
        chrono::Utc::now().to_rfc2822(),
        xml(feed_url)
    );
    if let Some(cover_url) = cover_url {
        feed.push_str(&format!(
            "<image><url>{}</url><title>{}</title><link>{}</link></image><itunes:image href=\"{}\"/>",
            xml(cover_url),
            xml(&source.title),
            xml(&source.url),
            xml(cover_url),
        ));
    }
    for episode in episodes
        .iter()
        .filter(|e| e.present && e.state == "uploaded")
    {
        let video = &episode.video;
        let date = video
            .published
            .as_deref()
            .and_then(|d| {
                chrono::DateTime::parse_from_rfc3339(d)
                    .ok()
                    .map(|d| d.to_rfc2822())
                    .or_else(|| {
                        chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
                            .ok()
                            .and_then(|d| d.and_hms_opt(0, 0, 0))
                            .map(|d| d.and_utc().to_rfc2822())
                    })
            })
            .map(|d| format!("<pubDate>{d}</pubDate>"))
            .unwrap_or_default();
        feed.push_str(&format!("<item><title>{}</title><description>{}</description><guid isPermaLink=\"false\">{}/{}</guid><link>https://www.youtube.com/watch?v={}</link>{}<itunes:duration>{}</itunes:duration><enclosure url=\"{}\" length=\"{}\" type=\"audio/mp4\"/></item>",xml(&video.title),xml(&video.description),xml(&source.id),xml(&video.id),xml(&video.id),date,video.duration.round() as u64,xml(episode.public_url.as_deref().unwrap_or("")),episode.bytes));
    }
    feed.push_str("</channel></rss>\n");
    feed
}
