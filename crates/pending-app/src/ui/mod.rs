//! The window.

mod board;
mod dialogs;
mod hero;
mod icons;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::Local;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use pending_core::WeatherReport;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use crate::activity::LookThrottle;
use crate::backend::{Backend, Command, Event, spawn_worker};
use crate::present::{Page, page};
use crate::state::ViewState;
use crate::theme;
use crate::view::{View, state_dir, view_path};

/// How often the snapshot is read while the window is shown.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// How often the weather is read; the daemon keeps it for as long.
const WEATHER_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// Matches the daemon's cooldown; the button comes back even if nothing
/// changed.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);
/// How often the window asks whether a newer version was published; the
/// daemon itself looks once a day.
const UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Everything the window knows, shared by its widgets.
pub struct Model {
    pub state: ViewState,
    pub view: View,
    pub view_path: std::path::PathBuf,
    /// The key of the state last drawn, so an unchanged snapshot does not
    /// rebuild the page.
    drawn: String,
    /// While set, the Refresh button stays off with this label.
    pub refresh_hold: Option<(Instant, String)>,
    pub weather: Option<WeatherReport>,
    /// A newer published version, when the daemon knows of one.
    pub update: Option<String>,
    pub source_line: String,
    pub icons: icons::Cache,
    looks: LookThrottle,
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
            model.view.save(&model.view_path.clone());
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
        let (key, state, view) = {
            let model = self.model.borrow();
            (model.state.key(), model.state.clone(), model.view.clone())
        };
        let page_view = state.snapshot().map(|snapshot| page(snapshot, &view));
        hero::update_controls(self, widgets, page_view.as_ref(), &view);
        let changed = self.model.borrow().drawn != key;
        if changed || force {
            let position = widgets.scroller.vadjustment().value();
            board::render(self, &widgets.page, &state, page_view.as_ref(), &view);
            self.model.borrow_mut().drawn = key;
            // Keep the reading position across a rebuild.
            let adjustment = widgets.scroller.vadjustment();
            gtk::glib::idle_add_local_once(move || adjustment.set_value(position));
        }
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

/// How long after opening the window its picture is taken: the first
/// snapshot and the logos have arrived by then.
const SCREENSHOT_AFTER: Duration = Duration::from_millis(3000);

/// Runs the GTK application over `backend` and returns its exit status.
/// With `screenshot`, the window is pictured into that file and closed.
pub fn run(runtime: &Runtime, backend: Backend, screenshot: Option<std::path::PathBuf>) -> i32 {
    let (commands, events) = spawn_worker(runtime, backend.client.clone());
    let view_path = view_path(&state_dir());
    let ctx = Rc::new(Ctx {
        model: RefCell::new(Model {
            state: ViewState::Loading,
            view: View::load(&view_path),
            view_path,
            drawn: String::new(),
            refresh_hold: None,
            weather: None,
            update: None,
            source_line: backend.source.describe(),
            icons: icons::Cache::default(),
            looks: LookThrottle::default(),
        }),
        commands,
        runtime_handle: runtime.handle().clone(),
        widgets: RefCell::new(None),
    });
    // The backend (and its config watch) lives as long as the window.
    let _backend = backend;

    let app = adw::Application::builder()
        .application_id(crate::APP_ID)
        .build();
    let events = RefCell::new(Some(events));
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        load_stylesheet();
        build_window(app, &ctx);
        if let Some(events) = events.borrow_mut().take() {
            pump_events(&ctx, events);
        }
        schedule_polling(&ctx);
        schedule_auto_refresh(&ctx);
        if let Some(path) = screenshot.clone() {
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
    i32::from(app.run_with_args::<&str>(&[]))
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
        .spacing(20)
        .margin_top(20)
        .margin_bottom(32)
        .margin_start(28)
        .margin_end(28)
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
        .default_width(1280)
        .default_height(860)
        .content(&content)
        .build();
    window.add_css_class("tasks-pending");
    // The weather is context, not content: it leaves narrow windows so the
    // clock keeps its room.
    let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        hero::WEATHER_MIN_WIDTH,
        adw::LengthUnit::Px,
    ));
    narrow.add_setter(&hero.weather_slot, "visible", Some(&false.to_value()));
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
                Event::Refreshed(result) => {
                    let label = match result {
                        Ok(()) => "Refreshing…".to_owned(),
                        Err(error) => format!("Refresh ({error})"),
                    };
                    ctx.model.borrow_mut().refresh_hold =
                        Some((Instant::now() + REFRESH_COOLDOWN, label));
                    ctx.redraw(true);
                    let ctx = ctx.clone();
                    gtk::glib::timeout_add_local_once(REFRESH_COOLDOWN, move || {
                        ctx.model.borrow_mut().refresh_hold = None;
                        ctx.redraw(true);
                    });
                }
                Event::Acted(Err(error)) => {
                    if let Some(widgets) = ctx.widgets.borrow().as_ref() {
                        hero::show_note(widgets, &format!("Could not apply the change ({error})"));
                    }
                }
                Event::Acted(Ok(())) => {}
                Event::Update(version) => {
                    ctx.model.borrow_mut().update = version;
                    ctx.redraw(true);
                }
            }
        }
    });
}

/// Reads the snapshot now and then every [`POLL_INTERVAL`] while the
/// window is shown; the weather every [`WEATHER_INTERVAL`].
fn schedule_polling(ctx: &Rc<Ctx>) {
    ctx.send(Command::Poll);
    {
        let ctx = ctx.clone();
        gtk::glib::timeout_add_local(POLL_INTERVAL, move || {
            if !is_hidden(&ctx) {
                ctx.send(Command::Poll);
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    {
        let ctx = ctx.clone();
        gtk::glib::timeout_add_local(WEATHER_INTERVAL, move || {
            if !is_hidden(&ctx) {
                ctx.send(Command::Weather);
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    let ctx = ctx.clone();
    gtk::glib::timeout_add_local(UPDATE_INTERVAL, move || {
        ctx.send(Command::Version);
        gtk::glib::ControlFlow::Continue
    });
}

/// Refreshes every source at the interval the settings ask for, while the
/// window is shown. Called again when the interval changes.
fn schedule_auto_refresh(ctx: &Rc<Ctx>) {
    thread_local! {
        static TIMER: RefCell<Option<gtk::glib::SourceId>> = const { RefCell::new(None) };
    }
    TIMER.with(|timer| {
        if let Some(id) = timer.borrow_mut().take() {
            id.remove();
        }
        let minutes = ctx.model.borrow().view.auto_refresh_minutes;
        if minutes == 0 {
            return;
        }
        let ctx = ctx.clone();
        let id =
            gtk::glib::timeout_add_local(Duration::from_secs(u64::from(minutes) * 60), move || {
                if !is_hidden(&ctx) {
                    ctx.send(Command::Refresh);
                }
                gtk::glib::ControlFlow::Continue
            });
        *timer.borrow_mut() = Some(id);
    });
}

fn is_hidden(ctx: &Ctx) -> bool {
    ctx.window().is_some_and(|window| window.is_suspended())
}

/// Renders the window as it is drawn into a PNG file: its background,
/// then its content (a toplevel itself paints nothing into a snapshot).
fn capture(window: &adw::ApplicationWindow, path: &std::path::Path) -> anyhow::Result<()> {
    use gtk::gdk::prelude::PaintableExt;
    let content = window
        .content()
        .ok_or_else(|| anyhow::anyhow!("the window has no content"))?;
    let width = f64::from(content.width());
    let height = f64::from(content.height());
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
    let paintable = gtk::WidgetPaintable::new(Some(&content));
    paintable.snapshot(&snapshot, width, height);
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
