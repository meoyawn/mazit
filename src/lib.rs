pub mod audio;
pub mod config;
pub mod database;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod downloads;
#[cfg(feature = "desktop")]
pub mod editor;
pub mod engine;
pub mod logging;
pub mod network;
pub mod rss;
pub mod storage;
pub mod youtube;

pub fn redact(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            if word.contains("http://") || word.contains("https://") {
                "[URL]"
            } else {
                word
            }
        })
        .take(100)
        .collect::<Vec<_>>()
        .join(" ")
}
