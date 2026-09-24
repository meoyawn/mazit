# Desktop UI tests

Run `task test:ui` for the library UI, or `task test` for the entire suite. The task graph is `check → test → [test:unit, test:ui, youtubei:test]`; `check` also requires `lint`. The UI task enables the `ui-tests` feature and GPUI's `test-support` feature. No desktop interaction, screen capture, browser, credentials, or running Mazit instance is needed.

The approach follows [GPUI's testing introduction](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md#other-resources) and the APIs in the [TestAppContext and VisualTestContext source for our pinned GPUI revision](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/src/app/test_context.rs). Consult the installed `gpui-0.2.2` sources when examples from Zed's main branch use newer APIs.

## What runs

The isolated `youtubei/` crate tests its actual Bun bundle in QuickJS with offline
responses. App tests in `src/youtube/native_tests.rs` exercise that crate through
Rust HTTP callbacks, parser objects, flat continuations, and audio selection.
The former TypeScript bridge tests now run as Rust tests.
An app-worker integration test starts the actual dedicated thread, initializes a
real upstream session, overlaps two scans of the same playlist with an audio
request, and verifies continuation isolation, session reuse, and recovery after
an API error. Only its HTTP responses are fixtures.
The overlapping first-page responses are delayed by different amounts so one
scan must keep progressing after the other finishes. The `youtubei` crate owns
the worker, runtime scheduling, and tests for shared callers across threads.

`task test:youtube-versions` runs the crate and desktop integration tests against
npm releases 18.0.0 and 18.1.0. The bindings target the 18+ API. The script
restores the normal bundle after running; run it separately from other
builds. The normal test suite stays offline.

For performance against the pre-crate application, see
[the baseline comparison](youtube-boundary-benchmark.md). Its shared Rust harness
compares complete playlist scans and media-info resolution against an isolated
`origin/main` checkout, using the latest npm library on both sides.

For a live end-to-end check, run `task prepare`, then
`bun run scripts/test-youtube-live.ts`. It enumerates a long playlist and a
channel's uploads with yt-dlp using flat pages, then exercises the app's actual
subscription, HTTP transport, worker, QuickJS, parsing, and database paths. Every
video ID and its order must match. Both scans run concurrently under a 30-second
deadline to cover startup scheduling. It uses a temporary database, performs no
per-video lookups or media downloads, and needs yt-dlp installed.

`src/desktop/tests.rs` renders the production `MazitView` under GPUI Component's `Root`, using `#[gpui::test]`. The test platform provides its own window, clipboard, opened-URL recorder, and deterministic executor. Native menu-bar and macOS window integration are attached separately by the production startup code.

Tests locate controls using GPUI `debug_selector` and `debug_bounds`, then send input through `simulate_click` and `simulate_input`. They assert the observable result: the complete copied RSS URL, the opened YouTube URL, the source-specific refresh command, disabled controls while syncing, and clearing the input after adding a subscription. The engine's channel is captured instead of starting a worker that accesses YouTube or S3.

Download manager tests click episode counts during sync, navigate between the library and queue, pause and resume admissions, filter stages, and scroll a 504-item queue. They check range-fill bounds at supported window sizes and advance the test clock to verify progress reaches the view without input. GPUI 0.2.2 retains old debug selectors across frames; test filtering by the new first row's bounds instead of expecting removed selectors to disappear. Backend tests cover the shared cross-podcast concurrency budget, range retries and live bytes, S3 upload cleanup, cancellation during conversion, and startup removal of interrupted transfer directories.

Deletion tests click the trash control during sync, check its bounds, disable duplicate deletion, and retry errors. Mock S3 tests cover paginated object and unfinished-upload cleanup, partial failures, restart recovery, and draining an in-flight publication before deleting its folder. SQLite tests read concurrently during an uncommitted write and verify connection pragmas and transactional source deletion.

Adaptive admission tests use deterministic throughput samples to cover growth, saturation, cooldown, retries, stalls, recovery, and the sixteen-slot ceiling. Manager tests feed byte callbacks through the real admission gate to check wakeups, pause, cancellation, and draining after a limit reduction. Recent-speed tests verify stale traffic expires. These tests do not measure a live network's maximum capacity.

The layout checks use multiple window widths and a long RSS URL. Artwork tests decode small landscape and portrait PNG fixtures and inspect the image's paint-time content mask. Removing `overflow_hidden()` from the production artwork container must fail the clipping regression test. Other checks exercise cover replacement and removal through the actual state-polling loop by advancing GPUI's test clock.

These are headless UI integration tests, not pixel-golden tests or an end-to-end test against live services. GPUI 0.2.2's test window performs layout and paint preparation but does not rasterize a GPU framebuffer. The separate engine tests cover publication order, S3 request bodies and content types, RSS artwork URLs, and preserving the existing feed when a cover upload fails.

The scan-completion regression changes shared engine state from scanning to idle,
advances the polling clock, and verifies the rendered status changes to “Up to
date” before any input. It then checks that refresh becomes available again.

## Adding a regression

1. Render the production view or artwork helper; avoid a duplicate test-only layout.
2. Add a stable debug selector if a control needs to be located. GPUI compiles these selectors away without `test-support`.
3. Simulate input and assert the resulting command, clipboard, URL, layout bounds, or paint mask.
4. Advance the deterministic clock for timers; do not sleep or automate the running desktop app.
5. Run `task lint`, then `task test`.

The PNG fixtures are synthetic, solid-color images generated with Python's standard `struct` and `zlib` modules. They contain no user artwork or network dependencies.
