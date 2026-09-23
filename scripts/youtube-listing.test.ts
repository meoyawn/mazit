import { describe, expect, test } from "bun:test";
import { YTNodes } from "youtubei.js";
import { playlistEntry } from "../js/youtube-listing.ts";

describe("flat playlist metadata", () => {
  test("reads age from the classic renderer's videoInfo", () => {
    const item = new YTNodes.PlaylistVideo({
      videoId: "FAaMG_3Lwug",
      title: {
        simpleText: "Showdown",
        accessibility: { accessibilityData: { label: "Showdown" } },
      },
      videoInfo: { simpleText: "50K views • 10 years ago" },
      isPlayable: true,
      lengthSeconds: "300",
    });
    expect(playlistEntry(item)).toEqual({
      id: "FAaMG_3Lwug",
      title: "Showdown",
      duration: 300,
      available: true,
      published_text: "50K views • 10 years ago",
    });
  });

  test("reads abbreviated age from the modern renderer, separately from the author", () => {
    const item = new YTNodes.LockupView({
      contentId: "FAaMG_3Lwug",
      contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
      metadata: {
        lockupMetadataViewModel: {
          title: { content: "Showdown" },
          metadata: {
            contentMetadataViewModel: {
              metadataRows: [
                { metadataParts: [{ text: { content: "2 days ago" } }] },
                {
                  metadataParts: [
                    { text: { content: "7.3K" } },
                    { text: { content: "15y ago" } },
                  ],
                },
              ],
            },
          },
        },
      },
    });
    expect(playlistEntry(item).published_text).toEqual("15y ago");
  });

  test("missing date metadata does not require another request", () => {
    const item = new YTNodes.LockupView({
      contentId: "FAaMG_3Lwug",
      contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
    });
    expect(playlistEntry(item).published_text).toEqual(null);
  });
});
