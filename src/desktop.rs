use crate::{
    config,
    engine::{Command, Engine},
};
use gpui::*;
use gpui_component::{
    Disableable, Root, StyledExt,
    button::{Button, ButtonVariants},
    input::{Input, InputState},
};
use std::time::Duration;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

struct MazitView {
    engine: Engine,
    source: Entity<InputState>,
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
        window.on_window_should_close(cx, |_, _| {
            hide_window();
            false
        });
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if view
                    .update(cx, |view, cx| {
                        while let Ok(event) = MenuEvent::receiver().try_recv() {
                            if event.id == view.open_id {
                                show_window();
                                cx.activate(true);
                            }
                            if event.id == view.refresh_id {
                                view.engine.command(Command::Refresh(None));
                            }
                            if event.id == view.quit_id {
                                cx.quit();
                            }
                        }
                        hide_if_minimized();
                        cx.notify();
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
            _tray: tray,
            open_id: open.id().clone(),
            refresh_id: refresh.id().clone(),
            quit_id: quit.id().clone(),
            #[cfg(target_os = "macos")]
            _activity: Activity::new(),
        }
    }
    fn open_config(&self, cx: &mut Context<Self>) {
        self.engine.state.write().message = match config::open_in_editor() {
            Ok(()) => "Opened config.toml; save it, then select Reload config".into(),
            Err(error) => crate::redact(&format!("{error:#}")),
        };
        cx.notify();
    }
}
impl Render for MazitView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.engine.state.read().clone();
        let mut sidebar = div()
            .v_flex()
            .w(px(240.))
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
            );
        for source in &state.sources {
            sidebar = sidebar.child(
                div()
                    .v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .child(source.title.clone())
                    .child(div().text_xs().text_color(rgb(0x7d8597)).child(format!(
                        "{} / {} · {}",
                        source.uploaded, source.total, source.phase
                    ))),
            );
        }
        sidebar = sidebar.child(div().flex_1()).child(
            div()
                .text_xs()
                .child("Checks every hour\nContinues in the menu bar"),
        );
        let mut content = div()
            .v_flex()
            .flex_1()
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
                    .child("Your public base URL must serve RSS and audio files directly, without a login.")
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
                    .child(Input::new(&self.source))
                    .child(
                        Button::new("add")
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
                .flex_1();
            if state.sources.is_empty() {
                list=list.child(div().p_8().child("Add your first playlist or channel. Mazit downloads audio, prepares fast-start M4A files, and publishes RSS to your storage."));
            }
            for (index, source) in state.sources.iter().enumerate() {
                let id = source.id.clone();
                let feed = source.feed_url.clone();
                let mut card = div()
                    .v_flex()
                    .gap_3()
                    .p_5()
                    .bg(rgb(0xffffff))
                    .rounded_xl()
                    .child(div().text_xl().child(source.title.clone()))
                    .child(format!(
                        "{} of {} uploaded · {}",
                        source.uploaded, source.total, source.phase
                    ))
                    .child(
                        Button::new(("source-refresh", index))
                            .label("Refresh")
                            .disabled(state.busy)
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.engine.command(Command::Refresh(Some(id.clone())))
                            })),
                    );
                if let Some(error) = &source.error {
                    card = card.child(
                        div()
                            .text_color(rgb(0xb64c48))
                            .text_sm()
                            .child(error.clone()),
                    );
                }
                if let Some(url) = feed {
                    card = card.child(div().text_sm().child(url.clone())).child(
                        Button::new(("copy", index)).label("Copy RSS URL").on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()))
                            },
                        ),
                    );
                }
                list = list.child(card);
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

pub fn run(engine: Engine) {
    let app = Application::new();
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
                let view = cx.new(|cx| MazitView::new(engine, window, cx));
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
