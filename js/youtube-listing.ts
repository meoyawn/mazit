import { Helpers, type Innertube, YT, YTNodes } from "youtubei.js";

export async function fetchPlaylist(yt: Innertube, id: string) {
  // Include unavailable placeholders so the flat listing matches the advertised count.
  // This is the same browse parameter used by yt-dlp's YouTube playlist extractor.
  const response = await yt.actions.execute("/browse", {
    browseId: `VL${id}`,
    params: "wgYCCAA=",
  });
  return new YT.Playlist(yt.actions, response);
}

export function playlistPage(page: YT.Playlist) {
  return {
    title: page.info.title,
    description: page.info.description || "",
    cover_url: page.info.thumbnails?.[0]?.url || null,
    count: page.info.total_items,
    continuation: page.has_continuation,
    // Message renderers also contain podcast descriptions, not just listing notices.
    suspicious:
      page.page.alerts?.some(
        (alert) =>
          alert.is(YTNodes.Alert, YTNodes.AlertWithButton) &&
          alert.alert_type !== "INFO",
      ) || false,
    videos: page.items.map(playlistEntry),
  };
}

export function playlistEntry(item: Helpers.YTNode) {
  if (item.is(YTNodes.PlaylistVideo))
    return {
      id: item.id,
      title: item.title.toString(),
      duration: item.duration.seconds || 0,
      available:
        item.is_playable &&
        !item.is_live &&
        !item.is_upcoming &&
        !item.upcoming,
      published_text: item.video_info.toString(),
    };
  if (
    item.is(YTNodes.LockupView) &&
    ["VIDEO", "SHORT"].includes(item.content_type)
  )
    return {
      id: item.content_id,
      title: item.metadata?.title.toString() || item.content_id,
      duration: 0,
      // Unavailable modern entries retain their ID but omit metadata entirely.
      available:
        !!item.metadata?.title.toString() && !hasLiveOrUpcomingBadge(item),
      // The final metadata row contains views and age; preceding rows name the author.
      published_text:
        item.metadata?.metadata?.metadata_rows
          .at(-1)
          ?.metadata_parts?.at(-1)
          ?.text?.toString() || null,
    };
  throw new Error("Unsupported playlist item; listing is incomplete.");
}

function hasLiveOrUpcomingBadge(item: YTNodes.LockupView): boolean {
  if (!item.content_image?.is(YTNodes.ThumbnailView)) return false;
  return item.content_image.overlays.some(
    (overlay) =>
      overlay.is(
        YTNodes.ThumbnailOverlayBadgeView,
        YTNodes.ThumbnailBottomOverlayView,
      ) &&
      overlay.badges.some(
        (badge) =>
          badge.badge_style === "THUMBNAIL_OVERLAY_BADGE_STYLE_LIVE" ||
          badge.text?.trim().toLowerCase() === "upcoming",
      ),
  );
}
