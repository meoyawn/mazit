# Desktop UI tests

Run `task test:ui` for the library UI, or `task test` for the entire suite. The task graph is `check → test → [test:unit, test:ui]`; `check` also requires `lint`. The UI task enables the `ui-tests` feature and GPUI's `test-support` feature. No desktop interaction, screen capture, browser, credentials, or running Mazit instance is needed.

The approach follows [GPUI's testing introduction](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md#other-resources) and the APIs in the [TestAppContext and VisualTestContext source for our pinned GPUI revision](https://github.com/zed-industries/zed/blob/69e2130295c2649963eb639fc70b4f2ee8ea1624/crates/gpui/src/app/test_context.rs). Consult the installed `gpui-0.2.2` sources when examples from Zed's main branch use newer APIs.

## What runs

`src/desktop/tests.rs` renders the production `MazitView` under GPUI Component's `Root`, using `#[gpui::test]`. The test platform provides its own window, clipboard, opened-URL recorder, and deterministic executor. Native menu-bar and macOS window integration are attached separately by the production startup code.

Tests locate controls using GPUI `debug_selector` and `debug_bounds`, then send input through `simulate_click` and `simulate_input`. They assert the observable result: the complete copied RSS URL, the opened YouTube URL, the source-specific refresh command, disabled controls while syncing, and clearing the input after adding a subscription. The engine's channel is captured instead of starting a worker that accesses YouTube or S3.

The layout checks use multiple window widths and a long RSS URL. Artwork tests decode small landscape and portrait PNG fixtures and inspect the image's paint-time content mask. Removing `overflow_hidden()` from the production artwork container must fail the clipping regression test. Other checks exercise cover replacement and removal through the actual state-polling loop by advancing GPUI's test clock.

These are headless UI integration tests, not pixel-golden tests or an end-to-end test against live services. GPUI 0.2.2's test window performs layout and paint preparation but does not rasterize a GPU framebuffer. The separate engine tests cover publication order, S3 request bodies and content types, RSS artwork URLs, and preserving the existing feed when a cover upload fails.

## Adding a regression

1. Render the production view or artwork helper; avoid a duplicate test-only layout.
2. Add a stable debug selector if a control needs to be located. GPUI compiles these selectors away without `test-support`.
3. Simulate input and assert the resulting command, clipboard, URL, layout bounds, or paint mask.
4. Advance the deterministic clock for timers; do not sleep or automate the running desktop app.
5. Run `task lint`, then `task test`.

The PNG fixtures are synthetic, solid-color images generated with Python's standard `struct` and `zlib` modules. They contain no user artwork or network dependencies.
