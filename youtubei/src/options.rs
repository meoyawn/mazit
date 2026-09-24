use crate::{Cache, Engine, FetchFunction, JsValue, Result};
use serde::Serialize;

/// Upstream SessionOptions / InnerTubeConfig. `None` leaves a property absent,
/// so defaults are selected by youtubei.js, not reimplemented in this crate.
#[derive(Clone, Default, Serialize)]
pub struct SessionOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_behalf_of_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieve_player: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_safety_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieve_innertube_config: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generate_session_locally: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_fast: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_session_cache: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visitor_data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub po_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_id: Option<String>,
    #[serde(skip)]
    pub cache: Option<Cache>,
    #[serde(skip)]
    pub fetch: Option<FetchFunction>,
}

impl SessionOptions {
    /// Explicit network-free creation; useful for fixtures and flat listing work.
    pub fn local() -> Self {
        Self {
            generate_session_locally: Some(true),
            retrieve_player: Some(false),
            retrieve_innertube_config: Some(false),
            ..Self::default()
        }
    }

    pub async fn to_value(&self, engine: &Engine) -> Result<JsValue> {
        let value = engine.value(serde_json::to_value(self)?).await?;
        if let Some(cache) = &self.cache {
            value.set("cache", cache).await?;
        }
        if let Some(fetch) = &self.fetch {
            value.set("fetch", fetch).await?;
        }
        Ok(value)
    }
}

#[derive(Clone, Default, Serialize)]
pub struct GetVideoInfoOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<Client>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub po_token: Option<String>,
}

/// Upstream GetVideoInfoOptions.client / InnerTubeClient.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum Client {
    #[serde(rename = "IOS")]
    Ios,
    #[serde(rename = "WEB")]
    Web,
    #[serde(rename = "MWEB")]
    Mweb,
    #[serde(rename = "ANDROID")]
    Android,
    #[serde(rename = "ANDROID_VR")]
    AndroidVr,
    #[serde(rename = "VISIONOS")]
    VisionOs,
    #[serde(rename = "YTMUSIC")]
    YtMusic,
    #[serde(rename = "YTMUSIC_ANDROID")]
    YtMusicAndroid,
    #[serde(rename = "YTSTUDIO_ANDROID")]
    YtStudioAndroid,
    #[serde(rename = "TV")]
    Tv,
    #[serde(rename = "TV_SIMPLY")]
    TvSimply,
    #[serde(rename = "TV_EMBEDDED")]
    TvEmbedded,
    #[serde(rename = "YTKIDS")]
    YtKids,
    #[serde(rename = "WEB_EMBEDDED")]
    WebEmbedded,
    #[serde(rename = "WEB_CREATOR")]
    WebCreator,
}

impl Client {
    /// Read the installed library's value. Some clients do not specify an agent.
    pub async fn user_agent(self, engine: &Engine) -> Result<Option<String>> {
        let name = serde_json::to_value(self)?;
        let key = if matches!(self, Self::YtKids) {
            "WEB_KIDS"
        } else {
            name.as_str().unwrap()
        };
        engine
            .export(&["Constants", "CLIENTS", key, "USER_AGENT"])
            .await?
            .read()
            .await
    }
}

/// The browse request fields used for flat playlist retrieval.
#[derive(Clone, Debug, Serialize)]
pub struct BrowseOptions {
    #[serde(rename = "browseId")]
    pub browse_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<String>,
}
