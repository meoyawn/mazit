# YouTube boundary comparison

Measured September 25, 2026 (Europe/Moscow) on Apple M1 Pro, Bun 1.4.2,
Rust 1.96.0. The npm registry's latest youtubei.js release was **18.1.0**;
both implementations used that exact release.

The baseline is freshly fetched `origin/main`, commit
`bd97fd111e6dd3bfab05aeff59a9526ac0369e13`. There is no `origin/master`.
Its TypeScript bridge and Rust implementation are unchanged. Only its npm
dependency/lockfile were updated from 18.0.0 to 18.1.0, and the shared benchmark
example was added in an isolated checkout. The candidate is the current working
tree's native crate integration.

## Results

Warm medians; lower is better. A sample covers the complete operation, including
the app worker, HTTP transport, upstream parsing, Rust conversion/validation,
all continuations, and format selection as applicable. Hashing and benchmark
output serialization are outside the timer.

| Workload | Before: origin/main bridge | Current native crate | Elapsed reduction |
| --- | ---: | ---: | ---: |
| Fixed HTTPS: 500 entries across 5 pages | 22.21 ms | 18.77 ms | 15.5% |
| Fixed HTTPS: one media-info resolution, 24 formats | 0.700 ms | 0.625 ms | 10.7% |
| Fixed HTTPS: 8 simultaneous media-info resolutions | 5.25 ms | 3.66 ms | 30.3% |
| Live: 504 entries across all pages | 1,969 ms | 1,937 ms | 1.6% observed |
| Live: one media-info resolution | 108.1 ms | 103.8 ms | 4.0% observed |

The fixed-response comparison shows reduced processing overhead. It uses the
real HTTPS and app paths with identical synthetic response bytes and no artificial
delay. It is not a parser-only microbenchmark. The concurrent result also includes
the change from a serialized worker to overlapping I/O in one engine.

The live difference is too small relative to variation to claim a reliable
network-latency improvement. Live playlist samples ranged from 1,800–2,428 ms
before and 1,833–2,100 ms after; media samples ranged from 95.5–153.8 ms before
and 93.8–122.1 ms after. No media was downloaded, so these are not download
throughput measurements.

Fixed-response p90 values, before → after: playlist 23.00 → 19.16 ms;
single media 0.854 → 0.735 ms; eight media 5.526 → 3.846 ms.

Cold first-operation samples include engine/bundle loading and session setup.
For fixed responses, the two playlist samples were 87.9/86.0 ms before and
94.5/82.1 ms after. Single-media samples were 63.7/62.7 vs 63.3/64.0 ms.
These do not establish a startup improvement.

## Method and correctness

- Both binaries use the identical `examples/youtube_bench.rs` harness and Cargo
  release profile, with desktop rendering disabled. It calls production
  `YouTube::start`, `snapshot`, and `media`; there is no duplicate implementation
  of either boundary in the harness.
- Each workload runs in baseline/current/current/baseline order. Each process
  retains its engine and session. Fixed-response cases have 40 warm samples per
  implementation; live cases have six. Each also has two cold samples.
- The local TLS server supplies five 100-item pages, alternating classic and
  modern renderer pages, with four unavailable placeholders. Media responses
  contain 24 audio formats. Session/config requests also go through the real
  transports. The URLs are unciphered fixtures; the separate crate tests exercise
  cached-player deciphering.
- Fixed-response output hashes match exactly. Expected IDs and order are checked
  on every scan. The fixture server verifies WEB listing requests, unavailable
  inclusion parameters, and VISIONOS media requests. It observed 12 session/config
  initializations for 12 processes, rather than per-operation initialization.
- The live playlist is `PLipBN7O7_H3oq9oDRWagdZUoBOV81GCkT`. Every one of its 504
  IDs, in order, matches a complete `yt-dlp --flat-playlist` enumeration on every
  run. The live media ID is `FAaMG_3Lwug`, independently checked with yt-dlp.
- Live cover URLs differed on every response, including repeated calls to the
  same implementation. They are excluded from the live equality assertion.
  Observation-dependent date values are hashed separately. In this run, titles,
  descriptions, publication dates, ordered IDs, durations, availability, and
  selected media metadata matched. Signed media URLs and CPN are never logged.
- A synthetic page mixing renderer classes exposed upstream grouping in
  `Playlist.items`; both implementations inherit it. The final fixture uses one
  renderer class per page. The API finding is
  recorded separately in `innertube.md`.

## Reproduce

Prepare the native crate and FFmpeg with `task prepare`, and use an isolated
checkout of the desired baseline commit. Ensure the current crate's npm install
and bundle use the latest release. Then run:

```text
bun run scripts/benchmark-youtube.ts /absolute/path/to/baseline --live
```

Omit `--live` for only the controlled comparison. The runner resolves npm's
latest version, updates only the isolated baseline's npm dependency, builds both
binaries, and writes raw samples to `.cache/youtube-benchmark/results.json`.
OpenSSL is needed for the loopback server certificate; yt-dlp is needed for live
checks. Run separately from other builds and benchmarks. Generated bundles,
certificates, binaries, and raw results remain ignored by Git.

The earlier `youtubei/examples/boundary_bench.rs` compares batched property reads
with individual reads inside the new crate. That microbenchmark is separate;
none of the before/after figures above use it as the baseline.
