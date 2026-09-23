import { describe, expect, test } from "bun:test";
import { resolveChannel } from "../js/youtube-channel.ts";

const channelId = "UCCsdwE2z_kRL3vfVsd5OGyA";
const canonical = `https://www.youtube.com/channel/${channelId}`;

describe("channel URL resolution", () => {
  test.each([
    "https://www.youtube.com/@RyanFleury/videos",
    "https://www.youtube.com/@RyanFleury/",
    "https://www.youtube.com/@RyanFleury",
    "https://www.youtube.com/@RyanFleury/videos/",
    "https://www.youtube.com/@RyanFleury/streams",
    canonical,
    "https://www.youtube.com/c/RyanFleury/videos",
    "https://www.youtube.com/user/RyanFleury/",
  ])("resolves %s with one HTML GET", async (url) => {
    const requests: string[] = [];
    async function fetchPage(input: string) {
      requests.push(input);
      return new Response(`<!doctype html><html><head>
        <script>var recommendation = {"browseId":"UCabcdefghijklmnopqrstuv"};</script>
        <link rel="canonical" href="${canonical}">
        </head><body><a href="/channel/UCabcdefghijklmnopqrstuv">Other channel</a></body></html>`);
    }
    expect(await resolveChannel(url, fetchPage)).toEqual(channelId);
    expect(requests).toEqual([url]);
  });

  test("accepts canonical attributes in either order and normalizes mobile HTTP URLs", async () => {
    const requests: string[] = [];
    async function fetchPage(url: string) {
      requests.push(url);
      return new Response(`<link href = '${canonical}/'\nrel = 'canonical' />`);
    }
    expect(
      await resolveChannel("http://m.youtube.com/@RyanFleury/", fetchPage),
    ).toEqual(channelId);
    expect(requests).toEqual(["https://www.youtube.com/@RyanFleury/"]);
  });

  test.each([
    "https://www.youtube.com/watch?v=abcdefghijk",
    "https://www.youtube.com/playlist?list=PLexample",
    "https://www.youtube.com/",
    "https://www.youtube.com/@/",
    "https://example.com/@RyanFleury/",
    "ftp://www.youtube.com/@RyanFleury/",
  ])("rejects non-channel URL %s before fetching", async (url) => {
    const requests: string[] = [];
    async function fetchPage(input: string) {
      requests.push(input);
      return new Response("");
    }
    await expect(resolveChannel(url, fetchPage)).rejects.toThrow(
      "Enter a YouTube channel URL",
    );
    expect(requests).toEqual([]);
  });

  test.each([
    "<html><title>Before you continue to YouTube</title></html>",
    `<a href="${canonical}">Recommended channel</a>`,
    `<link rel="alternate" href="${canonical}">`,
    '<link rel="canonical" href="https://www.youtube.com/watch?v=abcdefghijk">',
    '<link rel="canonical" href="https://www.youtube.com/channel/UCtoo_short">',
    '<link rel="canonical" href="https://example.com/channel/UCCsdwE2z_kRL3vfVsd5OGyA">',
  ])("rejects HTML without canonical channel metadata: %s", async (html) => {
    async function fetchPage() {
      return new Response(html);
    }
    await expect(
      resolveChannel("https://www.youtube.com/@RyanFleury/", fetchPage),
    ).rejects.toThrow("Channel not found in YouTube page");
  });

  test("reports a consent redirect instead of claiming the channel is missing", async () => {
    async function fetchPage() {
      return new Response(`<html><title>Before you continue to YouTube</title>
        <form action="https://consent.youtube.com/save" method="POST"></form></html>`);
    }
    await expect(
      resolveChannel("https://www.youtube.com/@RyanFleury/", fetchPage),
    ).rejects.toThrow("YouTube returned a consent page instead of the channel");
  });

  test.each([404, 429, 500])(
    "reports HTTP %s without reading channel metadata",
    async (status) => {
      async function fetchPage() {
        return new Response(`<link rel="canonical" href="${canonical}">`, {
          status,
        });
      }
      await expect(
        resolveChannel("https://www.youtube.com/@RyanFleury/", fetchPage),
      ).rejects.toThrow(`YouTube channel page returned HTTP ${status}`);
    },
  );
});
