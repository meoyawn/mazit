use super::*;
use crate::downloads::{Download, MAX_TRANSFERS, Phase, RangePhase};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

// gpui-component 0.5.1 uses a 16 px track; its width helper is private.
const SCROLLBAR_WIDTH: Pixels = px(16.);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DownloadFilter {
    All,
    Active,
    Queued,
    Failed,
    Complete,
}

impl DownloadFilter {
    fn matches(self, item: &Download) -> bool {
        match self {
            Self::All => true,
            Self::Active => item.phase.active(),
            Self::Queued => item.phase == Phase::Queued,
            Self::Failed => item.phase == Phase::Failed,
            Self::Complete => item.phase == Phase::Complete,
        }
    }
}

impl MazitView {
    pub(super) fn render_downloads(&self, cx: &mut Context<Self>) -> Div {
        let snapshot = self.engine.downloads.snapshot();
        let count = |filter: DownloadFilter| {
            snapshot
                .items
                .iter()
                .filter(|item| filter.matches(item))
                .count()
        };
        let mut tabs = div().h_flex().gap_2().flex_wrap();
        for (filter, label, id) in [
            (DownloadFilter::All, "All", "downloads-all"),
            (DownloadFilter::Active, "Active", "downloads-active"),
            (DownloadFilter::Queued, "Queued", "downloads-queued"),
            (DownloadFilter::Failed, "Failed", "downloads-failed"),
            (DownloadFilter::Complete, "Complete", "downloads-complete"),
        ] {
            let button = Button::new(id)
                .debug_selector(move || id.into())
                .small()
                .label(format!("{label} {}", count(filter)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.download_filter = filter;
                    this.download_scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }));
            tabs = tabs.child(if self.download_filter == filter {
                button.primary()
            } else {
                button.ghost()
            });
        }
        let active = count(DownloadFilter::Active);
        let slots = snapshot.slot_limit;
        let speed: u64 = snapshot.items.iter().map(Download::bytes_per_second).sum();
        let mut items = snapshot
            .items
            .into_iter()
            .filter(|item| self.download_filter.matches(item))
            .collect::<Vec<_>>();
        items.sort_by_key(|item| match item.phase {
            phase if phase.active() => 0,
            Phase::Failed => 1,
            Phase::Queued => 2,
            _ => 3,
        });
        let mut content = div()
            .debug_selector(|| "download-manager".into())
            .v_flex().flex_1().min_w_0().h_full().p_8().gap_4()
            .text_color(rgb(0x293248))
            .child(div().h_flex().justify_between().gap_3()
                .child(div().text_2xl().font_weight(FontWeight::BOLD).child("Downloads"))
                .child(Button::new("pause-downloads")
                    .debug_selector(|| "pause-downloads".into())
                    .label(if snapshot.paused { "Resume queue" } else { "Pause queue" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let paused = this.engine.downloads.snapshot().paused;
                        this.engine.downloads.set_paused(!paused);
                        cx.notify();
                    }))))
            .child(div().debug_selector(|| format!("download-slots:{slots}")).text_sm().text_color(rgb(0x68738a)).child(format!(
                "{active} active · {slots} slots · Auto · {}/s · All podcasts",
                bytes(speed)
            )))
            .child(div().text_xs().text_color(rgb(0x7d8597)).child(if snapshot.paused {
                "Queue paused. Active transfers finish uploading and remove their local files.".to_string()
            } else {
                format!("Adapts to network speed, up to {MAX_TRANSFERS} transfers. Local audio is deleted after upload.")
            }))
            .child(tabs);
        if items.is_empty() {
            content = content.child(
                div()
                    .flex_1()
                    .v_flex()
                    .justify_center()
                    .items_center()
                    .gap_3()
                    .debug_selector(|| "downloads-empty".into())
                    .child("No downloads here")
                    .child(div().text_sm().text_color(rgb(0x7d8597)).child(
                        if self.download_filter == DownloadFilter::All {
                            "Refresh a subscription to queue new episodes."
                        } else {
                            "Transfers appear here when their status changes."
                        },
                    )),
            );
        } else {
            content = content.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .pr(SCROLLBAR_WIDTH)
                    .child(
                        uniform_list("download-list", items.len(), move |range, _, _| {
                            range
                                .map(|index| download_row(&items[index]))
                                .collect::<Vec<_>>()
                        })
                        .size_full()
                        .track_scroll(self.download_scroll.clone()),
                    )
                    .child(
                        div()
                            .debug_selector(|| "downloads-scrollbar".into())
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(SCROLLBAR_WIDTH)
                            .child(
                                Scrollbar::vertical(&self.download_scroll)
                                    .scrollbar_show(ScrollbarShow::Always),
                            ),
                    ),
            );
        }
        content
            .child(
                div()
                    .h_flex()
                    .gap_4()
                    .text_xs()
                    .text_color(rgb(0x7d8597))
                    .child(legend("Waiting", 0xe4e8f0))
                    .child(legend("Receiving", 0x526bbe))
                    .child(legend("Saved", 0x36826c))
                    .child(legend("Retrying", 0xc38b3e)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x7d8597))
                    .child("Transfer history for this session · Completed episodes stay in S3"),
            )
    }
}

fn legend(label: &'static str, color: u32) -> Div {
    div()
        .h_flex()
        .gap_1p5()
        .child(div().size(px(7.)).rounded_sm().bg(rgb(color)))
        .child(label)
}

fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", value as f64 / (1024. * 1024. * 1024.))
    } else if value >= 1024 * 1024 {
        format!("{:.1} MiB", value as f64 / (1024. * 1024.))
    } else if value >= 1024 {
        format!("{:.0} KiB", value as f64 / 1024.)
    } else {
        format!("{value} B")
    }
}

fn download_row(item: &Download) -> Div {
    let color = match item.phase {
        Phase::Complete => 0x36826c,
        Phase::Failed => 0xb64c48,
        Phase::Queued => 0x7d8597,
        Phase::Retrying => 0xc38b3e,
        _ => 0x526bbe,
    };
    let speed = item.bytes_per_second();
    let progress = if let Some(percent) = (item.received() * 100).checked_div(item.total) {
        let mut text = format!("{percent}%");
        if speed > 0 {
            let seconds = item.total.saturating_sub(item.received()).div_ceil(speed);
            text.push_str(&format!(
                " · {}/s · {}:{:02} left",
                bytes(speed),
                seconds / 60,
                seconds % 60
            ));
        }
        text
    } else {
        if item.phase == Phase::Queued {
            "Waiting for a transfer slot".into()
        } else {
            "Waiting for audio metadata".into()
        }
    };
    let detail = if let Some(error) = &item.error {
        format!("Attempt {}/3 · {error}", item.attempt)
    } else {
        match item.phase {
            Phase::Downloading => "Receiving audio".into(),
            Phase::Complete => "Uploaded to S3 · Local audio removed".into(),
            Phase::Preparing => "Preparing fast-start M4A".into(),
            Phase::Uploading => "Saving to S3 before removing local audio".into(),
            Phase::Queued => "Starts automatically when a slot is available".into(),
            _ => format!("Attempt {}/3", item.attempt),
        }
    };
    div().h(px(146.)).pb_3().min_w_0().child(
        div()
            .debug_selector(|| format!("download:{}", item.id))
            .v_flex()
            .h_full()
            .min_w_0()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0xe4e8f0))
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .debug_selector(|| format!("download:{}:{:?}", item.id, item.phase))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(rgb(color))
                            .child(item.phase.label()),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .text_xs()
                    .text_color(rgb(0x68738a))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .child(item.source_title.clone()),
                    )
                    .child(div().flex_shrink_0().child(progress)),
            )
            .child(range_map(item))
            .child(
                div()
                    .id(SharedString::from(format!("download-detail:{}", item.id)))
                    .text_xs()
                    .text_color(rgb(if item.error.is_some() {
                        0xb64c48
                    } else {
                        0x7d8597
                    }))
                    .truncate()
                    .tooltip({
                        let detail = detail.clone();
                        move |window, cx| Tooltip::new(detail.clone()).build(window, cx)
                    })
                    .child(detail),
            ),
    )
}

fn range_map(item: &Download) -> Div {
    let mut map = div()
        .debug_selector(|| format!("download:{}:ranges", item.id))
        .h_flex()
        .h(px(12.))
        .w_full()
        .rounded_sm()
        .overflow_hidden()
        .bg(rgb(0xe4e8f0));
    // Preserve byte positions and bound UI work even for multi-gigabyte episodes.
    for (index, ranges) in item
        .ranges
        .chunks(item.ranges.len().div_ceil(96).max(1))
        .enumerate()
    {
        let start = ranges.first().unwrap().start;
        let end = ranges.last().unwrap().end;
        let received: u64 = ranges.iter().map(|range| range.received).sum();
        let color = if ranges
            .iter()
            .any(|range| range.phase == RangePhase::Retrying)
        {
            0xc38b3e
        } else if ranges.iter().any(|range| range.phase == RangePhase::Active) {
            0x526bbe
        } else {
            0x36826c
        };
        map = map.child(
            div()
                .debug_selector(|| format!("download:{}:range:{index}", item.id))
                .h_full()
                .w(relative(
                    (end - start + 1) as f32 / item.total.max(1) as f32,
                ))
                .border_r_1()
                .border_color(rgb(0xffffff))
                .bg(rgb(
                    if ranges
                        .iter()
                        .any(|range| range.phase == RangePhase::Retrying)
                    {
                        0xf2dfbc
                    } else if ranges.iter().any(|range| range.phase == RangePhase::Active) {
                        0xd5ddf6
                    } else {
                        0xe4e8f0
                    },
                ))
                .child(
                    div()
                        .debug_selector(|| format!("download:{}:received:{index}", item.id))
                        .h_full()
                        .w(relative(received as f32 / (end - start + 1) as f32))
                        .bg(rgb(color)),
                ),
        );
    }
    map
}
