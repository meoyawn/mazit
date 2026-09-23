# Mazit

Mazit is a macOS desktop app that syncs YouTube content to your own S3-compatible storage in the background. Add a playlist or channel, and Mazit periodically checks for new videos, downloads their audio, prepares playable fast-start M4A files, uploads them to storage, and publishes an `rss.xml` podcast feed for each source. Syncing continues while the app runs in the menu bar.

Select **Copy RSS URL** in Mazit and add the feed to Apple Podcasts, Pocket Casts, or another podcast app. Your podcast player streams episodes from your public storage URL while Mazit keeps the feed up to date. Storage must provide direct public URLs for feeds and audio. S3 settings and credentials live in `~/.config/mazit/config.toml`.

[Cloudflare R2 setup and CDN guide](docs/cloudflare-r2.md) — storage credentials, free CDN features, request budgets, byte-range playback, and ETag checks.

## Build and run

The current build targets Apple Silicon Macs running macOS 14 or later. Development requires CMake, rustup with stable Rust, go-task (`task`), Xcode, and Bun 1.4.2.

```sh
git clone https://github.com/meoyawn/mazit.git
cd mazit
task bundle
open dist/Mazit.app
```

The build prepares FFmpeg and the embedded YouTube.js bridge automatically.

- `task dev` — watch sources, rebuild, and restart the GPUI app. Installs Watchexec if needed. Quit Mazit from the menu bar to stop the dev session.
- `task run` — prepare dependencies, build debug Rust, and run `target/debug/mazit`. `task dev` calls this task on each restart.
- `task build:debug` — build the debug binary without starting it.
- `task build` — build the macOS Apple Silicon release binary at `target/aarch64-apple-darwin/release/mazit`.
- `task bundle` — build the macOS app at `dist/Mazit.app`.
- `task lint` — format the Rust workspace, then run Clippy.
- `task test` — run the Cargo workspace tests.
- `task check` — run lint and tests together as one check.

The dependency graph is `run → build:debug → prepare` and `build → prepare`. Preparation bundles the YouTube bridge with Bun and runs the Bun Shell script in `scripts/prepare-ffmpeg.ts` to build checksum-verified FFmpeg libraries when missing. JavaScript installation runs before bundling; Rust compilation waits for both the bridge and FFmpeg.

## Storage configuration

Select **Open config.toml** in the desktop app. The button creates `~/.config/mazit/config.toml` with an empty template and opens it using the `EDITOR` environment variable. For example, launch Mazit with `EDITOR='code --wait' task run`. If `EDITOR` is unset, Mazit opens the file in TextEdit. Set `EDITOR` in the environment that launches the app if you use the macOS app bundle.

Fill in the TOML file with your S3-compatible storage details:

```toml
[s3]
endpoint = "https://<ACCOUNT_ID>.r2.cloudflarestorage.com"
region = "auto"
bucket = "podcasts"
root = "mazit"
public_base_url = "https://audio.example.com"
access_key_id = "<ACCESS_KEY_ID>"
secret_access_key = "<SECRET_ACCESS_KEY>"
```

The schema has one `[s3]` table. All keys except `root` are required strings. `root` is an optional folder prefix and defaults to `""`; omit it or leave it empty to write at the bucket root. `endpoint` is the S3 API URL, `region` is the signing region, and `bucket` is the bucket name. `public_base_url` is the direct public URL for that bucket, without `root`; Mazit appends `root` and the subscription folder to published URLs. `access_key_id` and `secret_access_key` are the S3 key pair. Unknown keys are rejected. Storage URLs must use HTTPS, except HTTP on localhost for development.

Save the file, then select **Reload config** in the desktop app to apply it. A successful reload verifies S3 access and the public URL, refreshes every saved YouTube subscription, and uploads any changes to S3. An existing library cannot switch to another endpoint, region, bucket, root, or public base URL after subscriptions have been added. Use **Refresh all** in the app to synchronize on demand.

The file contains secrets. Mazit creates its config directory with mode `0700` and its template with mode `0600`; it tightens the permissions of an existing config file to `0600` when reading it. Keep the file outside the repository and do not share its contents.

## Privacy

Keep storage configuration files and library data outside the repository. The library lives in `~/Library/Application Support/Mazit`. Only publish files you intend to make public through your storage provider.

## Application logs

Select **Open logs** in the sidebar to open the current log in your `EDITOR` (TextEdit by default), including while a sync is running. Logs are written immediately to `~/Library/Application Support/Mazit/logs/mazit_rCURRENT.log`, with timestamps for startup, sync stages, transfers, retries, failures, publication, and completion. The log rotates at 5 MiB and retains four older files in the same directory. Restarting Mazit appends to the current log.

The log directory is private to your user. Logged URLs are redacted, and raw SDK request diagnostics, storage credentials, and cookie headers are not logged.

## License

[MIT](LICENSE). Third-party dependencies retain their own licenses, including FFmpeg under LGPL-2.1-or-later.
