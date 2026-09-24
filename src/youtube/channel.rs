use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use reqwest::Client;
use std::sync::LazyLock;

static CHANNEL_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/(?:@[^/]+|(?:channel|c|user)/[^/]+)(?:/[^/]*)?/?$").unwrap());
static LINKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<link\b[^>]*>").unwrap());
static REL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\brel\s*=\s*["']canonical["']"#).unwrap());
static HREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhref\s*=\s*["']([^"']+)["']"#).unwrap());
static CANONICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://www\.youtube\.com/channel/(UC[A-Za-z0-9_-]{22})/?$").unwrap()
});

pub(super) fn page_url(input: &str) -> Result<url::Url> {
    let mut page = url::Url::parse(input).context("Enter a YouTube channel URL")?;
    ensure!(
        ["http", "https"].contains(&page.scheme())
            && ["www.youtube.com", "youtube.com", "m.youtube.com"]
                .contains(&page.host_str().unwrap_or(""))
            && CHANNEL_PATH.is_match(page.path()),
        "Enter a YouTube channel URL"
    );
    page.set_scheme("https")
        .map_err(|_| anyhow::anyhow!("Invalid channel URL"))?;
    page.set_host(Some("www.youtube.com"))?;
    Ok(page)
}

pub(super) fn from_html(html: &str) -> Result<String> {
    let canonical = LINKS
        .find_iter(html)
        .map(|tag| tag.as_str())
        .find(|tag| REL.is_match(tag))
        .and_then(|tag| HREF.captures(tag))
        .and_then(|c| c.get(1));
    if let Some(id) = canonical.and_then(|url| CANONICAL.captures(url.as_str())) {
        return Ok(id[1].to_owned());
    }
    if html.contains("https://consent.youtube.com/") {
        bail!("YouTube returned a consent page instead of the channel");
    }
    bail!("Channel not found in YouTube page")
}

pub(super) async fn resolve(client: &Client, input: &str) -> Result<String> {
    let response = client
        .get(page_url(input)?)
        .send()
        .await
        .map_err(|e| e.without_url())?;
    ensure!(
        response.status().is_success(),
        "YouTube channel page returned HTTP {}",
        response.status().as_u16()
    );
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.without_url())?;
        ensure!(
            body.len() + chunk.len() <= 32 * 1024 * 1024,
            "YouTube channel page exceeded size limit"
        );
        body.extend_from_slice(&chunk);
    }
    from_html(&String::from_utf8_lossy(&body))
}
