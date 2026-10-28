//! The window.

mod board;
mod dialogs;
mod hero;
mod icons;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use chrono::Local;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use pending_core::WeatherReport;
use pending_runtime::live::Env;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use crate::activity::LookThrottle;
use crate::backend::{self, Command, Event, spawn_worker};
use crate::present::{
    BoardView, Page, Timer, auto_refresh_every, board_view, lasting, on_timer, page, refreshed,
};
use crate::state::ViewState;
use crate::theme;
use crate::view::View;

/// How often the snapshot is read while the window is shown.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// How often the weather is read; the daemon keeps it for as long.
const WEATHER_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How often the window asks whether a newer version was published; the
/// daemon itself looks once a day.
const UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// The window's size when first opened: wide enough for the weather, which
/// leaves narrower windows (see [`hero::WEATHER_MIN_WIDTH`]).
const DEFAULT_WIDTH: i32 = 1440;
const DEFAULT_HEIGHT: i32 = 860;
/// The page's margins and the space between its parts, in pixels: a little
/// less above, where the header sits, than below and at the sides.
const PAGE_TOP: i32 = 20;
const PAGE_BOTTOM: i32 = 32;
const PAGE_SIDE: i32 = 28;
const PAGE_SPACING: i32 = 20;

/// Everything the window knows, shared by its widgets.
pub struct Model {
    pub state: ViewState,
    pub view: View,
    /// Where the view is remembered; `None` for the sandbox, a screenshot,
    /// or an environment that names no state directory, and it then lasts
    /// as long as the window.
    view_path: Option<PathBuf>,
    /// The board last drawn, so a snapshot that changed nothing on it does
    /// not rebuild the page.
    drawn: Option<BoardView>,
    /// While set, the Refresh button stays off with this label.
    pub refresh_hold: Option<(Instant, String)>,
    pub weather: Option<WeatherReport>,
    /// A newer published version, when the daemon knows of one.
    pub update: Option<String>,
    pub source_line: String,
    pub icons: icons::Cache,
    looks: LookThrottle,
    /// The auto-refresh timer, replaced when the interval changes.
    auto_refresh: Option<gtk::glib::SourceId>,
}

/// Shared by every widget: the model, the way to the dashboard, and the
/// widgets that get updated in place.
pub struct Ctx {
    pub model: RefCell<Model>,
    pub commands: UnboundedSender<Command>,
    pub runtime_handle: tokio::runtime::Handle,
    pub widgets: RefCell<Option<Widgets>>,
}

/// The widgets updated in place; the page under them is rebuilt.
pub struct Widgets {
    pub window: adw::ApplicationWindow,
    pub hero: hero::Hero,
    pub page: gtk::Box,
    pub scroller: gtk::ScrolledWindow,
}

impl Ctx {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Changes the view, remembers it and redraws.
    pub fn change_view(self: &Rc<Self>, change: impl FnOnce(&mut View)) {
        let interval = {
            let mut model = self.model.borrow_mut();
            let before = model.view.auto_refresh_minutes;
            change(&mut model.view);
            if let Some(path) = &model.view_path {
                model.view.save(path);
            }
            (before, model.view.auto_refresh_minutes)
        };
        self.redraw(true);
        if interval.0 != interval.1 {
            schedule_auto_refresh(self);
        }
    }

    /// Draws the page and controls for the current model; `force` draws
    /// even when the snapshot did not change (the view did).
    pub fn redraw(self: &Rc<Self>, force: bool) {
        let widgets = self.widgets.borrow();
        let Some(widgets) = widgets.as_ref() else {
            return;
        };
        let (state, view) = {
            let model = self.model.borrow();
            (model.state.clone(), model.view.clone())
        };
        let page_view = state.snapshot().map(|snapshot| page(snapshot, &view));
        hero::update_controls(self, widgets, page_view.as_ref(), &view);
        let board = board_view(&state, page_view.as_ref());
        let changed = self.model.borrow().drawn.as_ref() != Some(&board);
        if changed || force {
            let position = widgets.scroller.vadjustment().value();
            board::render(self, &widgets.page, &board);
            self.model.borrow_mut().drawn = Some(board);
            // Keep the reading position across a rebuild.
            let adjustment = widgets.scroller.vadjustment();
            gtk::glib::idle_add_local_once(move || adjustment.set_value(position));
        }
    }

    /// Updates the controls for the current model without rebuilding the
    /// page: for changes that only the hero shows.
    pub fn refresh_controls(self: &Rc<Self>) {
        let widgets = self.widgets.borrow();
        let Some(widgets) = widgets.as_ref() else {
            return;
        };
        let (state, view) = {
            let model = self.model.borrow();
            (model.state.clone(), model.view.clone())
        };
        let page_view = state.snapshot().map(|snapshot| page(snapshot, &view));
        hero::update_controls(self, widgets, page_view.as_ref(), &view);
    }

    /// The page for the current model, if there is a snapshot.
    pub fn current_page(&self) -> Option<Page> {
        let model = self.model.borrow();
        model
            .state
            .snapshot()
            .map(|snapshot| page(snapshot, &model.view))
    }

    pub fn window(&self) -> Option<adw::ApplicationWindow> {
        self.widgets.borrow().as_ref().map(|w| w.window.clone())
    }

    /// Activity on the window is "looking": the daemon flags what changed
    /// since the previous sitting, so the snapshot is read again after
    /// reporting it. Reported at most once a minute.
    pub fn looked(&self) {
        if self.model.borrow_mut().looks.due(Instant::now()) {
            self.send(Command::Look);
        }
    }
}

/// A label that wraps onto more lines; the page and the dialogs build
/// their texts from it.
fn wrapped(label: &str, classes: &[&str]) -> gtk::Label {
    let widget = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    for class in classes {
        widget.add_css_class(class);
    }
    widget
}

/// When a screenshot's scene is set up (cards marked or snoozed): the
/// first snapshot has arrived by then.
const SCENE_AFTER: Duration = Duration::from_millis(2500);
/// When a screenshot's dialog opens: the scene's changes are back from
/// the dashboard by then.
const DIALOG_AFTER: Duration = Duration::from_millis(4200);
/// When the picture is taken: the logos have arrived by then.
const SCREENSHOT_AFTER: Duration = Duration::from_millis(6000);

/// Finds the dashboard, opens the window over it and runs until it closes,
/// returning the exit status. The view is remembered at `view_path`, or
/// for as long as the window when there is none. With `screenshot`, the
/// window shows that scene, is pictured into that file and closed. A
/// `unique` application hands over to an instance already running
/// (opening the app twice focuses the window) before looking for the
/// dashboard, so the second launch starts no sources of its own; the
/// sandbox and a screenshot always open their own window.
pub fn run(
    runtime: &Runtime,
    options: backend::Options,
    env: Env,
    view_path: Option<PathBuf>,
    screenshot: Option<(PathBuf, crate::Scene)>,
    unique: bool,
) -> anyhow::Result<i32> {
    let flags = if unique {
        gtk::gio::ApplicationFlags::FLAGS_NONE
    } else {
        gtk::gio::ApplicationFlags::NON_UNIQUE
    };
    let app = adw::Application::builder()
        .application_id(crate::APP_ID)
        .flags(flags)
        .build();
    app.register(gtk::gio::Cancellable::NONE)
        .context("registering the application")?;
    if app.is_remote() {
        // Another instance owns the window: `run` asks it to present the
        // window and returns.
        return Ok(i32::from(app.run_with_args::<&str>(&[])));
    }

    let backend = backend::resolve(runtime, options, env)?;
    let (commands, events) = spawn_worker(runtime, backend.client.clone());
    let ctx = Rc::new(Ctx {
        model: RefCell::new(Model {
            state: ViewState::Loading,
            view: view_path.as_deref().map(View::load).unwrap_or_default(),
            view_path,
            drawn: None,
            refresh_hold: None,
            weather: None,
            update: None,
            source_line: backend.source.describe(),
            icons: icons::Cache::default(),
            looks: LookThrottle::default(),
            auto_refresh: None,
        }),
        commands,
        runtime_handle: runtime.handle().clone(),
        widgets: RefCell::new(None),
    });
    // The backend (and its config watch) lives as long as the window.
    let _backend = backend;

    let events = RefCell::new(Some(events));
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        adw::StyleManager::default().set_color_scheme(theme::color_scheme(
            screenshot.as_ref().map(|(_, scene)| *scene),
        ));
        load_stylesheet();
        build_window(app, &ctx);
        if let Some(events) = events.borrow_mut().take() {
            pump_events(&ctx, events);
        }
        schedule_polling(&ctx);
        schedule_auto_refresh(&ctx);
        if let Some((path, scene)) = screenshot.clone() {
            stage_scene(&ctx, scene);
            let app = app.clone();
            let ctx = ctx.clone();
            gtk::glib::timeout_add_local_once(SCREENSHOT_AFTER, move || {
                let Some(window) = ctx.window() else {
                    app.quit();
                    return;
                };
                // On the next frame, so the picture is of a drawn window;
                // a window the compositor suspended never gets one.
                let on_frame = app.clone();
                window.queue_draw();
                window.add_tick_callback(move |window, _| {
                    match capture(window, &path) {
                        Ok(()) => eprintln!("wrote {}", path.display()),
                        Err(error) => eprintln!("could not picture the window: {error}"),
                    }
                    on_frame.quit();
                    gtk::glib::ControlFlow::Break
                });
                gtk::glib::timeout_add_local_once(SCREENSHOT_AFTER, move || {
                    eprintln!(
                        "the window is not being drawn (suspended: {}); no picture",
                        window.is_suspended()
                    );
                    app.quit();
                });
            });
        }
    });
    // GTK must not read the process arguments: the CLI already did.
    Ok(i32::from(app.run_with_args::<&str>(&[])))
}

/// Loads the stylesheet for the current scheme, and again when it changes.
fn load_stylesheet() {
    let provider = gtk::CssProvider::new();
    let manager = adw::StyleManager::default();
    let apply = {
        let provider = provider.clone();
        move |manager: &adw::StyleManager| {
            let palette = if manager.is_dark() {
                &theme::DARK
            } else {
                &theme::LIGHT
            };
            provider.load_from_string(&theme::stylesheet(palette));
        }
    };
    apply(&manager);
    manager.connect_dark_notify(apply);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_window(app: &adw::Application, ctx: &Rc<Ctx>) {
    let hero = hero::build(ctx);
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(PAGE_SPACING)
        .margin_top(PAGE_TOP)
        .margin_bottom(PAGE_BOTTOM)
        .margin_start(PAGE_SIDE)
        .margin_end(PAGE_SIDE)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&page)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&hero.root);
    content.append(&scroller);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("TasksPending")
        .default_width(DEFAULT_WIDTH)
        .default_height(DEFAULT_HEIGHT)
        .content(&content)
        .build();
    window.add_css_class("tasks-pending");
    // The weather is context, not content: it leaves narrow windows so the
    // clock keeps its room.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        hero::NARROW_MAX_WIDTH,
        adw::LengthUnit::Px,
    ));
    narrow.add_setter(&hero.weather.slot, "visible", Some(&false.to_value()));
    window.add_breakpoint(narrow);
    watch_activity(ctx, &window);
    {
        let ctx = ctx.clone();
        window.connect_suspended_notify(move |window| {
            if !window.is_suspended() {
                ctx.send(Command::Poll);
                ctx.send(Command::Weather);
            }
        });
    }
    *ctx.widgets.borrow_mut() = Some(Widgets {
        window: window.clone(),
        hero,
        page,
        scroller,
    });
    window.present();
    // Opening the window is looking at it.
    ctx.looked();
    ctx.send(Command::Weather);
    ctx.send(Command::Version);
    ctx.redraw(true);
}

/// Clicks, keys and scrolling anywhere in the window count as looking;
/// the pointer merely crossing it does not.
fn watch_activity(ctx: &Rc<Ctx>, window: &adw::ApplicationWindow) {
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ctx = ctx.clone();
        click.connect_pressed(move |_, _, _, _| ctx.looked());
    }
    window.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ctx = ctx.clone();
        keys.connect_key_pressed(move |_, _, _, _| {
            ctx.looked();
            gtk::glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ctx = ctx.clone();
        scroll.connect_scroll(move |_, _, _| {
            ctx.looked();
            gtk::glib::Propagation::Proceed
        });
    }
    window.add_controller(scroll);
}

/// Applies what the dashboard says, on the GTK thread.
fn pump_events(ctx: &Rc<Ctx>, mut events: tokio::sync::mpsc::UnboundedReceiver<Event>) {
    let ctx = ctx.clone();
    gtk::glib::spawn_future_local(async move {
        while let Some(event) = events.recv().await {
            match event {
                Event::Snapshot(result) => {
                    {
                        let mut model = ctx.model.borrow_mut();
                        let state = std::mem::replace(&mut model.state, ViewState::Loading);
                        model.state = state.next(result, Local::now());
                    }
                    ctx.redraw(false);
                }
                Event::Weather(report) => {
                    ctx.model.borrow_mut().weather = report;
                    if let Some(widgets) = ctx.widgets.borrow().as_ref() {
                        hero::update_weather(&ctx, widgets);
                    }
                }
                Event::Refreshed { manual, result } => {
                    let shown = refreshed(manual, &result);
                    if let Some(note) = &shown.note
                        && let Some(widgets) = ctx.widgets.borrow().as_ref()
                    {
                        hero::show_note(widgets, note);
                    }
                    if let Some((wait, label)) = shown.hold {
                        ctx.model.borrow_mut().refresh_hold = Some((Instant::now() + wait, label));
                        let ctx = ctx.clone();
                        gtk::glib::timeout_add_local_once(wait, move || {
                            let mut model = ctx.model.borrow_mut();
                            let hold = model.refresh_hold.take();
                            model.refresh_hold = lasting(hold, Instant::now());
                            drop(model);
                            ctx.refresh_controls();
                        });
                    }
                    ctx.refresh_controls();
                }
                Event::Acted(Err(error)) => {
                    if let Some(widgets) = ctx.widgets.borrow().as_ref() {
                        hero::show_note(widgets, &format!("Could not apply the change ({error})"));
                    }
                    // No snapshot follows a refused change: rebuild the
                    // page as it was, so the button pressed is usable again.
                    ctx.redraw(true);
                }
                Event::Acted(Ok(())) => {}
                Event::Update(version) => {
                    ctx.model.borrow_mut().update = version;
                    ctx.refresh_controls();
                }
            }
        }
    });
}

/// Reads the snapshot now and then every [`POLL_INTERVAL`] while the
/// window is shown; the weather every [`WEATHER_INTERVAL`].
fn schedule_polling(ctx: &Rc<Ctx>) {
    ctx.send(Command::Poll);
    repeat(ctx, POLL_INTERVAL, Timer::Poll);
    repeat(ctx, WEATHER_INTERVAL, Timer::Weather);
    repeat(ctx, UPDATE_INTERVAL, Timer::Version);
}

/// Fires `timer` every `interval`, sending what it asks of the daemon.
fn repeat(ctx: &Rc<Ctx>, interval: Duration, timer: Timer) -> gtk::glib::SourceId {
    let ctx = ctx.clone();
    gtk::glib::timeout_add_local(interval, move || {
        if let Some(command) = on_timer(timer, is_hidden(&ctx)) {
            ctx.send(command);
        }
        gtk::glib::ControlFlow::Continue
    })
}

/// Refreshes every source at the interval the settings ask for, while the
/// window is shown. Called again when the interval changes.
fn schedule_auto_refresh(ctx: &Rc<Ctx>) {
    let minutes = {
        let mut model = ctx.model.borrow_mut();
        if let Some(timer) = model.auto_refresh.take() {
            timer.remove();
        }
        model.view.auto_refresh_minutes
    };
    let Some(interval) = auto_refresh_every(minutes) else {
        return;
    };
    let timer = repeat(ctx, interval, Timer::AutoRefresh);
    ctx.model.borrow_mut().auto_refresh = Some(timer);
}

fn is_hidden(ctx: &Ctx) -> bool {
    ctx.window().is_some_and(|window| window.is_suspended())
}

/// Sets a screenshot's scene up once the first snapshot is in: two cards
/// marked for the board, one snoozed for the snoozed list, and the dialog
/// opened a moment later.
fn stage_scene(ctx: &Rc<Ctx>, scene: crate::Scene) {
    use crate::Scene;
    let ctx = ctx.clone();
    gtk::glib::timeout_add_local_once(SCENE_AFTER, move || {
        let ids: Vec<String> = ctx
            .model
            .borrow()
            .state
            .snapshot()
            .map(|snapshot| {
                snapshot
                    .boards
                    .iter()
                    .flat_map(|board| &board.groups)
                    .flat_map(|group| &group.columns)
                    .filter_map(|column| column.cards.first())
                    .map(|card| card.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        match scene {
            Scene::Board | Scene::Light | Scene::Settings | Scene::Sources => {
                for id in ids.into_iter().take(2) {
                    ctx.send(Command::SetMark { id, marked: true });
                }
            }
            Scene::Snoozed => {
                if let Some(id) = ids.into_iter().next() {
                    ctx.send(Command::Snooze { id, until: None });
                }
            }
        }
        let ctx = ctx.clone();
        gtk::glib::timeout_add_local_once(DIALOG_AFTER - SCENE_AFTER, move || match scene {
            Scene::Sources => dialogs::show_sources(&ctx),
            Scene::Settings => dialogs::show_settings(&ctx),
            Scene::Snoozed => dialogs::show_snoozed(&ctx),
            Scene::Board | Scene::Light => {}
        });
    });
}

/// Renders the window as it is drawn into a PNG file: its background,
/// then everything in it (content and dialogs), snapshotted through the
/// window the way GTK draws it (a toplevel itself paints nothing into a
/// snapshot, and a widget paintable only has a picture a frame after it
/// is made).
fn capture(window: &adw::ApplicationWindow, path: &std::path::Path) -> anyhow::Result<()> {
    let content = window
        .first_child()
        .ok_or_else(|| anyhow::anyhow!("the window has no content"))?;
    let parent = window.clone().upcast::<gtk::Widget>();
    let width = f64::from(content.width());
    let height = f64::from(content.height());
    // Headless (Broadway without a client) the frame clock does not lay
    // widgets out between frames, so a dialog opened after the first
    // layout has no allocation yet: lay the whole tree out now.
    content.allocate(content.width(), content.height(), -1, None);
    let palette = if adw::StyleManager::default().is_dark() {
        &theme::DARK
    } else {
        &theme::LIGHT
    };
    let background = gtk::gdk::RGBA::parse(palette.bg)
        .map_err(|_| anyhow::anyhow!("bad background colour {}", palette.bg))?;
    let snapshot = gtk::Snapshot::new();
    snapshot.append_color(
        &background,
        &gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32),
    );
    parent.snapshot_child(&content, &snapshot);
    let node = snapshot
        .to_node()
        .ok_or_else(|| anyhow::anyhow!("the window drew nothing"))?;
    let renderer = window
        .renderer()
        .ok_or_else(|| anyhow::anyhow!("the window has no renderer yet"))?;
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window opens wide enough for the weather: the breakpoint that
    /// hides it counts its own width in, so it must end below the
    /// narrowest width that shows the weather.
    #[test]
    fn opens_wide_enough_for_the_weather() {
        const { assert!(DEFAULT_WIDTH as f64 >= hero::WEATHER_MIN_WIDTH) };
        const { assert!(hero::NARROW_MAX_WIDTH < hero::WEATHER_MIN_WIDTH) };
    }
}
