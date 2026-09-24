import { $ } from "bun";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const directory = await mkdtemp(join(tmpdir(), "mazit-youtube-live-"));
const sources = [
  {
    url: "https://www.youtube.com/playlist?list=PLipBN7O7_H3oq9oDRWagdZUoBOV81GCkT",
    listing:
      "https://www.youtube.com/playlist?list=PLipBN7O7_H3oq9oDRWagdZUoBOV81GCkT",
  },
  {
    url: "https://www.youtube.com/@RyanFleury/videos",
    listing: "https://www.youtube.com/playlist?list=UUCsdwE2z_kRL3vfVsd5OGyA",
  },
];

try {
  const expected = [];
  for (const { url, listing } of sources) {
    const data =
      await $`yt-dlp --ignore-config --flat-playlist --skip-download --dump-single-json ${listing}`.json();
    const ids = data.entries.map((entry: { id: string }) => entry.id);
    console.log(`yt-dlp enumerated ${ids.length} entries for ${url}`);
    expected.push({ url, ids });
  }
  const fixture = join(directory, "listings.json");
  await Bun.write(fixture, JSON.stringify(expected));
  await $`cargo test --locked --test youtube_live -- --ignored --nocapture`
    .cwd(root)
    .env({ ...process.env, MAZIT_YOUTUBE_EXPECTED_LISTINGS: fixture });
} finally {
  await rm(directory, { recursive: true, force: true });
}
