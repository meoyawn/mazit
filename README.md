# Mazit

Mazit is a macOS desktop app that syncs YouTube content to your own S3-compatible storage in the background. Add a playlist or channel, and Mazit periodically checks for new videos, downloads their audio, prepares playable fast-start M4A files, uploads them and the source's cover image to storage, and publishes an `rss.xml` podcast feed for each source. The feed links to the uploaded cover for podcast artwork. Syncing continues while the app runs in the menu bar.

Select **Copy RSS URL** in Mazit and add the feed to Apple Podcasts, Pocket Casts, or another podcast app. Your podcast player streams episodes from your public storage URL while Mazit keeps the feed up to date. Storage must provide direct public URLs for feeds, audio, and cover images. S3 settings and credentials live in `~/.config/mazit/config.toml`.

The trash button deletes a podcast, including its local audio and artwork, S3 folder, and SQLite checkpoints. It stops that podcast's sync immediately and waits for requests already sent to S3 before removing remote files. Failed deletions stay paused with an error; click the trash button again to retry. An interrupted deletion resumes when Mazit restarts.

Storage credentials need permission to list and delete objects and to list and abort unfinished multipart uploads in the configured bucket.

SQLite uses WAL with concurrent read connections and one mutex-protected writer. Connections use `synchronous=NORMAL`, foreign keys, a 10-second busy timeout, a 2 MB cache, a 1,000-page automatic checkpoint, in-memory temporary storage, and disabled memory mapping. A power loss can lose recent checkpoint commits; the next sync repeats that work. See [SQLite's synchronous documentation](https://sqlite.org/pragma.html#pragma_synchronous).

Playlist and channel feeds use newest-first episodic ordering, without seasons or episode numbers. Sync diffs flat playlist pages and extracts their date labels without per-video lookups. Relative labels provide approximate dates, cached so they do not drift on refresh; exact dates already saved or returned during audio downloads take precedence. Videos with the same approximate date may appear tied in podcast apps. Client choices and experiments are recorded in the [Innertube experiment log](innertube.md).

The library shows each subscription once, with its cover, a link to the original YouTube source, sync status, and a compact RSS field with a copy button. Cover images are cached locally so they remain visible after restarting Mazit.

The download manager shows segmented progress, percentage, recent speed, and time remaining. All podcasts share an adaptive transfer budget: start with two slots, sample throughput every five seconds, and probe one additional slot at a time up to sixteen. Keep the extra slot when throughput improves by at least 5%; otherwise restore the previous limit and wait before probing again. Retries, sustained stalls, and sharp slowdowns reduce the limit, down to one slot. Existing transfers finish when the limit decreases or the queue is paused. Each transfer still uses four parallel audio ranges, and its slot stays occupied through conversion and upload.

[Cloudflare R2 setup and CDN guide](docs/cloudflare-r2.md) — storage credentials, free CDN features, request budgets, byte-range playback, and ETag checks.

## Build and run

The current build targets Apple Silicon Macs running macOS 14 or later. Development requires CMake, rustup with stable Rust, go-task (`task`), Xcode, and Bun 1.4.2.

```sh
git clone https://github.com/meoyawn/mazit.git
cd mazit
task bundle
open dist/Mazit.app
```

The build prepares FFmpeg automatically. Cargo fetches the pinned
[native YouTube crate](https://github.com/listenbox/youtubei) directly from Git.

SQLite schema changes live in [`migrations/`](migrations/README.md), with embedded, checksum-verified Refinery migrations run by `src/database/migrations.rs`.

- `task dev` — watch sources, rebuild, and restart the GPUI app. Installs Watchexec if needed. Quit Mazit from the menu bar to stop the dev session.
- `task run` — prepare dependencies, build debug Rust, and run `target/debug/mazit`. `task dev` calls this task on each restart.
- `task build:debug` — build the debug binary without starting it.
- `task build` — build the macOS Apple Silicon release binary at `target/aarch64-apple-darwin/release/mazit`.
- `task bundle` — build the macOS app at `dist/Mazit.app`.
- `task lint` — format the Rust workspace, then run Clippy.
- `task test` — run the Cargo workspace, YouTube integration, and headless GPUI tests.
- `task test:ui` — run the library's layout and interaction tests without desktop automation. See [UI testing](docs/testing.md).
- `task check` — run lint and tests together as one check.

The dependency graph is `run → build:debug → prepare` and `build → prepare`.
Preparation runs the Bun Shell script in `scripts/prepare-ffmpeg.ts` to build
checksum-verified FFmpeg libraries. The `youtubei` Cargo Git dependency embeds
QuickJS and downloads the published upstream CF-worker bundle, verifying its
checksum inside Cargo's output directory; no submodule setup or JavaScript
bundling is needed. Its exact revision is pinned in Cargo.toml and Cargo.lock. YouTube
policies and the shared HTTP transport stay in this app's Rust code. Task caches
FFmpeg preparation by input checksums and required output files.

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

The file contains secrets. Mazit creates its config directory with mode `0700` and its template with mode `0600`; it tightens the permissions of an existing config file to `0600` when reading it. Keep the file outside the repository and do not share its contents. The library's storage-location binding is a separate private `storage-binding` file, not a database setting.

SQLite holds sync checkpoints: subscribed sources, playlist membership and order, video metadata, completed upload receipts, and feed progress. Each completed upload is committed durably before syncing continues. If the process exits after four of eight uploads, the next launch immediately retries that source and transfers only the remaining four. Audio transfers interrupted before their checkpoint are retried from the beginning. Publishing and cleanup can also be retried after a crash; an unsuccessful sync preserves the previous feed.

## Privacy

Keep storage configuration files and library data outside the repository. The library lives in `~/Library/Application Support/Mazit`. Only publish files you intend to make public through your storage provider.

## Application logs

Select **Open logs** in the sidebar to open the current log in your `EDITOR` (TextEdit by default), including while a sync is running. Logs are written immediately to `~/Library/Application Support/Mazit/logs/mazit_rCURRENT.log`, with timestamps for startup, sync stages, transfers, retries, failures, publication, and completion. The log rotates at 5 MiB and retains four older files in the same directory. Restarting Mazit appends to the current log.

The log directory is private to your user. Logged URLs are redacted, and raw SDK request diagnostics, storage credentials, and cookie headers are not logged.

## License

[MIT](LICENSE). Third-party dependencies retain their own licenses, including FFmpeg under LGPL-2.1-or-later.
