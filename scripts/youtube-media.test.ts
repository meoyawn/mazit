import { describe, expect, test } from "bun:test";
import { Misc } from "youtubei.js";
import { selectAudioFormat } from "../js/youtube-media.ts";

function format(overrides: Record<string, unknown> = {}) {
  return new Misc.Format({
    itag: 140,
    mimeType: 'audio/mp4; codecs="mp4a.40.2"',
    bitrate: 128000,
    audioQuality: "AUDIO_QUALITY_MEDIUM",
    contentLength: "12345",
    approxDurationMs: "1000",
    lastModified: "0",
    url: "https://example.googlevideo.com/audio",
    ...overrides,
  });
}

describe("audio download selection", () => {
  test("prefers the highest bitrate M4A over Opus and combined video", () => {
    const best = format({ itag: 141, bitrate: 256000 });
    expect(
      selectAudioFormat([
        format({
          itag: 18,
          mimeType: 'video/mp4; codecs="avc1.42001E, mp4a.40.2"',
          qualityLabel: "360p",
          bitrate: 1000000,
        }),
        format({
          itag: 251,
          mimeType: 'audio/webm; codecs="opus"',
          bitrate: 300000,
        }),
        format(),
        best,
      ]),
    ).toEqual(best);
  });

  test("falls back only to audio-only formats when M4A is unavailable", () => {
    const best = format({
      itag: 251,
      mimeType: 'audio/webm; codecs="opus"',
      bitrate: 160000,
    });
    expect(
      selectAudioFormat([
        format({
          itag: 250,
          mimeType: 'audio/webm; codecs="opus"',
          bitrate: 64000,
        }),
        best,
      ]),
    ).toEqual(best);
  });

  test("never falls back to video, DRM, OTF, or unbounded downloads", () => {
    for (const item of [
      format({ qualityLabel: "360p" }),
      format({ mimeType: 'video/mp4; codecs="mp4a.40.2"' }),
      format({ audioQuality: undefined }),
      format({ drmFamilies: ["WIDEVINE"] }),
      format({ type: "FORMAT_STREAM_TYPE_OTF" }),
      format({ url: undefined }),
      format({ contentLength: undefined }),
      format({ contentLength: "0" }),
      format({ contentLength: "8589934593" }),
    ]) {
      expect(() => selectAudioFormat([item])).toThrow(
        "No downloadable audio-only format.",
      );
    }
  });
});
