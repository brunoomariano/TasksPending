//! The window.

mod board;
mod dialogs;
mod hero;
mod icons;
mod keys;
mod tabs;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use chrono::{Local, Utc};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use pending_core::WeatherReport;
use pending_runtime::live::Env;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender;

use crate::Scene;
use crate::activity::LookThrottle;
use crate::alert::{self, Alert, notification};
use crate::backend::{self, Command, Event, spawn_worker};
use crate::dropdown::{self, Close, Hyprland, Toggle};
use crate::omarchy_bar;
use crate::present::{
    BarState, BoardView, CardAction, CardView, Page, Tab, Timer, auto_refresh_every, board_view,
    lasting, on_timer, page, refreshed,
};
use crate::session::{Session, Todos};
use crate::snooze::until;
use crate::state::ViewState;
use crate::theme::{self, OmarchyTheme, Palette};
use crate::view::{Stage, Theme, View};

/// How often the snapshot is read while the window is shown.
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// How often the weather is read; the daemon keeps it for as long.
const WEATHER_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How often the window asks whether a newer version was published; the
/// daemon itself looks once a day.
const UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How often the alerts are looked at, by the wall clock: an alert rings
/// within this of its time, also after the machine slept through it.
const ALERT_TICK: Duration = Duration::from_secs(30);
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
    /// The tab in front; the window opens on the Activity.
    pub tab: Tab,
    /// What lasts as long as the window: the alerts.
    pub session: Session,
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
    /// Omarchy's theme as last read, while the window follows it.
    pub omarchy: Option<Result<OmarchyTheme, String>>,
    /// The icon on Omarchy's bar while Omarchy puts it on or takes it
    /// off, and why it did not when it refused.
    pub bar_change: Option<BarState>,
    /// The icon as last put on or taken off, and when, until Omarchy's
    /// settings say the same (see [`crate::present::bar_view`]).
    pub bar_wanted: Option<(bool, std::time::Instant)>,
}

impl Model {
    /// The Omarchy theme drawn: the one read, while the window follows
    /// Omarchy and there is one.
    fn omarchy_drawn(&self) -> Option<&OmarchyTheme> {
        self.omarchy.as_ref()?.as_ref().ok()
    }
}

/// Shared by every widget: the model, the way to the dashboard, and the
/// widgets that get updated in place.
pub struct Ctx {
    pub model: RefCell<Model>,
    pub commands: UnboundedSender<Command>,
    pub runtime_handle: tokio::runtime::Handle,
    pub widgets: RefCell<Option<Widgets>>,
    pub styling: Styling,
    /// The page of the Settings dialog while it is open.
    pub settings_page: gtk::glib::WeakRef<gtk::Box>,
    /// Hyprland, which places the window as a drop-down; `None` off it.
    pub hyprland: Option<Hyprland>,
    /// The user's home, where Omarchy keeps its settings and plugins;
    /// `None` for the sandbox and a picture, which leave them alone.
    pub home: Option<PathBuf>,
    /// Omarchy's shell is here to carry the icon on its bar.
    pub omarchy_shell: bool,
    /// The file that tells the icon on Omarchy's bar which alerts were
    /// missed; `None` for the sandbox and a picture, which leave it alone.
    pub bar_signal: Option<PathBuf>,
    /// The cards on the page by column, top to bottom, for the keys that
    /// move between them and act on the one in focus.
    pub cards: RefCell<Vec<Vec<board::Slot>>>,
    /// Delete pressed once on a todo, waiting for the second press.
    pub armed: RefCell<crate::nav::Armed>,
    /// The keys down, to tell a press from its repeats.
    pub held: RefCell<crate::nav::Held>,
}

/// How the window is coloured.
pub struct Styling {
    /// The stylesheet, refilled when the palette changes.
    provider: gtk::CssProvider,
    /// What a screenshot shows: it can ask for the light scheme, or for
    /// the sample Omarchy theme.
    scene: Option<Scene>,
    /// Where Omarchy keeps the theme it applied.
    omarchy_dirs: Vec<PathBuf>,
    /// Watches Omarchy switch themes, for as long as the window.
    switches: RefCell<Option<gtk::gio::FileMonitor>>,
    /// The restyle waiting for a switch to settle.
    settling: RefCell<Option<gtk::glib::SourceId>>,
}

/// The widgets updated in place; the page under them is rebuilt.
pub struct Widgets {
    pub window: adw::ApplicationWindow,
    pub hero: hero::Hero,
    pub tabs: tabs::Tabs,
    pub page: gtk::Box,
    pub scroller: gtk::ScrolledWindow,
}

impl Ctx {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Changes the view, remembers it and redraws.
    pub fn change_view(self: &Rc<Self>, change: impl FnOnce(&mut View)) {
        let (interval, theme) = {
            let mut model = self.model.borrow_mut();
            let before = (model.view.auto_refresh_minutes, model.view.theme);
            change(&mut model.view);
            if let Some(path) = &model.view_path {
                model.view.save(path);
            }
            (
                (before.0, model.view.auto_refresh_minutes),
                (before.1, model.view.theme),
            )
        };
        self.redraw(true);
        if interval.0 != interval.1 {
            schedule_auto_refresh(self);
        }
        if theme.0 != theme.1 {
            self.restyle();
        }
    }

    /// Colours the window as the view asks: reads Omarchy's theme again
    /// while following it, then sets the scheme and fills the stylesheet.
    pub fn restyle(&self) {
        let follows = self.model.borrow().view.theme == Theme::Omarchy;
        let omarchy = follows.then(|| self.read_omarchy());
        if let Some(Err(why)) = &omarchy {
            tracing::warn!(%why, "Omarchy's theme not followed");
        }
        let scheme = {
            let mut model = self.model.borrow_mut();
            model.omarchy = omarchy;
            theme::color_scheme(self.styling.scene, model.omarchy_drawn())
        };
        // A scheme that turns dark or light fills the stylesheet from the
        // notify handler too; the palette can change without that.
        adw::StyleManager::default().set_color_scheme(scheme);
        self.fill_stylesheet();
    }

    /// Omarchy's theme, or the sample one for its screenshot.
    fn read_omarchy(&self) -> Result<OmarchyTheme, String> {
        if self.styling.scene == Some(Scene::Omarchy) {
            theme::parse_omarchy(theme::SAMPLE)
        } else {
            theme::read_omarchy(&self.styling.omarchy_dirs)
        }
    }

    /// The palette drawn now.
    pub fn palette(&self) -> Palette {
        theme::palette(
            adw::StyleManager::default().is_dark(),
            self.model.borrow().omarchy_drawn(),
        )
    }

    /// Fills the stylesheet with the palette drawn now.
    fn fill_stylesheet(&self) {
        self.styling
            .provider
            .load_from_string(&theme::stylesheet(&self.palette()));
    }

    /// Draws the page and controls for the current model; `force` draws
    /// even when the snapshot did not change (the view did).
    pub fn redraw(self: &Rc<Self>, force: bool) {
        let widgets = self.widgets.borrow();
        let Some(widgets) = widgets.as_ref() else {
            return;
        };
        let (state, view, tab, page_view) = self.current();
        hero::update_controls(self, widgets, page_view.as_ref(), &view);
        tabs::update(
            &widgets.tabs,
            &crate::present::tabs(page_view.as_ref()),
            tab,
        );
        let board = board_view(&state, page_view.as_ref(), tab);
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
        let (_, view, tab, page_view) = self.current();
        hero::update_controls(self, widgets, page_view.as_ref(), &view);
        tabs::update(
            &widgets.tabs,
            &crate::present::tabs(page_view.as_ref()),
            tab,
        );
    }

    /// The state, view and tab of the moment, and the page they make.
    fn current(&self) -> (ViewState, View, Tab, Option<Page>) {
        let model = self.model.borrow();
        let page_view = model
            .state
            .snapshot()
            .map(|snapshot| page(snapshot, &model.view, &model.session));
        (
            model.state.clone(),
            model.view.clone(),
            model.tab,
            page_view,
        )
    }

    /// Brings `tab` to the front.
    pub fn show_tab(self: &Rc<Self>, tab: Tab) {
        if std::mem::replace(&mut self.model.borrow_mut().tab, tab) != tab {
            self.redraw(false);
        }
    }

    /// The page for the current model, if there is a snapshot.
    pub fn current_page(&self) -> Option<Page> {
        self.current().3
    }

    pub fn window(&self) -> Option<adw::ApplicationWindow> {
        self.widgets.borrow().as_ref().map(|w| w.window.clone())
    }

    /// Does `action` to `card`, as chosen in its menu. The snapshot holds
    /// the marks and snoozes: the dashboard is asked, and the page redraws
    /// when the snapshot comes back with the change. Stages, alerts and
    /// the todos are the window's own, and redraw at once.
    pub fn act(self: &Rc<Self>, card: &CardView, action: CardAction) {
        let id = card.id.clone();
        if card.local {
            match action {
                CardAction::Stage(stage) => {
                    self.change_todos(|todos| todos.move_to(&id, stage));
                }
                CardAction::Edit => dialogs::show_todo(self, Some(&id)),
                CardAction::Delete => self.change_session(|session| session.forget_todo(&id)),
                // A todo lives in the Activity area and is the window's
                // own: its menu offers neither.
                CardAction::Pick | CardAction::Unpick | CardAction::Snooze(_) => {}
                CardAction::Alert(_) | CardAction::CancelAlert => self.act_on_alert(card, action),
            }
            return;
        }
        match action {
            CardAction::Pick => self.send(Command::SetMark { id, marked: true }),
            CardAction::Unpick => self.send(Command::SetMark { id, marked: false }),
            CardAction::Stage(stage) => self.change_view(|view| view.set_stage(&id, stage)),
            CardAction::Edit | CardAction::Delete => {}
            CardAction::Snooze(choice) => self.send(Command::Snooze {
                id,
                until: until(choice, &Local::now()).map(|at| at.with_timezone(&Utc)),
            }),
            CardAction::Alert(_) | CardAction::CancelAlert => self.act_on_alert(card, action),
        }
    }

    /// Sets or calls off the alert on `card`.
    fn act_on_alert(self: &Rc<Self>, card: &CardView, action: CardAction) {
        match action {
            CardAction::Alert(choice) => {
                let at = Local::now()
                    + chrono::TimeDelta::from_std(alert::wait(choice)).unwrap_or_default();
                self.set_alert(
                    &card.id,
                    Alert {
                        at,
                        title: card.title.clone(),
                        source: card.source.clone(),
                    },
                );
            }
            CardAction::CancelAlert => {
                self.model.borrow_mut().session.alerts.cancel(&card.id);
                self.redraw(true);
            }
            _ => {}
        }
    }

    /// Changes what lasts as long as the window (todos, alerts) and
    /// redraws when something did change.
    pub fn change_session(self: &Rc<Self>, change: impl FnOnce(&mut Session) -> bool) {
        if change(&mut self.model.borrow_mut().session) {
            self.redraw(true);
        }
    }

    /// Where Hyprland has the window and the screen in front, as far as
    /// it says.
    fn placed(
        &self,
        hyprland: &Hyprland,
    ) -> (Option<dropdown::Placement>, Option<dropdown::Screen>) {
        let window = hyprland
            .placement(std::process::id())
            .inspect_err(|error| tracing::warn!(%error, "asking Hyprland for the window"))
            .ok()
            .flatten();
        let screen = hyprland
            .screen()
            .inspect_err(|error| tracing::warn!(%error, "asking Hyprland for the screen"))
            .ok()
            .flatten();
        (window, screen)
    }

    /// What the icon on Omarchy's bar does (`tasks-pending toggle`): the
    /// window drops down over the screen, or goes away when it is shown
    /// there. Without Hyprland to place it, it comes up the usual way.
    pub fn toggle_dropdown(&self, app: &adw::Application) {
        let Some(hyprland) = &self.hyprland else {
            app.activate();
            return;
        };
        let (window, screen) = self.placed(hyprland);
        match dropdown::toggle(window.as_ref(), screen.as_ref()) {
            Toggle::Present => app.activate(),
            Toggle::Run(steps) => {
                if let Err(error) = hyprland.run(&steps) {
                    tracing::warn!(%error, "Hyprland did not drop the window down");
                    app.activate();
                }
            }
        }
    }

    /// Closing the window: the drop-down only goes away, its page kept
    /// for the next click; any other window closes, and the app with it,
    /// as it does when Hyprland will not hide the drop-down.
    fn close_or_hide(&self) -> gtk::glib::Propagation {
        let Some(hyprland) = &self.hyprland else {
            return gtk::glib::Propagation::Proceed;
        };
        let (window, screen) = self.placed(hyprland);
        match dropdown::close(window.as_ref(), screen.as_ref()) {
            Close::Quit => gtk::glib::Propagation::Proceed,
            Close::Hide(steps) => match hyprland.run(&steps) {
                Ok(()) => gtk::glib::Propagation::Stop,
                Err(error) => {
                    tracing::warn!(%error, "Hyprland did not hide the drop-down");
                    gtk::glib::Propagation::Proceed
                }
            },
        }
    }

    /// Esc: the drop-down goes away, its page kept, as closing it does.
    /// In any other window Esc is left to whatever else wants it, and
    /// never quits.
    fn escape(&self) -> gtk::glib::Propagation {
        let Some(hyprland) = &self.hyprland else {
            return gtk::glib::Propagation::Proceed;
        };
        let (window, screen) = self.placed(hyprland);
        let Some(steps) = dropdown::escape(window.as_ref(), screen.as_ref()) else {
            return gtk::glib::Propagation::Proceed;
        };
        if let Err(error) = hyprland.run(&steps) {
            tracing::warn!(%error, "Hyprland did not send the drop-down away");
        }
        gtk::glib::Propagation::Stop
    }

    /// Changes the todos and redraws.
    pub fn change_todos(self: &Rc<Self>, change: impl FnOnce(&mut Todos) -> bool) {
        self.change_session(|session| change(&mut session.todos));
    }

    /// Sets `alert` on the card `id` and shows it on the card.
    pub fn set_alert(self: &Rc<Self>, id: &str, alert: Alert) {
        self.model.borrow_mut().session.alerts.set(id, alert);
        self.redraw(true);
    }

    /// Rings the alerts whose time has come: a desktop notification each,
    /// and the cards no longer say they have one.
    fn ring_due(self: &Rc<Self>) {
        let due = self.model.borrow_mut().session.alerts.due(Local::now());
        if due.is_empty() {
            return;
        }
        for (id, alert) in &due {
            notify(self, id, alert);
        }
        let in_front = self.window().is_some_and(|window| window.is_active());
        if self.model.borrow_mut().session.missed.rang(&due, in_front) {
            self.tell_bar();
        }
        self.redraw(true);
    }

    /// The window came to the front: the alerts that rang meanwhile were
    /// seen, and the icon on Omarchy's bar loses its dot.
    pub fn seen(&self) {
        if self.model.borrow_mut().session.missed.seen() {
            self.tell_bar();
        }
    }

    /// Tells the icon on Omarchy's bar which alerts were missed.
    fn tell_bar(&self) {
        let Some(path) = &self.bar_signal else {
            return;
        };
        let model = self.model.borrow();
        if let Err(error) = omarchy_bar::tell(path, &model.session.missed.titles()) {
            tracing::warn!(%error, "telling the icon on Omarchy's bar");
        }
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

/// Sends the desktop notification for `alert`, named after the card so
/// that an alert set again replaces the earlier notification. Clicking it
/// brings the window up.
fn notify(ctx: &Ctx, id: &str, alert: &Alert) {
    let Some(app) = ctx.window().and_then(|window| window.application()) else {
        return;
    };
    let (summary, body) = notification(alert);
    let notification = gtk::gio::Notification::new(&summary);
    notification.set_body(Some(&body));
    notification.set_icon(&gtk::gio::ThemedIcon::new("alarm-symbolic"));
    notification.set_priority(gtk::gio::NotificationPriority::High);
    notification.set_default_action("app.show");
    app.send_notification(Some(&format!("alert:{id}")), &notification);
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

/// Where the window keeps what it remembers; `None` keeps it for as long
/// as the window.
pub struct Files {
    /// The view.
    pub view: Option<PathBuf>,
    /// The todos.
    pub todos: Option<PathBuf>,
}

/// Finds the dashboard, opens the window over it and runs until it closes,
/// returning the exit status. The view and the todos are kept in `files`. With `screenshot`, the
/// window shows that scene, is pictured into that file and closed. A
/// `unique` application hands over to an instance already running
/// (opening the app twice focuses the window) before looking for the
/// dashboard, so the second launch starts no sources of its own; the
/// sandbox and a screenshot always open their own window.
pub fn run(
    runtime: &Runtime,
    options: backend::Options,
    env: Env,
    files: Files,
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
    // What a notification's click does: bring the window up.
    let show = gtk::gio::SimpleAction::new("show", None);
    show.connect_activate({
        let app = app.downgrade();
        move |_, _| {
            if let Some(app) = app.upgrade() {
                app.activate();
            }
        }
    });
    app.add_action(&show);
    app.register(gtk::gio::Cancellable::NONE)
        .context("registering the application")?;
    if app.is_remote() {
        // Another instance owns the window: `run` asks it to present the
        // window and returns.
        return Ok(i32::from(app.run_with_args::<&str>(&[])));
    }
    // Ctrl+Q quits, also from the drop-down, which closing only hides.
    let quit = gtk::gio::SimpleAction::new("quit", None);
    quit.connect_activate({
        let app = app.downgrade();
        move |_, _| {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
        }
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    // The sandbox opens with something going on; a screenshot stages its
    // own scene.
    let demo = options.sandbox && screenshot.is_none();
    let omarchy_dirs = env("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| theme::omarchy_dirs(Path::new(&home)))
        .unwrap_or_default();
    let scene = screenshot.as_ref().map(|(_, scene)| *scene);
    // The sandbox and a picture neither read nor write Omarchy's settings,
    // and always show the icon's choice, off.
    let home = env("HOME")
        .filter(|home| unique && !home.is_empty())
        .map(PathBuf::from);
    let hyprland = dropdown::socket(&|name| env(name).and_then(|value| value.into_string().ok()))
        .map(Hyprland::new);
    let omarchy_shell = !unique || omarchy_bar::available(env("PATH").as_deref());
    let bar_signal = unique
        .then(|| {
            omarchy_bar::signal_path(&|name| env(name).and_then(|value| value.into_string().ok()))
        })
        .flatten();
    if unique && let Some(home) = &home {
        // The icon on Omarchy's bar keeps up with this version, once
        // Settings has put it there.
        let dir = omarchy_bar::plugin_dir(home);
        if dir.is_dir()
            && let Err(error) = omarchy_bar::write(&dir)
        {
            tracing::warn!(%error, "updating the icon on Omarchy's bar");
        }
    }
    // Read once this is the window: a second launch hands over above.
    let todos = match (&files.todos, &screenshot) {
        (Some(path), _) => Todos::open(path),
        (None, Some(_)) => Todos::picture(),
        (None, None) => Todos::in_memory(),
    };
    let view_path = files.view;
    let backend = backend::resolve(runtime, options, env)?;
    let (commands, events) = spawn_worker(runtime, backend.client.clone());
    let ctx = Rc::new(Ctx {
        model: RefCell::new(Model {
            state: ViewState::Loading,
            view: view_path.as_deref().map(View::load).unwrap_or_default(),
            tab: Tab::default(),
            session: Session {
                todos,
                ..Session::default()
            },
            view_path,
            drawn: None,
            refresh_hold: None,
            weather: None,
            update: None,
            source_line: backend.source.describe(),
            icons: icons::Cache::default(),
            looks: LookThrottle::default(),
            auto_refresh: None,
            omarchy: None,
            bar_change: None,
            bar_wanted: None,
        }),
        commands,
        runtime_handle: runtime.handle().clone(),
        widgets: RefCell::new(None),
        styling: Styling {
            provider: gtk::CssProvider::new(),
            scene,
            omarchy_dirs,
            switches: RefCell::new(None),
            settling: RefCell::new(None),
        },
        settings_page: gtk::glib::WeakRef::new(),
        hyprland,
        home,
        omarchy_shell,
        bar_signal,
        cards: RefCell::default(),
        armed: RefCell::default(),
        held: RefCell::default(),
    });
    // Nothing is missed yet, whatever an earlier run that did not end
    // cleanly left; the icon loses its dot when the app ends.
    ctx.tell_bar();
    app.connect_shutdown({
        let path = ctx.bar_signal.clone();
        move |_| {
            if let Some(path) = &path {
                let _ = std::fs::remove_file(path);
            }
        }
    });
    // What the icon on Omarchy's bar sends, through `tasks-pending toggle`.
    let toggle = gtk::gio::SimpleAction::new("toggle", None);
    toggle.connect_activate({
        let ctx = Rc::downgrade(&ctx);
        let app = app.downgrade();
        move |_, _| {
            if let (Some(ctx), Some(app)) = (ctx.upgrade(), app.upgrade()) {
                ctx.toggle_dropdown(&app);
            }
        }
    });
    app.add_action(&toggle);
    if scene == Some(Scene::Omarchy) {
        // The picture of the window following Omarchy; a screenshot saves
        // no view.
        ctx.model.borrow_mut().view.theme = Theme::Omarchy;
    }
    // The backend (and its config watch) lives as long as the window.
    let _backend = backend;

    let events = RefCell::new(Some(events));
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &ctx.styling.provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        // When the scheme turns dark or light, the window's own palette
        // follows, and so does a logo's variant on every picture showing
        // it, the open dialogs' included.
        adw::StyleManager::default().connect_dark_notify({
            let ctx = Rc::downgrade(&ctx);
            move |_| {
                if let Some(ctx) = ctx.upgrade() {
                    ctx.fill_stylesheet();
                    icons::follow_theme(&ctx);
                }
            }
        });
        ctx.restyle();
        follow_omarchy(&ctx);
        build_window(app, &ctx);
        if let Some(events) = events.borrow_mut().take() {
            pump_events(&ctx, events);
        }
        schedule_polling(&ctx);
        schedule_auto_refresh(&ctx);
        if demo {
            stage_demo(&ctx);
        }
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
                    match capture(window, &path, &ctx.palette().bg) {
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

/// How long Omarchy's folder stays quiet before the window reads the
/// theme again: a switch removes the old theme, moves the new one in and
/// writes its name, a few changes in a row.
const OMARCHY_SETTLE: Duration = Duration::from_millis(300);

/// Follows Omarchy switching themes: once the folder holding its theme
/// has settled after a change, the window, when it follows Omarchy, is
/// coloured again, and an open Settings says which theme it follows. A
/// screenshot follows nothing, and without Omarchy there is nothing to
/// watch.
fn follow_omarchy(ctx: &Rc<Ctx>) {
    if ctx.styling.scene.is_some() {
        return;
    }
    let Some(dir) = theme::omarchy_watch_dir(&ctx.styling.omarchy_dirs) else {
        return;
    };
    let monitor = match gtk::gio::File::for_path(&dir).monitor_directory(
        gtk::gio::FileMonitorFlags::NONE,
        gtk::gio::Cancellable::NONE,
    ) {
        Ok(monitor) => monitor,
        Err(error) => {
            tracing::warn!(path = %dir.display(), %error, "Omarchy's theme switches not followed");
            return;
        }
    };
    monitor.connect_changed({
        let ctx = Rc::downgrade(ctx);
        move |_, _, _, _| {
            let Some(ctx) = ctx.upgrade() else {
                return;
            };
            if let Some(waiting) = ctx.styling.settling.take() {
                waiting.remove();
            }
            let settled = gtk::glib::timeout_add_local_once(OMARCHY_SETTLE, {
                let ctx = Rc::downgrade(&ctx);
                move || {
                    let Some(ctx) = ctx.upgrade() else {
                        return;
                    };
                    ctx.styling.settling.take();
                    let follows = ctx.model.borrow().view.theme == Theme::Omarchy;
                    if follows {
                        ctx.restyle();
                        dialogs::refresh_settings(&ctx);
                    }
                }
            });
            ctx.styling.settling.replace(Some(settled));
        }
    });
    ctx.styling.switches.replace(Some(monitor));
}

fn build_window(app: &adw::Application, ctx: &Rc<Ctx>) {
    let hero = hero::build(ctx);
    let tabs = tabs::build(ctx);
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
    content.append(&tabs.root);
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
    window.connect_close_request({
        let ctx = Rc::downgrade(ctx);
        move |_| {
            ctx.upgrade()
                .map_or(gtk::glib::Propagation::Proceed, |ctx| ctx.close_or_hide())
        }
    });
    // Esc sends the drop-down away. It is looked at last, on the way back
    // up from the focused widget: an open dialog or menu takes its own Esc
    // first, and closes.
    let keys = gtk::ShortcutController::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Bubble);
    keys.add_shortcut(gtk::Shortcut::new(
        Some(gtk::KeyvalTrigger::new(
            gtk::gdk::Key::Escape,
            gtk::gdk::ModifierType::empty(),
        )),
        Some(gtk::CallbackAction::new({
            let ctx = Rc::downgrade(ctx);
            move |_, _| {
                ctx.upgrade()
                    .map_or(gtk::glib::Propagation::Proceed, |ctx| ctx.escape())
            }
        })),
    ));
    window.add_controller(keys);
    // The other keys, and the dialog that lists them.
    self::keys::listen(ctx, &window);
    watch_activity(ctx, &window);
    window.connect_is_active_notify({
        let ctx = Rc::downgrade(ctx);
        move |window| {
            if window.is_active()
                && let Some(ctx) = ctx.upgrade()
            {
                ctx.seen();
            }
        }
    });
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
        tabs,
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
/// the pointer merely crossing it does not. Clicks and keys count once
/// released: the snapshot read after the look may rebuild the page, which
/// before the release would take the pressed button away, and the click
/// with it (a button activated from the keyboard clicks on the key's
/// release). A raw controller sees the release whatever gesture below
/// claims the click.
fn watch_activity(ctx: &Rc<Ctx>, window: &adw::ApplicationWindow) {
    let release = gtk::EventControllerLegacy::new();
    release.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ctx = ctx.clone();
        release.connect_event(move |_, event| {
            if matches!(
                event.event_type(),
                gtk::gdk::EventType::ButtonRelease | gtk::gdk::EventType::TouchEnd
            ) {
                ctx.looked();
            }
            gtk::glib::Propagation::Proceed
        });
    }
    window.add_controller(release);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let ctx = ctx.clone();
        keys.connect_key_released(move |_, _, _, _| ctx.looked());
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
                        // A card unpicked, or finished at its source, loses
                        // its stage: picked again, it starts over.
                        let picked = model.state.snapshot().map(|s| s.marked.clone());
                        if let Some(picked) = picked
                            && model.view.keep_stages_of(&picked)
                            && let Some(path) = model.view_path.clone()
                        {
                            model.view.save(&path);
                        }
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
                    // A refused change brings no new snapshot of its own:
                    // rebuild the page as it was, so the button pressed is
                    // usable again.
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
/// window is shown; the weather every [`WEATHER_INTERVAL`]. The alerts
/// are looked at every [`ALERT_TICK`], shown or not.
fn schedule_polling(ctx: &Rc<Ctx>) {
    ctx.send(Command::Poll);
    repeat(ctx, POLL_INTERVAL, Timer::Poll);
    repeat(ctx, WEATHER_INTERVAL, Timer::Weather);
    repeat(ctx, UPDATE_INTERVAL, Timer::Version);
    let alerts = ctx.clone();
    gtk::glib::timeout_add_local(ALERT_TICK, move || {
        alerts.ring_due();
        gtk::glib::ControlFlow::Continue
    });
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

/// The first card of every stack on the page, in page order.
fn first_cards(ctx: &Ctx) -> Vec<pending_core::PendingCard> {
    ctx.model
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
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Sends `cards` to the Activity, one at each stage in turn, with an
/// alert in an hour on the one under way when asked.
fn pick_along_the_stages(ctx: &Rc<Ctx>, cards: &[pending_core::PendingCard], with_alert: bool) {
    let stages = [Stage::Picked, Stage::InProgress, Stage::Done];
    for (card, stage) in cards.iter().zip(stages) {
        ctx.send(Command::SetMark {
            id: card.id.clone(),
            marked: true,
        });
        ctx.change_view(|view| view.set_stage(&card.id, stage));
        if with_alert && stage == Stage::InProgress {
            ctx.set_alert(
                &card.id,
                Alert {
                    at: Local::now() + chrono::TimeDelta::hours(1),
                    title: card.title.clone(),
                    source: card.source.clone(),
                },
            );
        }
    }
}

/// The sandbox opens with something going on, once its first snapshot is
/// in: three cards along the stages of the Activity and two todos, so the
/// main screen is not empty before anything is picked. None of it is
/// saved, as nothing of the sandbox is.
fn stage_demo(ctx: &Rc<Ctx>) {
    let ctx = ctx.clone();
    gtk::glib::timeout_add_local_once(SCENE_AFTER, move || {
        pick_along_the_stages(&ctx, &first_cards(&ctx), false);
        ctx.change_todos(|todos| {
            todos
                .add(
                    "Write the release notes",
                    "Changelog, then the README picture",
                )
                .is_some()
                | todos
                    .add(
                        "Try an alert",
                        "15 min on a card in progress: the desktop notification names it",
                    )
                    .is_some()
        });
    });
}

/// Sets a screenshot's scene up once the first snapshot is in: a todo in
/// Picked, three cards picked and moved along the stages for the Activity
/// (and under the todo dialog), two picked for the Board, one snoozed for
/// the snoozed list, and the dialog opened a moment later.
fn stage_scene(ctx: &Rc<Ctx>, scene: crate::Scene) {
    use crate::Scene;
    let ctx = ctx.clone();
    gtk::glib::timeout_add_local_once(SCENE_AFTER, move || {
        let firsts = first_cards(&ctx);
        let ids: Vec<String> = firsts.iter().map(|card| card.id.clone()).collect();
        // A todo written down in the window, in Picked.
        ctx.change_todos(|todos| {
            todos
                .add(
                    "Write the release notes",
                    "Changelog, then the README picture",
                )
                .is_some()
        });
        match scene {
            Scene::Activity | Scene::Light | Scene::Omarchy | Scene::Todo | Scene::Keys => {
                pick_along_the_stages(&ctx, &firsts, true)
            }
            Scene::Board | Scene::Settings => {
                ctx.show_tab(Tab::Board);
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
            Scene::Settings => dialogs::show_settings(&ctx),
            Scene::Snoozed => dialogs::show_snoozed(&ctx),
            Scene::Todo => dialogs::show_todo(&ctx, None),
            Scene::Keys => keys::show(&ctx),
            Scene::Activity | Scene::Board | Scene::Light | Scene::Omarchy => {}
        });
    });
}

/// Renders the window as it is drawn into a PNG file: its background,
/// then everything in it (content and dialogs), snapshotted through the
/// window the way GTK draws it (a toplevel itself paints nothing into a
/// snapshot, and a widget paintable only has a picture a frame after it
/// is made).
fn capture(window: &adw::ApplicationWindow, path: &Path, background: &str) -> anyhow::Result<()> {
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
    let background = gtk::gdk::RGBA::parse(background)
        .map_err(|_| anyhow::anyhow!("bad background colour {background}"))?;
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
