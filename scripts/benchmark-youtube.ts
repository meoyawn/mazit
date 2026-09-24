import { $ } from "bun";
import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { strict as assert } from "node:assert";

const root = resolve(import.meta.dir, "..");
if (!process.argv[2])
  throw new Error("Pass the isolated origin/main checkout path");
const baseline = resolve(process.argv[2]);
assert.notEqual(baseline, root, "Use an isolated baseline checkout");
await $`git diff --exit-code -- src js Cargo.toml`.cwd(baseline).quiet();
const live = process.argv.includes("--live");
const output = join(root, ".cache/youtube-benchmark");
await mkdir(output, { recursive: true });
const baselineCommit = (
  await $`git rev-parse HEAD`.cwd(baseline).text()
).trim();
const latest = (await $`bun pm view youtubei.js version`.text()).trim();
const installed = await Bun.file(
  join(root, "youtubei/node_modules/youtubei.js/package.json"),
).json();
assert.equal(
  installed.version,
  latest,
  "Prepare the current crate with the latest npm release first",
);
const binaries = {
  baseline: join(output, "baseline"),
  current: join(output, "current"),
};

await $`bun add youtubei.js@${latest}`.cwd(baseline);
await $`bun build js/youtube.ts --keep-names --target browser --format esm --outfile generated/youtube.js`.cwd(
  baseline,
);
await mkdir(join(baseline, "examples"), { recursive: true });
await Bun.write(
  join(baseline, "examples/youtube_bench.rs"),
  Bun.file(join(root, "examples/youtube_bench.rs")),
);
for (const [name, cwd] of [
  ["baseline", baseline],
  ["current", root],
] as const) {
  await $`cargo build --release --locked --no-default-features --example youtube_bench`
    .cwd(cwd)
    .env({
      ...process.env,
      FFMPEG_DIR: join(root, ".runtime/ffmpeg"),
      CARGO_TARGET_DIR: join(root, "target"),
      MACOSX_DEPLOYMENT_TARGET: "14.0",
      DEVELOPER_DIR: "/Applications/Xcode.app/Contents/Developer",
    });
  await $`cp ${join(root, "target/release/examples/youtube_bench")} ${binaries[name]}`;
}
function video(index: number) {
  const id = String(index).padStart(11, "0");
  const title = `Episode ${index}`;
  if (Math.floor(index / 100) % 2 === 1)
    return {
      playlistVideoRenderer: {
        videoId: id,
        title: {
          simpleText: title,
          accessibility: { accessibilityData: { label: title } },
        },
        isPlayable: true,
        lengthSeconds: "300",
        videoInfo: { simpleText: "2 days ago" },
      },
    };
  return {
    lockupViewModel: {
      contentId: id,
      contentType: "LOCKUP_CONTENT_TYPE_VIDEO",
      ...(index >= 496
        ? {}
        : {
            metadata: {
              lockupMetadataViewModel: {
                title: { content: title },
                metadata: {
                  contentMetadataViewModel: {
                    metadataRows: [
                      { metadataParts: [{ text: { content: "2 days ago" } }] },
                    ],
                  },
                },
              },
            },
          }),
    },
  };
}

const pages = Array.from({ length: 5 }, (_, page) => {
  const contents: unknown[] = Array.from({ length: 100 }, (_, i) =>
    video(page * 100 + i),
  );
  if (page < 4)
    contents.push({
      continuationItemRenderer: {
        continuationEndpoint: {
          commandMetadata: {
            webCommandMetadata: {
              apiUrl: "/youtubei/v1/browse",
              sendPost: true,
            },
          },
          continuationCommand: {
            token: `page-${page + 1}`,
            request: "CONTINUATION_REQUEST_TYPE_BROWSE",
          },
        },
      },
    });
  return JSON.stringify(
    page === 0
      ? {
          metadata: {
            playlistMetadataRenderer: {
              title: "Benchmark",
              description: "500 entries, 5 pages",
            },
          },
          sidebar: {
            playlistSidebarRenderer: {
              items: [
                {
                  playlistSidebarPrimaryInfoRenderer: {
                    stats: [{ simpleText: "500 videos" }],
                  },
                },
              ],
            },
          },
          contents: { playlistVideoListRenderer: { contents } },
        }
      : {
          onResponseReceivedActions: [
            { appendContinuationItemsAction: { continuationItems: contents } },
          ],
        },
  );
});
const player = JSON.stringify({
  videoDetails: {
    videoId: "abcdefghijk",
    title: "Fixture",
    shortDescription: "Description",
    lengthSeconds: "300",
  },
  playabilityStatus: { status: "OK" },
  microformat: {
    playerMicroformatRenderer: {
      publishDate: "2020-01-02",
      uploadDate: "2020-01-01",
    },
  },
  streamingData: {
    expiresInSeconds: "3600",
    formats: [],
    adaptiveFormats: Array.from({ length: 24 }, (_, i) => ({
      itag: 140 + i,
      mimeType:
        i % 2 ? 'audio/webm; codecs="opus"' : 'audio/mp4; codecs="mp4a.40.2"',
      bitrate: 128000 + i * 1000,
      audioQuality: "AUDIO_QUALITY_MEDIUM",
      contentLength: "12345",
      approxDurationMs: "300000",
      lastModified: "0",
      url: "https://example.googlevideo.com/audio",
    })),
  },
});
const device: unknown[] = [];
device[0] = "en";
device[1] = "US";
device[13] = "fixture";
device[16] = "2.20260101.00.00";
device[61] = ["fixture"];
device[79] = "UTC";
const session =
  ")]}'\n" + JSON.stringify([[null, null, [[device], "fixture-key"]]]);
const certificate = join(output, "fixture-cert.pem");
const key = join(output, "fixture-key.pem");
await $`openssl req -x509 -newkey rsa:2048 -nodes -keyout ${key} -out ${certificate} -days 1 -subj /CN=www.youtube.com -addext subjectAltName=DNS:www.youtube.com,DNS:youtubei.googleapis.com`.quiet();
const requests: Record<string, number> = {};
interface FixtureRequest {
  continuation?: string;
  params?: string;
  context: { client: { clientName: string } };
}
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  tls: { key: Bun.file(key), cert: Bun.file(certificate) },
  async fetch(request) {
    const path = new URL(request.url).pathname;
    requests[path] = (requests[path] ?? 0) + 1;
    if (path === "/sw.js_data") return new Response(session);
    if (path.endsWith("/config")) return Response.json({});
    if (path.endsWith("/browse")) {
      const body = (await request.json()) as FixtureRequest;
      const page = body.continuation
        ? Number(body.continuation.split("-")[1])
        : 0;
      assert.equal(body.context.client.clientName, "WEB");
      if (!body.continuation) assert.equal(body.params, "wgYCCAA=");
      return new Response(pages[page], {
        headers: { "content-type": "application/json" },
      });
    }
    if (path.endsWith("/player")) {
      const body = (await request.json()) as FixtureRequest;
      assert.equal(body.context.client.clientName, "VISIONOS");
      return new Response(player, {
        headers: { "content-type": "application/json" },
      });
    }
    throw new Error(`Unexpected fixture request: ${path}`);
  },
});

interface Sample {
  cold: boolean;
  ms: number;
  count: number;
  digest: string;
  ids_digest: string | null;
}
interface Run {
  mode: string;
  concurrency: number;
  samples: Sample[];
}
const results: { source: string; implementation: string; run: Run }[] = [];
try {
  for (const source of live ? ["fixture", "live"] : ["fixture"]) {
    const playlist =
      source === "fixture"
        ? "PLbenchmark"
        : "PLipBN7O7_H3oq9oDRWagdZUoBOV81GCkT";
    const video = source === "fixture" ? "abcdefghijk" : "FAaMG_3Lwug";
    let expectedIdsDigest = new Bun.CryptoHasher("sha256")
      .update(
        JSON.stringify(
          Array.from({ length: 500 }, (_, i) => String(i).padStart(11, "0")),
        ),
      )
      .digest("hex");
    let expectedCount = 500;
    if (source === "live") {
      const expected =
        await $`yt-dlp --ignore-config --flat-playlist --skip-download --dump-single-json https://www.youtube.com/playlist?list=${playlist}`.json();
      expectedIdsDigest = new Bun.CryptoHasher("sha256")
        .update(
          JSON.stringify(
            expected.entries.map((entry: { id: string }) => entry.id),
          ),
        )
        .digest("hex");
      expectedCount = expected.entries.length;
      console.log(
        `yt-dlp checked all ${expected.entries.length} live playlist entries`,
      );
    }
    for (const [mode, id, concurrency] of [
      ["playlist", playlist, 1],
      ["media", video, 1],
      ["media", video, 8],
    ] as const) {
      if (source === "live" && concurrency > 1) continue;
      const digests = new Set<string>();
      for (const implementation of [
        "baseline",
        "current",
        "current",
        "baseline",
      ] as const) {
        const env = { ...process.env };
        if (source === "fixture") {
          env.MAZIT_BENCH_CA = certificate;
          env.MAZIT_BENCH_ADDRESS = `127.0.0.1:${server.port}`;
        }
        const run: Run =
          await $`${binaries[implementation]} ${mode} ${id} ${source === "fixture" ? 20 : 3} ${concurrency}`
            .env(env)
            .json();
        for (const sample of run.samples) {
          assert.equal(
            sample.count,
            mode === "playlist" ? expectedCount : concurrency,
          );
          if (mode === "playlist")
            assert.equal(
              sample.ids_digest,
              expectedIdsDigest,
              "All IDs and their order must match the source",
            );
        }
        run.samples.forEach((sample) => digests.add(sample.digest));
        results.push({ source, implementation, run });
        console.log(
          `${source} ${implementation} ${mode} × ${concurrency}: cold ${run.samples[0]!.ms.toFixed(1)} ms; warm median ${median(run.samples.filter((s) => !s.cold).map((s) => s.ms)).toFixed(1)} ms`,
        );
      }
      assert.equal(
        digests.size,
        1,
        `Output differs between implementations: ${source} ${mode}`,
      );
    }
  }
} finally {
  server.stop(true);
  await Bun.write(
    join(output, "results.json"),
    JSON.stringify(
      {
        baselineCommit,
        youtubeiVersion: latest,
        date: new Date().toISOString(),
        requests,
        results,
      },
      null,
      2,
    ),
  );
}

function median(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2
    ? sorted[middle]!
    : (sorted[middle - 1]! + sorted[middle]!) / 2;
}
