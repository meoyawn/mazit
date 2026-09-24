Do not run `cargo fmt` directly. Run `task lint`; it formats the workspace immediately before linting.

For JavaScript tooling, use only `bun`. Run scripts and package binaries with `bun run`. Never use `bunx`, `nub`, `nubx`, or `node`.

**All scripting must use JavaScript/TypeScript with Bun Shell. Python and system-shell scripting are banned.** This includes ad hoc automation, file transformations, inline scripts, and temporary scripts. Read and follow the `bun-shell` skill; use `import { $ } from "bun"` for commands and run scripts with `bun run`. Do not write or run Python scripts, `.sh` files, Bash/Zsh/Fish scripts, or inline equivalents such as `python -c`, `bash -c`, and `sh -c`. Do not bypass this rule by launching another interpreter from Bun. Simple direct CLI invocations such as `rg`, `git`, and `task` are allowed.

This project is greenfield. Do not add migrations or compatibility paths for old configuration or stored data unless explicitly requested.

SQLite stores durable sync checkpoints, not application settings. Keep configuration in files. Run versioned SQL from `migrations/` through Refinery in `src/database/migrations.rs`; never edit or remove an applied migration.

**`innertube.md` is exclusively a log of Innertube decisions and experiments: YouTube client/API behavior, authentication, request strategies, observed results, failures, and unverified assumptions.** Read it before changing or experimenting with Innertube, then update it with relevant findings.

**Compiler and runtime work does not belong in `innertube.md`, even when it uses youtubei.js as its workload.** Never add compiler evaluations, native compilation, language transpilation, C ABI/FFI design, JavaScript engine/runtime integration, build pipelines, or runtime/build/CPU/RAM benchmarks. Keep those notes in their own experiment documentation. If an experiment also discovers Innertube behavior, record only the API/client finding here. This file is not a general decision log or changelog: Apple Podcasts ordering, RSS presentation, UI, database, and other unrelated decisions are also out of scope, even when part of the same task. Keep entries concise; omit routine test transcripts and per-video tables. Preserve relevant Innertube experiment history. Never include credentials, cookies, tokens, or signed media URLs.

Playlist/channel sync and diffing must use flat playlist pages, including continuations. Extract dates from listing metadata; never expand each video to fill metadata. Only audio downloads may fetch per-video info, using the media client documented in `innertube.md`.

When investigating YouTube videos, playlists, or channels, use `yt-dlp` to inspect and cross-check metadata, availability, and listing behavior. For playlist/channel checks, use `--flat-playlist` and verify all continuation pages without downloading media or expanding individual videos.

Verify desktop UI changes with the GPUI test harness (`task test:ui`), not computer use or screenshots of the running app. Follow `docs/testing.md` for simulated input, layout bounds, and artwork clipping checks.
