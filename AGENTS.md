Do not run `cargo fmt` directly. Run `task lint`; it formats the workspace immediately before linting.

For JavaScript tooling, use only `bun`. Run scripts and package binaries with `bun run`. Never use `bunx`, `nub`, `nubx`, or `node`.

This project is greenfield. Do not add migrations or compatibility paths for old configuration or stored data unless explicitly requested.

SQLite stores durable sync checkpoints, not application settings. Keep configuration in files. Run versioned SQL from `migrations/` through Refinery in `src/database/migrations.rs`; never edit or remove an applied migration.

**`innertube.md` is exclusively an Innertube experiment log, not a general decision log or changelog.** Read it before changing or experimenting with Innertube, then update it with client/API behavior, authentication, request strategies, observed results, failures, and unverified assumptions. Do not add Apple Podcasts ordering, RSS presentation, UI, database, or other unrelated decisions, even when they are part of the same task. Keep entries concise; omit routine test transcripts and per-video tables. Preserve relevant experiment history. Never include credentials, cookies, tokens, or signed media URLs.

Playlist/channel sync and diffing must use flat playlist pages, including continuations. Extract dates from listing metadata; never expand each video to fill metadata. Only audio downloads may fetch per-video info, using the media client documented in `innertube.md`.

Verify desktop UI changes with the GPUI test harness (`task test:ui`), not computer use or screenshots of the running app. Follow `docs/testing.md` for simulated input, layout bounds, and artwork clipping checks.
