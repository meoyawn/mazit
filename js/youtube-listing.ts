import { Helpers, YTNodes } from "youtubei.js";

export function playlistEntry(item: Helpers.YTNode) {
  if (item.is(YTNodes.PlaylistVideo))
    return {
      id: item.id,
      title: item.title.toString(),
      duration: item.duration.seconds || 0,
      available: item.is_playable && !item.is_live && !item.is_upcoming,
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
      available: true,
      // The final metadata row contains views and age; preceding rows name the author.
      published_text:
        item.metadata?.metadata?.metadata_rows
          .at(-1)
          ?.metadata_parts?.at(-1)
          ?.text?.toString() || null,
    };
  throw new Error("Unsupported playlist item; listing is incomplete.");
}
