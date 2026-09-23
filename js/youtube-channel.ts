export async function resolveChannel(
  url: string,
  fetchPage: (url: string) => Promise<Response>,
): Promise<string> {
  const page = new URL(url);
  if (
    !["http:", "https:"].includes(page.protocol) ||
    !["www.youtube.com", "youtube.com", "m.youtube.com"].includes(
      page.hostname,
    ) ||
    !/^\/(?:@[^/]+|(?:channel|c|user)\/[^/]+)(?:\/[^/]*)?\/?$/.test(
      page.pathname,
    )
  ) {
    throw new Error("Enter a YouTube channel URL");
  }
  page.protocol = "https:";
  page.hostname = "www.youtube.com";
  const response = await fetchPage(page.toString());
  if (!response.ok) {
    throw new Error(`YouTube channel page returned HTTP ${response.status}`);
  }
  const html = await response.text();
  // The canonical link identifies this channel; other page links can name recommendations.
  const canonical = html.match(
    /<link\b(?=[^>]*\brel\s*=\s*["']canonical["'])[^>]*\bhref\s*=\s*["']([^"']+)["'][^>]*>/i,
  )?.[1];
  const id = canonical?.match(
    /^https:\/\/www\.youtube\.com\/channel\/(UC[A-Za-z0-9_-]{22})\/?$/,
  )?.[1];
  if (!id) {
    if (html.includes("https://consent.youtube.com/")) {
      throw new Error("YouTube returned a consent page instead of the channel");
    }
    throw new Error("Channel not found in YouTube page");
  }
  return id;
}
