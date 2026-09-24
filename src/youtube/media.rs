use anyhow::{Result, bail};
use youtubei::{
    Client, Format, GetVideoInfoOptions, Innertube, Player, VideoInfo,
    models::{Microformat, VideoInfoData},
};

use super::MediaRequest;

pub(super) async fn select(info: &VideoInfo, data: &VideoInfoData) -> Result<Option<Format>> {
    let status = data
        .playability_status
        .as_ref()
        .map(|playback| playback.status.as_str());
    if data.basic_info.is_live == Some(true)
        || data.basic_info.is_upcoming == Some(true)
        || status == Some("LIVE_STREAM_OFFLINE")
    {
        return Ok(None);
    }
    if status != Some("OK") {
        let reason = data
            .playability_status
            .as_ref()
            .and_then(|playback| playback.reason.as_deref());
        bail!(
            "YouTube playback unavailable: {}",
            reason.unwrap_or("unknown reason")
        );
    }
    select_format(info.adaptive_formats().await?).map(Some)
}

pub(super) fn select_format(formats: Vec<Format>) -> Result<Format> {
    let mut best: Option<((bool, f64), Format)> = None;
    for format in formats {
        let data = format.info();
        let mime = &data.mime_type;
        let length = data.content_length.unwrap_or_default();
        if !data.has_audio
            || data.has_video
            || !mime.starts_with("audio/")
            || data
                .drm_families
                .as_ref()
                .is_some_and(|drm| !drm.is_empty())
            || data.is_type_otf
            || !length.is_finite()
            || length.fract() != 0.0
            || length <= 0.0
            || length > 8.0 * 1024.0 * 1024.0 * 1024.0
            || ![&data.url, &data.cipher, &data.signature_cipher]
                .iter()
                .any(|value| value.as_ref().is_some_and(|value| !value.is_empty()))
        {
            continue;
        }
        let rank = (
            mime.starts_with("audio/mp4;") && mime.contains("mp4a"),
            data.bitrate,
        );
        if best.as_ref().is_none_or(|(previous, _)| rank > *previous) {
            best = Some((rank, format));
        }
    }
    best.map(|(_, format)| format)
        .ok_or_else(|| anyhow::anyhow!("No downloadable audio-only format."))
}

pub(super) async fn resolve(
    yt: &Innertube,
    id: &str,
    player: &tokio::sync::OnceCell<Player>,
) -> Result<Option<MediaRequest>> {
    let info = yt
        .get_basic_info(
            id,
            GetVideoInfoOptions {
                client: Some(Client::VisionOs),
                ..Default::default()
            },
        )
        .await?;
    let data = info.data().await?;
    let Some(format) = select(&info, &data).await? else {
        return Ok(None);
    };
    let details = format.info();
    let session = yt.session().await?;
    let needs_player = details.cipher.is_some()
        || details.signature_cipher.is_some()
        || details
            .url
            .as_deref()
            .and_then(|url| url::Url::parse(url).ok())
            .is_some_and(|url| url.query_pairs().any(|(key, _)| key == "n"));
    if needs_player {
        let player = player
            .get_or_try_init(async || {
                let cache = session.cache().await?;
                let fetch = session.fetch().await?;
                Player::create(yt.engine(), cache.as_ref(), Some(&fetch), None, None).await
            })
            .await?;
        session.set_player(player).await?;
    }
    let mut url = url::Url::parse(&format.decipher(session.player().await?.as_ref()).await?)?;
    let pairs: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key != "cpn")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair("cpn", &data.cpn);
    let published = match data.microformat {
        Some(Microformat::PlayerMicroformat(microformat)) => microformat
            .publish_date
            .filter(|date| !date.is_empty())
            .or(microformat.upload_date),
        _ => None,
    };
    let basic = data.basic_info;
    let user_agent = Client::VisionOs
        .user_agent(yt.engine())
        .await?
        .ok_or_else(|| anyhow::anyhow!("VISIONOS client has no user agent"))?;
    Ok(Some(MediaRequest {
        url: url.into(),
        bytes: details.content_length.unwrap_or_default() as u64,
        itag: details.itag,
        mime_type: details.mime_type.clone(),
        bitrate: details.bitrate as u64,
        user_agent,
        title: basic
            .title
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.into()),
        description: basic.short_description.unwrap_or_default(),
        duration: basic.duration.unwrap_or_default(),
        published,
    }))
}
