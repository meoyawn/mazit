Do not run `cargo fmt` directly. Run `task lint`; it formats the workspace immediately before linting.

For JavaScript tooling, use only `bun`. Run scripts and package binaries with `bun run`. Never use `bunx`, `nub`, `nubx`, or `node`.

This project is greenfield. Do not add migrations or compatibility paths for old configuration or stored data unless explicitly requested.

Verify desktop UI changes with the GPUI test harness (`task test:ui`), not computer use or screenshots of the running app. Follow `docs/testing.md` for simulated input, layout bounds, and artwork clipping checks.
