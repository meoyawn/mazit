use anyhow::{Result, ensure};
use futures::StreamExt;
use reqwest::Client;
use youtubei::{FetchRequest, FetchResponse};

pub(super) async fn fetch(client: Client, input: FetchRequest) -> Result<FetchResponse> {
    let url = url::Url::parse(&input.url)?;
    let host = url.host_str().unwrap_or("");
    ensure!(
        url.scheme() == "https"
            && [
                "youtube.com",
                "google.com",
                "googleapis.com",
                "googlevideo.com",
                "ytimg.com"
            ]
            .iter()
            .any(|base| host == *base || host.ends_with(&format!(".{base}"))),
        "Unexpected YouTube API host"
    );
    let mut request = client.request(input.method.parse()?, url);
    for (key, value) in input.headers {
        // The app's jar owns consent and imported cookies, including redirects.
        if !["host", "content-length", "cookie"].contains(&key.to_ascii_lowercase().as_str()) {
            request = request.header(key, value);
        }
    }
    if let Some(body) = input.body {
        request = request.body(body);
    }
    let response = request.send().await.map_err(|e| e.without_url())?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.to_string(), value.to_owned()))
        })
        .collect();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.without_url())?;
        ensure!(
            body.len() + chunk.len() <= 32 * 1024 * 1024,
            "YouTube metadata response exceeded size limit"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(FetchResponse {
        status,
        headers,
        body,
    })
}
