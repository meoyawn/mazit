use super::{MazitView, cover_artwork};
use crate::{
    database::Database,
    engine::{Command, Engine, ViewState},
    network::Cover,
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, Image, ImageFormat, ImageSource, IntoElement,
    Modifiers, ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div,
    point, px, size,
};
use gpui_component::Root;
use std::{cell::RefCell, collections::HashMap, path::Path, rc::Rc, sync::Arc, time::Duration};
use tokio::sync::mpsc::UnboundedReceiver;

const LANDSCAPE: &[u8] = include_bytes!("fixtures/landscape.png");
const PORTRAIT: &[u8] = include_bytes!("fixtures/portrait.png");

fn library() -> ViewState {
    let db = Database::open(Path::new(":memory:")).unwrap();
    let id = db
        .add(
            "playlist",
            "test",
            "https://www.youtube.com/playlist?list=test",
        )
        .unwrap();
    let mut source = db.source(&id).unwrap();
    source.title = "Covers".into();
    source.feed_url = Some(format!(
        "https://audio.example.com/{}/rss.xml",
        "long-library-name/".repeat(12)
    ));
    source.total = 8;
    source.uploaded = 8;
    ViewState {
        sources: vec![source],
        covers: HashMap::from([(id, Arc::new(Cover::from_bytes(LANDSCAPE.to_vec()).unwrap()))]),
        configured: true,
        ..Default::default()
    }
}

fn setup(
    cx: &mut TestAppContext,
    state: ViewState,
) -> (
    Entity<MazitView>,
    Engine,
    UnboundedReceiver<Command>,
    &mut VisualTestContext,
) {
    cx.update(gpui_component::init);
    let (engine, commands) = Engine::for_test(state);
    let mut view = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let library = cx.new(|cx| MazitView::new(engine.clone(), window, cx));
        view = Some(library.clone());
        Root::new(library, window, cx)
    });
    cx.simulate_resize(size(px(1080.), px(800.)));
    draw(cx);
    (view.unwrap(), engine, commands, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear());
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    draw(cx);
    let bounds = cx.debug_bounds(selector).expect(selector);
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    cx.simulate_click(bounds.center(), Modifiers::none());
}

#[gpui::test]
fn library_links_and_copy_use_the_complete_urls(cx: &mut TestAppContext) {
    let state = library();
    let youtube_url = state.sources[0].url.clone();
    let feed_url = state.sources[0].feed_url.clone().unwrap();
    let (_, _, mut commands, cx) = setup(cx, state);
    click(cx, "playlist:test:youtube");
    assert_eq!(cx.opened_url(), Some(youtube_url));
    click(cx, "playlist:test:copy");
    assert_eq!(
        cx.read_from_clipboard().unwrap().text(),
        Some(feed_url.clone())
    );
    click(cx, "playlist:test:feed");
    assert_eq!(cx.opened_url(), Some(feed_url));
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn refresh_dispatches_the_selected_source_and_respects_busy_state(cx: &mut TestAppContext) {
    let (_, engine, mut commands, cx) = setup(cx, library());
    click(cx, "playlist:test:refresh");
    assert!(
        matches!(commands.try_recv().unwrap(), Command::Refresh(Some(id)) if id == "playlist:test")
    );
    click(cx, "refresh-all");
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::Refresh(None)
    ));

    engine.state.write().busy = true;
    cx.background_executor
        .advance_clock(Duration::from_millis(500));
    click(cx, "playlist:test:refresh");
    click(cx, "refresh-all");
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn typing_and_adding_a_source_dispatches_once_and_clears_input(cx: &mut TestAppContext) {
    let (view, _, mut commands, cx) = setup(cx, library());
    let url = "https://www.youtube.com/playlist?list=PLexample";
    click(cx, "source-input");
    cx.simulate_input(url);
    click(cx, "add-source");
    assert!(matches!(commands.try_recv().unwrap(), Command::Add(value) if value == url));
    assert!(cx.read_entity(&view, |view, cx| view.source.read(cx).value().is_empty()));
    click(cx, "add-source");
    assert!(commands.try_recv().is_err());
}

#[gpui::test]
fn artwork_and_controls_stay_inside_the_card_at_supported_window_sizes(cx: &mut TestAppContext) {
    let (_, _, _, cx) = setup(cx, library());
    for width in [900., 1080., 1440.] {
        cx.simulate_resize(size(px(width), px(800.)));
        draw(cx);
        let sidebar = cx.debug_bounds("sidebar").unwrap();
        let card = cx.debug_bounds("playlist:test:card").unwrap();
        let cover = cx.debug_bounds("playlist:test:cover").unwrap();
        let details = cx.debug_bounds("playlist:test:details").unwrap();
        let copy = cx.debug_bounds("playlist:test:copy").unwrap();
        assert_eq!(cover.size, size(px(112.), px(112.)));
        assert!(card.left() >= sidebar.right());
        assert!(cover.left() > card.left() && cover.top() > card.top());
        assert!(cover.right() < details.left());
        assert!(copy.right() < card.right());
        assert!(copy.bottom() < card.bottom());
    }
}

#[gpui::test]
fn cover_replacement_and_removal_reach_the_rendered_view(cx: &mut TestAppContext) {
    let (view, engine, _, cx) = setup(cx, library());
    engine.state.write().covers.insert(
        "playlist:test".into(),
        Arc::new(Cover::from_bytes(PORTRAIT.to_vec()).unwrap()),
    );
    cx.background_executor
        .advance_clock(Duration::from_millis(500));
    draw(cx);
    cx.read_entity(&view, |view, _| {
        assert_eq!(view.cover_images["playlist:test"].bytes, PORTRAIT)
    });
    engine.state.write().covers.clear();
    cx.background_executor
        .advance_clock(Duration::from_millis(500));
    draw(cx);
    assert!(cx.read_entity(&view, |view, _| view.cover_images.is_empty()));
    assert_eq!(
        cx.debug_bounds("playlist:test:cover").unwrap().size,
        size(px(112.), px(112.))
    );
}

#[gpui::test]
fn wide_and_tall_images_are_painted_inside_the_square_clip(cx: &mut TestAppContext) {
    for bytes in [LANDSCAPE, PORTRAIT] {
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, bytes.to_vec()));
        let masks = Rc::new(RefCell::new(Vec::new()));
        let recorded = masks.clone();
        let source = ImageSource::from(move |window: &mut Window, cx: &mut App| {
            let image = image.clone().use_render_image(window, cx)?;
            recorded.borrow_mut().push(window.content_mask().bounds);
            Some(Ok(image))
        });
        let (_, cx) = cx.add_window_view(|_, _| Artwork { source });
        draw(cx);
        // The final image-source call is the Img paint pass, inside its parent's content mask.
        assert_eq!(
            masks.borrow().last().copied(),
            Some(Bounds::new(
                point(px(30.), px(40.)),
                size(px(112.), px(112.))
            ))
        );
    }
}

struct Artwork {
    source: ImageSource,
}

impl Render for Artwork {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .pl(px(30.))
            .pt(px(40.))
            .child(cover_artwork(Some(self.source.clone())))
    }
}
