use anyhow::{Context, Result, ensure};
use futures::{StreamExt, stream};
use reqwest::{Client, header};
use std::{io::SeekFrom, path::Path, time::Duration};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::youtube::MediaRequest;

#[derive(Clone, Copy)]
struct DownloadOptions {
    chunk_bytes: usize,
    concurrency: usize,
    chunk_timeout: Duration,
    total_timeout: Duration,
}

const DOWNLOAD_OPTIONS: DownloadOptions = DownloadOptions {
    // Small parallel ranges allow cheap retries and avoid one connection limiting throughput.
    chunk_bytes: 1024 * 1024,
    concurrency: 4,
    chunk_timeout: Duration::from_secs(30),
    total_timeout: Duration::from_secs(5 * 60),
};

pub async fn download(client: &Client, request: &MediaRequest, path: &Path) -> Result<u64> {
    let url = url::Url::parse(&request.url)?;
    ensure!(
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| host.ends_with(".googlevideo.com")),
        "Invalid audio download host"
    );
    ensure!(
        request.mime_type.starts_with("audio/"),
        "Expected an audio-only format"
    );
    ensure!(
        request.bytes > 0 && request.bytes <= 8 * 1024 * 1024 * 1024,
        "Audio length is missing or exceeds the 8 GiB limit"
    );
    download_ranges(client, request, &url, path, DOWNLOAD_OPTIONS).await
}

async fn download_ranges(
    client: &Client,
    request: &MediaRequest,
    url: &url::Url,
    path: &Path,
    options: DownloadOptions,
) -> Result<u64> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await?;
    let result = tokio::time::timeout(options.total_timeout, async {
        let mut chunks = stream::iter((0..request.bytes).step_by(options.chunk_bytes))
            .map(|start| async move {
                let end = (start + options.chunk_bytes as u64).min(request.bytes) - 1;
                let bytes = download_chunk(client, request, url, start, end, options).await?;
                Ok::<_, anyhow::Error>((start, bytes))
            })
            .buffer_unordered(options.concurrency);
        let mut count = 0;
        while let Some(chunk) = chunks.next().await {
            let (start, bytes) = chunk?;
            file.seek(SeekFrom::Start(start)).await?;
            file.write_all(&bytes).await?;
            count += bytes.len() as u64;
        }
        file.sync_all().await?;
        ensure!(count == request.bytes, "Incomplete audio download");
        Ok(count)
    })
    .await
    .context("Audio download exceeded its time limit; will resolve a fresh URL and retry")
    .and_then(|result| result);
    drop(file);
    if result.is_err() {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}

async fn download_chunk(
    client: &Client,
    request: &MediaRequest,
    url: &url::Url,
    start: u64,
    end: u64,
    options: DownloadOptions,
) -> Result<Vec<u8>> {
    let mut url = url.clone();
    // YouTube.js uses the CDN's range query parameter for audio, not a Range header.
    let query = url
        .query_pairs()
        .filter(|(key, _)| key != "range")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(query)
        .append_pair("range", &format!("{start}-{end}"));
    for attempt in 1..=3 {
        let result = fetch_chunk(client, request, &url, start, end, options.chunk_timeout).await;
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(error) if attempt == 3 => {
                return Err(error).context("Audio range failed after 3 attempts");
            }
            Err(error) => {
                log::warn!("Audio range {start}-{end} attempt={attempt}/3 failed: {error:#}");
                tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
            }
        }
    }
    unreachable!()
}

async fn fetch_chunk(
    client: &Client,
    request: &MediaRequest,
    url: &url::Url,
    start: u64,
    end: u64,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let response = client
        .get(url.clone())
        .header(header::USER_AGENT, &request.user_agent)
        .header(header::ACCEPT, "*/*")
        .header(header::ACCEPT_ENCODING, "identity")
        .header(header::ORIGIN, "https://www.youtube.com")
        .header(header::REFERER, "https://www.youtube.com/")
        // This deadline includes the body, so a trickling server cannot hold a slot forever.
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| error.without_url())?;
    let status = response.status().as_u16();
    ensure!(
        status == 200 || status == 206,
        "Audio download failed (HTTP {status})"
    );
    let length = end - start + 1;
    if status == 206 || response.headers().contains_key(header::CONTENT_RANGE) {
        let wanted = format!("bytes {start}-{end}/{}", request.bytes);
        ensure!(
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                == Some(wanted.as_str()),
            "Audio server returned an unexpected byte range"
        );
    }
    ensure!(
        response
            .content_length()
            .is_none_or(|actual| actual == length),
        "Incorrect audio response length"
    );
    let mut bytes = Vec::with_capacity(length as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.without_url())?;
        ensure!(
            bytes.len() as u64 + chunk.len() as u64 <= length,
            "Audio response exceeds its byte range"
        );
        bytes.extend_from_slice(&chunk);
    }
    ensure!(bytes.len() as u64 == length, "Incomplete audio range");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::{io::AsyncReadExt, net::TcpListener, task::JoinHandle};

    #[derive(Clone, Copy)]
    enum Behavior {
        Normal,
        PartialStatus,
        RetryMiddle,
        WrongRange,
        IgnoreRange,
        Oversized,
        Truncated,
        Trickle,
    }

    struct Server {
        url: url::Url,
        requests: Arc<Mutex<Vec<(u64, u64)>>>,
        completed: Arc<Mutex<Vec<u64>>>,
        task: JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn server(behavior: Behavior) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = url::Url::parse(&format!(
            "http://{}/audio?cpn=test",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let completed = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let finished = completed.clone();
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept() => {
                        let (mut socket, _) = connection.unwrap();
                        let seen = seen.clone();
                        let finished = finished.clone();
                        connections.spawn(async move {
                            let mut header = Vec::new();
                            loop {
                                let mut byte = [0];
                                if socket.read_exact(&mut byte).await.is_err() { return; }
                                header.push(byte[0]);
                                if header.ends_with(b"\r\n\r\n") { break; }
                            }
                            let header = String::from_utf8(header).unwrap();
                            let path = header.split_whitespace().nth(1).unwrap();
                            let url = url::Url::parse(&format!("http://localhost{path}")).unwrap();
                            assert!(url.query_pairs().any(|(key, value)| key == "cpn" && value == "test"));
                            assert!(header.to_ascii_lowercase().contains("accept-encoding: identity"));
                            let range = url.query_pairs().find(|(key, _)| key == "range").unwrap().1;
                            let (start, end) = range.split_once('-').unwrap();
                            let start: u64 = start.parse().unwrap();
                            let end: u64 = end.parse().unwrap();
                            let attempt = {
                                let mut seen = seen.lock().unwrap();
                                seen.push((start, end));
                                seen.iter().filter(|(offset, _)| *offset == start).count()
                            };
                            if matches!(behavior, Behavior::RetryMiddle) && start == 4 && attempt == 1 {
                                let _ = socket.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                                return;
                            }
                            // The first range finishes last, exercising writes by offset.
                            if start == 0 { tokio::time::sleep(Duration::from_millis(50)).await; }
                            let data = b"abcdefghijklmn";
                            let status = if matches!(behavior, Behavior::PartialStatus | Behavior::WrongRange) { 206 } else { 200 };
                            let content_range = match behavior {
                                Behavior::PartialStatus => format!("Content-Range: bytes {start}-{end}/14\r\n"),
                                Behavior::WrongRange => "Content-Range: bytes 0-3/99\r\n".into(),
                                _ => String::new(),
                            };
                            let mut bytes = data[start as usize..=end as usize].to_vec();
                            if matches!(behavior, Behavior::IgnoreRange) { bytes = data.to_vec(); }
                            let length = bytes.len();
                            if matches!(behavior, Behavior::Oversized) { bytes.push(b'!'); }
                            if matches!(behavior, Behavior::Truncated) { bytes.pop(); }
                            let content_length = if matches!(behavior, Behavior::Oversized) {
                                String::new()
                            } else {
                                format!("Content-Length: {length}\r\n")
                            };
                            let header = format!("HTTP/1.1 {status} OK\r\n{content_length}{content_range}Connection: close\r\n\r\n");
                            if socket.write_all(header.as_bytes()).await.is_err() { return; }
                            if matches!(behavior, Behavior::Trickle) {
                                for byte in bytes {
                                    if socket.write_all(&[byte]).await.is_err() { return; }
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            } else {
                                let _ = socket.write_all(&bytes).await;
                            }
                            finished.lock().unwrap().push(start);
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => { result.unwrap(); }
                }
            }
        });
        Server {
            url,
            requests,
            completed,
            task,
        }
    }

    fn request(url: &url::Url) -> MediaRequest {
        serde_json::from_value(serde_json::json!({
            "url": url.as_str(), "bytes": 14, "itag": 140,
            "mime_type": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 128000,
            "user_agent": "VISIONOS", "title": "Audio", "description": "",
            "duration": 1, "published": null
        }))
        .unwrap()
    }

    const TEST_OPTIONS: DownloadOptions = DownloadOptions {
        chunk_bytes: 4,
        concurrency: 4,
        chunk_timeout: Duration::from_secs(2),
        total_timeout: Duration::from_secs(5),
    };

    #[tokio::test]
    async fn parallel_ranges_are_reassembled_exactly_for_200_and_206() {
        for behavior in [Behavior::Normal, Behavior::PartialStatus] {
            let server = server(behavior).await;
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("audio");
            let bytes = download_ranges(
                &Client::new(),
                &request(&server.url),
                &server.url,
                &path,
                TEST_OPTIONS,
            )
            .await
            .unwrap();
            assert_eq!(bytes, 14);
            assert_eq!(std::fs::read(&path).unwrap(), b"abcdefghijklmn");
            let mut ranges = server.requests.lock().unwrap().clone();
            ranges.sort();
            assert_eq!(ranges, [(0, 3), (4, 7), (8, 11), (12, 13)]);
            assert_ne!(server.completed.lock().unwrap()[0], 0);
        }
    }

    #[tokio::test]
    async fn retries_only_the_failed_range() {
        let server = server(Behavior::RetryMiddle).await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audio");
        download_ranges(
            &Client::new(),
            &request(&server.url),
            &server.url,
            &path,
            TEST_OPTIONS,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"abcdefghijklmn");
        let mut ranges = server.requests.lock().unwrap().clone();
        ranges.sort();
        assert_eq!(ranges, [(0, 3), (4, 7), (4, 7), (8, 11), (12, 13)]);
    }

    #[tokio::test]
    async fn invalid_responses_fail_and_remove_partial_files() {
        for behavior in [
            Behavior::WrongRange,
            Behavior::IgnoreRange,
            Behavior::Oversized,
            Behavior::Truncated,
        ] {
            let server = server(behavior).await;
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("audio");
            assert!(
                download_ranges(
                    &Client::new(),
                    &request(&server.url),
                    &server.url,
                    &path,
                    TEST_OPTIONS
                )
                .await
                .is_err()
            );
            assert!(!path.exists());
            assert!(server.requests.lock().unwrap().len() <= 12);
        }
    }

    #[tokio::test]
    async fn trickling_chunks_time_out_and_retry_with_a_fixed_budget() {
        let server = server(Behavior::Trickle).await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audio");
        let options = DownloadOptions {
            chunk_timeout: Duration::from_millis(150),
            ..TEST_OPTIONS
        };
        let error = download_ranges(
            &Client::new(),
            &request(&server.url),
            &server.url,
            &path,
            options,
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("after 3 attempts"));
        assert!(!path.exists());
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(start, _)| *start == 4)
                .count()
                >= 2
        );
    }

    #[tokio::test]
    async fn total_deadline_cancels_remaining_ranges_and_removes_partial_files() {
        let server = server(Behavior::Trickle).await;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audio");
        let options = DownloadOptions {
            total_timeout: Duration::from_millis(150),
            ..TEST_OPTIONS
        };
        let error = download_ranges(
            &Client::new(),
            &request(&server.url),
            &server.url,
            &path,
            options,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exceeded its time limit"));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn public_downloader_rejects_video_before_creating_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("audio");
        let mut request =
            request(&url::Url::parse("https://example.googlevideo.com/audio").unwrap());
        request.mime_type = "video/mp4".into();
        let error = download(&Client::new(), &request, &path).await.unwrap_err();
        assert_eq!(error.to_string(), "Expected an audio-only format");
        assert!(!path.exists());
    }
}
