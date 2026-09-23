import type { Misc, YT } from "youtubei.js";

export function selectDownloadableAudio(
  info: Pick<
    YT.VideoInfo,
    "basic_info" | "playability_status" | "streaming_data"
  >,
) {
  if (
    info.basic_info.is_live ||
    info.basic_info.is_upcoming ||
    info.playability_status?.status === "LIVE_STREAM_OFFLINE"
  )
    return null;
  if (info.playability_status?.status !== "OK")
    throw new Error(
      `YouTube playback unavailable: ${info.playability_status?.reason || "unknown reason"}`,
    );
  return selectAudioFormat(info.streaming_data?.adaptive_formats || []);
}

export function selectAudioFormat(formats: Misc.Format[]) {
  const audio = formats.filter(
    (item) =>
      item.has_audio &&
      !item.has_video &&
      item.mime_type.startsWith("audio/") &&
      !item.drm_families?.length &&
      !item.is_type_otf &&
      Number.isSafeInteger(item.content_length) &&
      item.content_length! > 0 &&
      item.content_length! <= 8 * 1024 * 1024 * 1024 &&
      !!(item.url || item.cipher || item.signature_cipher),
  );
  const m4a = audio.filter(
    (item) =>
      item.mime_type.startsWith("audio/mp4;") &&
      item.mime_type.includes("mp4a"),
  );
  const selected = (m4a.length ? m4a : audio).sort(
    (a, b) => b.bitrate - a.bitrate,
  )[0];
  if (!selected) throw new Error("No downloadable audio-only format.");
  return selected;
}
