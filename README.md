# Mazit

A macOS desktop app for listening to YouTube as a podcast. Add a playlist or channel, connect your own S3-compatible storage, and subscribe to the published `rss.xml` feed in your podcast player.

Mazit downloads audio, prepares fast-start M4A files, and refreshes feeds in the background. Storage must provide direct public URLs for feeds and audio. Desktop storage credentials are saved in macOS Keychain.

[Cloudflare R2 setup and CDN guide](docs/cloudflare-r2.md) — storage credentials, free CDN features, request budgets, byte-range playback, and ETag checks.

## Build and run

The current build targets Apple Silicon Macs running macOS 14 or later. Development requires CMake, rustup with stable Rust, go-task (`task`), Xcode, and Nub 0.9.2.

```sh
git clone https://github.com/meoyawn/mazit.git
cd mazit
task build
./target/aarch64-apple-darwin/release/mazit
```

The build prepares FFmpeg and the embedded YouTube.js bridge automatically.

- `task dev` — watch sources, rebuild, and restart the GPUI app. Installs Watchexec if needed.
- `task build` — build the macOS Apple Silicon release binary at `target/aarch64-apple-darwin/release/mazit`.
- `task lint` — format the Rust workspace, then run Clippy.

Run `task build` or `task dev` once before linting to prepare the native libraries and JavaScript bundle. Use `mazit --help` for the command-line options, including one-shot synchronization and a custom library directory.

## Privacy

Keep storage configuration files, exported browser cookies, and library data outside the repository. The default library lives in `~/Library/Application Support/Mazit`. Only publish files you intend to make public through your storage provider.

## License

[MIT](LICENSE). Third-party dependencies retain their own licenses, including FFmpeg under LGPL-2.1-or-later.
