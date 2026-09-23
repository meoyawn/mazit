use anyhow::{Result, ensure};
use futures::StreamExt;
use reqwest::{
    Client,
    cookie::{CookieStore, Jar},
};
use std::{path::Path, sync::Arc, time::Duration};
mod audio_download;
pub use audio_download::download;

pub fn client(cookies: Option<&Path>) -> Result<(Client, Arc<Jar>)> {
    let jar = Arc::new(Jar::default());
    if let Some(path) = cookies {
        let text = std::fs::read_to_string(path)?;
        ensure!(
            text.len() < 16 * 1024 * 1024,
            "Cookie jar exceeds size limit"
        );
        for line in text.lines() {
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let parts: Vec<_> = line.splitn(7, '\t').collect();
            ensure!(parts.len() == 7, "Invalid Netscape cookie jar entry");
            let domain = parts[0].trim_start_matches('.');
            if !["youtube.com", "google.com", "googlevideo.com"]
                .iter()
                .any(|base| domain == *base || domain.ends_with(&format!(".{base}")))
            {
                continue;
            }
            let expires: i64 = parts[4].parse()?;
            if expires != 0 && expires < chrono::Utc::now().timestamp() {
                continue;
            }
            let origin = url::Url::parse(&format!("https://{domain}/"))?;
            let mut cookie = format!("{}={}; Path={}", parts[5], parts[6], parts[2]);
            if parts[1] == "TRUE" {
                cookie.push_str(&format!("; Domain={}", parts[0]));
            }
            if parts[3] == "TRUE" {
                cookie.push_str("; Secure");
            }
            if let Some(date) = chrono::DateTime::from_timestamp(expires, 0).filter(|_| expires > 0)
            {
                cookie.push_str(&format!(
                    "; Expires={}",
                    date.format("%a, %d %b %Y %H:%M:%S GMT")
                ));
            }
            jar.add_cookie_str(&cookie, &origin);
        }
        // The source jar is read-only. Set-Cookie updates stay in this process; no file can be corrupted.
    }
    // Reject optional cookies so anonymous requests receive channel HTML in consent regions.
    // Preserve an existing consent choice from an imported jar.
    if !youtube_cookie(&jar).split(';').any(|cookie| {
        cookie
            .trim()
            .strip_prefix("SOCS=")
            .is_some_and(|value| !value.starts_with("CAA"))
    }) {
        jar.add_cookie_str(
            "SOCS=CAE; Domain=.youtube.com; Path=/; Secure",
            &url::Url::parse("https://www.youtube.com/")?,
        );
    }
    let client = Client::builder()
        .cookie_provider(jar.clone())
        .user_agent("Mozilla/5.0")
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;
    Ok((client, jar))
}
pub fn youtube_cookie(jar: &Jar) -> String {
    jar.cookies(&url::Url::parse("https://www.youtube.com/").unwrap())
        .and_then(|v| v.to_str().ok().map(str::to_string))
        .unwrap_or_default()
}

#[derive(PartialEq, Eq)]
pub struct Cover {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub extension: &'static str,
}

pub async fn download_cover(client: &Client, url: &str) -> Result<Cover> {
    let url = url::Url::parse(url)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| {
                ["ytimg.com", "ggpht.com", "googleusercontent.com"]
                    .iter()
                    .any(|base| host == *base || host.ends_with(&format!(".{base}")))
            }),
        "Invalid cover download host"
    );
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "image/jpeg, image/png")
        .send()
        .await
        .map_err(|error| error.without_url())?
        .error_for_status()
        .map_err(|error| error.without_url())?;
    const MAX_BYTES: usize = 10 * 1024 * 1024;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_BYTES as u64),
        "Cover exceeds the 10 MiB limit"
    );
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.without_url())?;
        ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "Cover exceeds the 10 MiB limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Cover::from_bytes(bytes)
}

impl Cover {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        // The bytes determine the MIME type and suffix even for extensionless avatar URLs.
        let (mime, extension) = if bytes.starts_with(b"\xff\xd8\xff") {
            ("image/jpeg", "jpg")
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            ("image/png", "png")
        } else {
            anyhow::bail!("YouTube cover must be a JPEG or PNG image");
        };
        Ok(Self {
            bytes,
            mime,
            extension,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_youtube_requests_reject_optional_cookies() {
        let (_, jar) = client(None).unwrap();
        assert_eq!(youtube_cookie(&jar), "SOCS=CAE");
        for origin in [
            "https://www.youtube.com/",
            "https://m.youtube.com/",
            "https://consent.youtube.com/",
        ] {
            assert!(jar.cookies(&url::Url::parse(origin).unwrap()).is_some());
        }
        for origin in ["http://www.youtube.com/", "https://example.com/"] {
            assert!(jar.cookies(&url::Url::parse(origin).unwrap()).is_none());
        }
    }

    #[test]
    fn imported_youtube_consent_is_preserved_without_changing_the_file() {
        for (stored, expected) in [("CAE", "CAE"), ("CAI", "CAI"), ("CAA", "CAE")] {
            let file = tempfile::NamedTempFile::new().unwrap();
            let contents = format!(".youtube.com\tTRUE\t/\tTRUE\t0\tSOCS\t{stored}\n");
            std::fs::write(file.path(), &contents).unwrap();
            let (_, jar) = client(Some(file.path())).unwrap();
            assert_eq!(youtube_cookie(&jar), format!("SOCS={expected}"));
            assert_eq!(std::fs::read_to_string(file.path()).unwrap(), contents);
        }
    }

    #[test]
    fn cover_uses_image_bytes_for_type_and_extension() {
        for (bytes, mime, extension) in [
            (b"\xff\xd8\xff\xe0".to_vec(), "image/jpeg", "jpg"),
            (b"\x89PNG\r\n\x1a\n".to_vec(), "image/png", "png"),
        ] {
            let image = Cover::from_bytes(bytes.clone()).unwrap();
            assert_eq!(image.bytes, bytes);
            assert_eq!(image.mime, mime);
            assert_eq!(image.extension, extension);
        }
        assert!(Cover::from_bytes(b"<html>Access denied</html>".to_vec()).is_err());
        assert!(Cover::from_bytes(Vec::new()).is_err());
    }

    #[tokio::test]
    async fn cover_rejects_untrusted_download_hosts() {
        let client = Client::new();
        for url in [
            "http://i.ytimg.com/cover.jpg",
            "https://ytimg.com.example.com/cover.jpg",
            "https://example.com/cover.jpg",
        ] {
            let result = download_cover(&client, url).await;
            assert!(result.is_err_and(|error| error.to_string() == "Invalid cover download host"));
        }
    }
}
