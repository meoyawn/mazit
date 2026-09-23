import { describe, expect, test } from "bun:test";
import { Innertube, YT, YTNodes } from "youtubei.js";
import {
  fetchPlaylist,
  playlistEntry,
  playlistPage,
} from "../js/youtube-listing.ts";

describe("flat playlist metadata", () => {
  test.each(["LIVE", "UPCOMING", "scheduled"])(
    "skips classic %s entries while retaining their identity",
    (status) => {
      const item = new YTNodes.PlaylistVideo({
        videoId: "AmUPnXrZ9J0",
        title: {
          simpleText: "N64 3D Rendering From Scratch",
          accessibility: { accessibilityData: { label: "N64" } },
        },
        isPlayable: true,
        thumbnailOverlays:
          status === "scheduled"
            ? []
            : [{ thumbnailOverlayTimeStatusRenderer: { style: status } }],
        upcomingEventData:
          status === "scheduled" ? { startTime: "1790812800" } : undefined,
      });
      expect(playlistEntry(item)).toMatchObject({
        id: "AmUPnXrZ9J0",
        available: false,
      });
    },
  );

  test.each([
    "thumbnailBottomOverlayViewModel",
    "thumbnailOverlayBadgeViewModel",
  ])(
    "reads live/upcoming state from %s without excluding archived streams",
    (overlayType) => {
      for (const [text, badgeStyle, available] of [
        ["Upcoming", "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT", false],
        ["NA ŻYWO", "THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE", false],
        ["1:02:03", "THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT", true],
      ] as const) {
        const badgeKey =
          overlayType === "thumbnailBottomOverlayViewModel"
            ? "badges"
            : "thumbnailBadges";
        const item = new YTNodes.LockupView({
          contentId: "AmUPnXrZ9J0",
          contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
          contentImage: {
            thumbnailViewModel: {
              image: { sources: [] },
              overlays: [
                {
                  [overlayType]: {
                    [badgeKey]: [
                      { thumbnailBadgeViewModel: { text, badgeStyle } },
                    ],
                  },
                },
              ],
            },
          },
          metadata: {
            lockupMetadataViewModel: {
              title: { content: "N64 3D Rendering From Scratch" },
              metadata: {
                contentMetadataViewModel: {
                  metadataRows: [
                    {
                      metadataParts: [
                        { text: { content: "Streamed 2 days ago" } },
                      ],
                    },
                  ],
                },
              },
            },
          },
        });
        expect(playlistEntry(item)).toMatchObject({
          id: "AmUPnXrZ9J0",
          available,
        });
      }
    },
  );

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

  test("includes unavailable entries across flat podcast pages without rejecting the description", async () => {
    const first = {
      metadata: {
        playlistMetadataRenderer: {
          title: "Interviews",
          description: "Expert interviews",
        },
      },
      sidebar: {
        playlistSidebarRenderer: {
          items: [
            {
              playlistSidebarPrimaryInfoRenderer: {
                stats: [{ simpleText: "3 episodes" }],
              },
            },
          ],
        },
      },
      alerts: [
        {
          alertWithButtonRenderer: {
            type: "INFO",
            text: {
              simpleText: "Unavailable videos will be hidden during playback",
            },
          },
        },
      ],
      contents: {
        itemSectionRenderer: {
          contents: [
            { messageRenderer: { text: { simpleText: "Expert interviews" } } },
            {
              playlistVideoListRenderer: {
                contents: [
                  {
                    lockupViewModel: {
                      contentId: "abcdefghijk",
                      contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
                      metadata: {
                        lockupMetadataViewModel: {
                          title: { content: "First interview" },
                        },
                      },
                    },
                  },
                  {
                    continuationItemRenderer: {
                      continuationEndpoint: {
                        commandMetadata: {
                          webCommandMetadata: {
                            apiUrl: "/youtubei/v1/browse",
                            sendPost: true,
                          },
                        },
                        continuationCommand: {
                          token: "next-page",
                          request: "CONTINUATION_REQUEST_TYPE_BROWSE",
                        },
                      },
                    },
                  },
                ],
              },
            },
          ],
        },
      },
    };
    const next = {
      onResponseReceivedActions: [
        {
          appendContinuationItemsAction: {
            continuationItems: [
              {
                lockupViewModel: {
                  contentId: "T6juU_4UqKI",
                  contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
                },
              },
              {
                lockupViewModel: {
                  contentId: "lmnopqrstuv",
                  contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
                  metadata: {
                    lockupMetadataViewModel: {
                      title: { content: "Last interview" },
                    },
                  },
                },
              },
            ],
          },
        },
      ],
    };
    const requests: { url: string; body: unknown }[] = [];
    async function fetchPage(
      input: Parameters<typeof fetch>[0],
      init?: Parameters<typeof fetch>[1],
    ) {
      requests.push({
        url:
          typeof input === "object" && "url" in input
            ? input.url
            : String(input),
        body: JSON.parse(String(init?.body)),
      });
      return Response.json(requests.length === 1 ? first : next);
    }
    fetchPage.preconnect = fetch.preconnect;
    const yt = await Innertube.create({
      generate_session_locally: true,
      retrieve_player: false,
      retrieve_innertube_config: false,
      fetch: fetchPage,
    });

    const page = await fetchPlaylist(yt, "PLtest");
    expect(page.messages.map((message) => message.text.toString())).toEqual([
      "Expert interviews",
    ]);
    const initial = playlistPage(page);
    expect(initial.suspicious).toEqual(false);
    expect(initial.count).toEqual("3 episodes");
    expect(initial.continuation).toEqual(true);
    expect(initial.title).toEqual("Interviews");

    const continued = playlistPage(await page.getContinuation());
    expect(continued.continuation).toEqual(false);
    expect(continued.suspicious).toEqual(false);
    expect(
      [...initial.videos, ...continued.videos].map(({ id, available }) => ({
        id,
        available,
      })),
    ).toEqual([
      { id: "abcdefghijk", available: true },
      { id: "T6juU_4UqKI", available: false },
      { id: "lmnopqrstuv", available: true },
    ]);
    expect(continued.videos[0]).toEqual({
      id: "T6juU_4UqKI",
      title: "T6juU_4UqKI",
      duration: 0,
      available: false,
      published_text: null,
    });
    expect(requests.map((request) => new URL(request.url).pathname)).toEqual([
      "/youtubei/v1/browse",
      "/youtubei/v1/browse",
    ]);
    expect(requests[0].body).toMatchObject({
      browseId: "VLPLtest",
      params: "wgYCCAA=",
    });
    expect(requests[1].body).toMatchObject({ continuation: "next-page" });
  });

  test.each(["WARNING", "ERROR"])(
    "still rejects %s playlist alerts",
    async (type) => {
      const yt = await Innertube.create({
        generate_session_locally: true,
        retrieve_player: false,
        retrieve_innertube_config: false,
      });
      const page = new YT.Playlist(yt.actions, {
        success: true,
        status_code: 200,
        data: {
          contents: { playlistVideoListRenderer: { contents: [] } },
          alerts: [
            {
              alertWithButtonRenderer: {
                type,
                text: { simpleText: "Listing failed" },
              },
            },
          ],
        },
      });
      expect(playlistPage(page).suspicious).toEqual(true);
    },
  );
});
