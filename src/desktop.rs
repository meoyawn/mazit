use crate::{
    config,
    engine::{Command, Engine},
};
use gpui::*;
use gpui_component::{
    Disableable, Icon as UiIcon, IconName, Root, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    clipboard::Clipboard,
    input::{Input, InputState},
    link::Link,
    tooltip::Tooltip,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

struct MazitView {
    engine: Engine,
    source: Entity<InputState>,
    cover_images: HashMap<String, Arc<Image>>,
    menu_bar: Option<MenuBar>,
    show_downloads: bool,
    download_filter: downloads::DownloadFilter,
    download_scroll: UniformListScrollHandle,
}

mod downloads;

struct MenuBar {
    _tray: TrayIcon,
    open_id: tray_icon::menu::MenuId,
    refresh_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    #[cfg(target_os = "macos")]
    _activity: Activity,
}

impl MazitView {
    fn new(engine: Engine, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Paste a YouTube playlist or channel URL")
        });
        let mut last_state = engine.state.read().clone();
        let mut last_downloads = engine.downloads.snapshot();
        let cover_images = build_cover_images(&last_state);
        cx.spawn_in(window, async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if view
                    .update_in(cx, |view, window, cx| {
                        if let Some(menu_bar) = &view.menu_bar {
                            while let Ok(event) = MenuEvent::receiver().try_recv() {
                                if event.id == menu_bar.open_id {
                                    show_window();
                                    cx.activate(true);
                                    window.refresh();
                                }
                                if event.id == menu_bar.refresh_id {
                                    view.engine.command(Command::Refresh(None));
                                }
                                if event.id == menu_bar.quit_id {
                                    cx.quit();
                                }
                            }
                            hide_if_minimized();
                        }
                        let state = view.engine.state.read().clone();
                        let downloads = view.engine.downloads.snapshot();
                        if state != last_state
                            || downloads != last_downloads
                            || (view.show_downloads
                                && downloads.items.iter().any(|item| item.phase.active()))
                        {
                            if state.covers != last_state.covers {
                                view.cover_images = build_cover_images(&state);
                            }
                            last_state = state;
                            last_downloads = downloads;
                            // Sync state lives outside GPUI. Invalidate the whole window so
                            // completion is painted even without a mouse or keyboard event.
                            cx.notify();
                            window.refresh();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            engine,
            source,
            cover_images,
            menu_bar: None,
            show_downloads: false,
            download_filter: downloads::DownloadFilter::All,
            download_scroll: UniformListScrollHandle::new(),
        }
    }
    fn open_config(&self, cx: &mut Context<Self>) {
        self.engine.state.write().message = match config::open_in_editor() {
            Ok(()) => "Opened config.toml; save it, then select Reload config".into(),
            Err(error) => crate::redact(&format!("{error:#}")),
        };
        cx.notify();
    }
    fn open_logs(&self, cx: &mut Context<Self>) {
        self.engine.state.write().message = match crate::logging::open_in_editor() {
            Ok(()) => "Opened application logs in your editor".into(),
            Err(error) => {
                log::error!("Open logs: {error:#}");
                crate::redact(&format!("{error:#}"))
            }
        };
        cx.notify();
    }
}
impl MenuBar {
    fn new() -> Self {
        let menu = Menu::new();
        let open = MenuItem::new("Open Mazit", true, None);
        let refresh = MenuItem::new("Refresh all", true, None);
        let quit = MenuItem::new("Quit Mazit", true, None);
        menu.append_items(&[&open, &refresh, &quit])
            .expect("Menu bar items");
        let mut pixels = vec![0u8; 22 * 22 * 4];
        for x in [4usize, 8, 12, 16] {
            let height = if x == 8 || x == 12 { 16 } else { 8 };
            for y in (22 - height) / 2..(22 + height) / 2 {
                for dx in 0..2 {
                    pixels[(y * 22 + x + dx) * 4 + 3] = 255;
                }
            }
        }
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Mazit")
            .with_icon(Icon::from_rgba(pixels, 22, 22).unwrap())
            .with_icon_as_template(true)
            .build()
            .expect("Menu bar icon");
        Self {
            _tray: tray,
            open_id: open.id().clone(),
            refresh_id: refresh.id().clone(),
            quit_id: quit.id().clone(),
            #[cfg(target_os = "macos")]
            _activity: Activity::new(),
        }
    }
}

impl Render for MazitView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.engine.state.read().clone();
        let sidebar = div()
            .debug_selector(|| "sidebar".into())
            .v_flex()
            .w(px(240.))
            .flex_shrink_0()
            .h_full()
            .bg(rgb(0xedf0f6))
            .p_6()
            .gap_4()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("≋ Mazit"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x7d8597))
                    .child("YOUR AUDIO, EVERYWHERE"),
            )
            .child(
                Button::new("library")
                    .debug_selector(|| "open-library".into())
                    .label("Library")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_downloads = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("downloads")
                    .debug_selector(|| "open-downloads".into())
                    .label("Downloads")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_downloads = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("settings")
                    .label("Open config.toml")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_config(cx);
                    })),
            )
            .child(
                Button::new("reload-config")
                    .label("Reload config")
                    .disabled(state.busy)
                    .on_click(cx.listener(|this, _, _, _| {
                        this.engine.command(Command::ReloadConfig);
                    })),
            )
            .child(
                Button::new("open-logs")
                    .label("Open logs")
                    .on_click(cx.listener(|this, _, _, cx| this.open_logs(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .child("Checks every hour\nContinues in the menu bar"),
            );
        if self.show_downloads {
            return div()
                .h_flex()
                .size_full()
                .bg(rgb(0xf8f9fc))
                .child(sidebar)
                .child(self.render_downloads(cx));
        }
        let mut content = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_8()
            .gap_5()
            .text_color(rgb(0x293248));
        content =
            content.child(
                div()
                    .h_flex()
                    .justify_between()
                    .child(div().text_2xl().font_weight(FontWeight::BOLD).child(
                        if !state.configured {
                            "Connect your storage"
                        } else {
                            "Your listening library"
                        },
                    ))
                    .child(
                        Button::new("refresh")
                            .debug_selector(|| "refresh-all".into())
                            .label("Refresh all")
                            .disabled(state.busy || !state.configured)
                            .on_click(cx.listener(|this, _, _, _| {
                                this.engine.command(Command::Refresh(None))
                            })),
                    ),
            );
        if !state.message.is_empty() {
            content = content.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(0xe9edf8))
                    .text_sm()
                    .child(state.message.clone()),
            );
        }
        if !state.configured {
            content = content.child(
                div()
                    .v_flex()
                    .gap_4()
                    .p_5()
                    .rounded_xl()
                    .bg(rgb(0xffffff))
                    .child("Fill in the [s3] section of ~/.config/mazit/config.toml, save it, then select Reload config.")
                    .child("Your public base URL must serve RSS, audio, and cover images directly, without a login.")
                    .child(Button::new("open-config")
                        .primary()
                        .label("Open config.toml")
                        .on_click(cx.listener(|this, _, _, cx| this.open_config(cx)))),
            );
        } else {
            content = content.child(
                div()
                    .v_flex()
                    .p_5()
                    .gap_3()
                    .bg(rgb(0xffffff))
                    .rounded_xl()
                    .child("Turn a playlist into a podcast")
                    .child(
                        div()
                            .debug_selector(|| "source-input".into())
                            .child(Input::new(&self.source)),
                    )
                    .child(
                        Button::new("add")
                            .debug_selector(|| "add-source".into())
                            .primary()
                            .label("Add subscription")
                            .disabled(state.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let url = this.source.read(cx).value().to_string();
                                if !url.trim().is_empty() {
                                    this.engine.command(Command::Add(url));
                                    this.source
                                        .update(cx, |input, cx| input.set_value("", window, cx));
                                }
                            })),
                    ),
            );
            let mut list = div()
                .id("sources")
                .v_flex()
                .overflow_y_scroll()
                .gap_5()
                .min_h_0()
                .flex_1();
            if state.sources.is_empty() {
                list=list.child(div().p_8().child("Add your first playlist or channel. Mazit downloads audio, prepares fast-start M4A files, and publishes RSS to your storage."));
            }
            for source in &state.sources {
                let id = source.id.clone();
                let element_id = ElementId::from(SharedString::from(id.clone()));
                let cover = cover_artwork(
                    self.cover_images
                        .get(&source.id)
                        .map(|image| image.clone().into()),
                )
                .debug_selector(|| format!("{}:cover", source.id));
                let (status, status_color) = match source.phase.as_str() {
                    "idle" if source.feed_url.is_some() => ("Up to date", 0x36826c),
                    "idle" => ("Waiting to sync", 0x7d8597),
                    "error" => ("Needs attention", 0xb64c48),
                    "scanning" => ("Scanning", 0x526bbe),
                    "downloading" => ("Downloading", 0x526bbe),
                    "publishing" => ("Publishing", 0x526bbe),
                    "cleaning" => ("Finishing up", 0x526bbe),
                    _ => ("Syncing", 0x526bbe),
                };
                let mut details = div()
                    .debug_selector(|| format!("{}:details", source.id))
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(source.title.clone()),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(rgb(status_color))
                                    .child(div().size(px(6.)).rounded_full().bg(rgb(status_color)))
                                    .child(status),
                            )
                            .child(
                                Button::new((element_id.clone(), "refresh"))
                                    .debug_selector(|| format!("{}:refresh", source.id))
                                    .icon(UiIcon::default().path("icons/refresh.svg"))
                                    .ghost()
                                    .small()
                                    .tooltip("Refresh subscription")
                                    .disabled(state.busy)
                                    .on_click(cx.listener(move |this, _, _, _| {
                                        this.engine.command(Command::Refresh(Some(id.clone())))
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_3()
                            .text_sm()
                            .child(
                                div()
                                    .debug_selector(|| format!("{}:youtube", source.id))
                                    .child(
                                        Link::new((element_id.clone(), "youtube"))
                                            .href(source.url.clone())
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .text_color(rgb(0x526bbe))
                                            .child(if source.kind == "channel" {
                                                "YouTube channel"
                                            } else {
                                                "YouTube playlist"
                                            })
                                            .child(
                                                UiIcon::new(IconName::ExternalLink).size(px(12.)),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new((element_id.clone(), "downloads"))
                                    .debug_selector(|| format!("{}:downloads", source.id))
                                    .ghost()
                                    .small()
                                    .label(format!(
                                        "{} / {} episodes",
                                        source.uploaded, source.total
                                    ))
                                    .tooltip("Open download manager for all podcasts")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_downloads = true;
                                        this.download_filter = downloads::DownloadFilter::All;
                                        this.download_scroll.scroll_to_item(0, ScrollStrategy::Top);
                                        cx.notify();
                                    })),
                            ),
                    );
                if let Some(error) = &source.error {
                    details = details.child(
                        div()
                            .text_color(rgb(0xb64c48))
                            .text_sm()
                            .child(error.clone()),
                    );
                }
                if let Some(url) = &source.feed_url {
                    let tooltip_url = url.clone();
                    details = details.child(
                        div()
                            .h_flex()
                            .min_w_0()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(0xe4e8f0))
                            .bg(rgb(0xf8f9fc))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xa36932))
                                    .child("RSS"),
                            )
                            .child(
                                div()
                                    .id((element_id.clone(), "feed-url"))
                                    .debug_selector(|| format!("{}:feed", source.id))
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(tooltip_url.clone()).build(window, cx)
                                    })
                                    .child(
                                        Link::new((element_id.clone(), "feed-link"))
                                            .href(url.clone())
                                            .text_sm()
                                            .text_color(rgb(0x68738a))
                                            .truncate()
                                            .child(url.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .id((element_id.clone(), "copy-tooltip"))
                                    .debug_selector(|| format!("{}:copy", source.id))
                                    .flex_shrink_0()
                                    .tooltip(|window, cx| {
                                        Tooltip::new("Copy RSS URL").build(window, cx)
                                    })
                                    .child(
                                        Clipboard::new((element_id.clone(), "copy"))
                                            .value(url.clone()),
                                    ),
                            ),
                    );
                } else {
                    details = details.child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x7d8597))
                            .child("RSS feed will appear after the first sync"),
                    );
                }
                list = list.child(
                    div()
                        .debug_selector(|| format!("{}:card", source.id))
                        .h_flex()
                        .items_start()
                        .flex_shrink_0()
                        .gap_5()
                        .p_5()
                        .bg(rgb(0xffffff))
                        .rounded_xl()
                        .border_1()
                        .border_color(rgb(0xe8ecf3))
                        .child(cover)
                        .child(details),
                );
            }
            content = content.child(list);
        }
        content = content.child(
            div()
                .text_xs()
                .text_color(rgb(0x7d8597))
                .child(if state.busy {
                    "Syncing your subscriptions…"
                } else {
                    "Ready · Automatic refresh every hour"
                }),
        );
        div()
            .h_flex()
            .size_full()
            .bg(rgb(0xf8f9fc))
            .child(sidebar)
            .child(content)
    }
}

struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if path == "icons/refresh.svg" {
            return Ok(Some(std::borrow::Cow::Borrowed(include_bytes!(
                "../assets/refresh.svg"
            ))));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut assets = gpui_component_assets::Assets.list(path)?;
        if "icons/refresh.svg".starts_with(path) {
            assets.push("icons/refresh.svg".into());
        }
        Ok(assets)
    }
}

fn build_cover_images(state: &crate::engine::ViewState) -> HashMap<String, Arc<Image>> {
    state
        .covers
        .iter()
        .filter_map(|(id, cover)| {
            let format = ImageFormat::from_mime_type(cover.mime)?;
            Some((
                id.clone(),
                Arc::new(Image::from_bytes(format, cover.bytes.clone())),
            ))
        })
        .collect()
}

fn cover_artwork(image: Option<ImageSource>) -> Div {
    let image = match image {
        Some(image) => img(image)
            .size_full()
            .object_fit(ObjectFit::Cover)
            .with_fallback(cover_placeholder)
            .into_any_element(),
        None => cover_placeholder(),
    };
    div()
        .size(px(112.))
        .flex_shrink_0()
        .rounded_lg()
        .overflow_hidden()
        .child(image)
}

fn cover_placeholder() -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(112.))
        .flex_shrink_0()
        .rounded_lg()
        .bg(rgb(0xedf0f6))
        .text_color(rgb(0x7d8597))
        .text_2xl()
        .child("♫")
        .into_any_element()
}

pub fn run(engine: Engine) {
    let app = Application::new().with_assets(Assets);
    app.on_reopen(|cx| {
        show_window();
        cx.activate(true);
    });
    app.run(move |cx| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(None, size(px(1080.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Mazit".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(900.), px(650.))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    let mut view = MazitView::new(engine, window, cx);
                    view.menu_bar = Some(MenuBar::new());
                    window.on_window_should_close(cx, |_, _| {
                        hide_window();
                        false
                    });
                    view
                });
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("Open Mazit window");
        cx.activate(true);
    });
}

#[cfg(target_os = "macos")]
fn hide_window() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let app = NSApplication::sharedApplication(objc2::MainThreadMarker::new().unwrap());
    for window in app.windows() {
        if window.title().to_string() == "Mazit" {
            window.orderOut(None);
        }
    }
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}
#[cfg(target_os = "macos")]
fn show_window() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    let app = NSApplication::sharedApplication(objc2::MainThreadMarker::new().unwrap());
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    for window in app.windows() {
        if window.title().to_string() == "Mazit" {
            window.deminiaturize(None);
            window.makeKeyAndOrderFront(None);
        }
    }
}
#[cfg(target_os = "macos")]
fn hide_if_minimized() {
    let app =
        objc2_app_kit::NSApplication::sharedApplication(objc2::MainThreadMarker::new().unwrap());
    if app
        .windows()
        .iter()
        .any(|window| window.title().to_string() == "Mazit" && window.isMiniaturized())
    {
        hide_window();
    }
}
#[cfg(not(target_os = "macos"))]
fn hide_window() {}
#[cfg(not(target_os = "macos"))]
fn show_window() {}
#[cfg(not(target_os = "macos"))]
fn hide_if_minimized() {}
#[cfg(target_os = "macos")]
struct Activity(
    objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>,
);
#[cfg(target_os = "macos")]
impl Activity {
    fn new() -> Self {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        Self(
            NSProcessInfo::processInfo().beginActivityWithOptions_reason(
                NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
                &NSString::from_str("Keep podcast synchronization active in the menu bar"),
            ),
        )
    }
}
#[cfg(target_os = "macos")]
impl Drop for Activity {
    fn drop(&mut self) {
        unsafe {
            objc2_foundation::NSProcessInfo::processInfo().endActivity(&self.0);
        }
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests;
