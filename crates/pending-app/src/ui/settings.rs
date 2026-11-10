//! Settings: the sources, written to the config file with their stacks
//! and to the env file with their credentials; the board shown, how often
//! the sources refresh, the theme and the icon on Omarchy's bar. A page
//! for each source and each stack opens over the first.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use pending_core::config::AppConfig;
use pending_core::edit::{ConfigDoc, EditError};
use pending_core::schema;
use pending_core::schema::Var;
use toml::Value;

use super::board::{clear, notice, tracked};
use super::dialogs::{DIALOG_WIDTH, flow_child, pill, show_over};
use super::icons::{size, source_icon, symbolic};
use super::{Ctx, sources, wrapped as text};
use crate::backend::Command;
use crate::omarchy_bar;
use crate::present::{
    BarState, Choice, Page, SourceView, bar_choices, bar_note, bar_view, board_choices,
    theme_choices, theme_note,
};
use crate::settings::{
    ConfigFile, EnvEdit, Place, REFRESH_CHOICES, Refusal, SANDBOX_CONFIG, SANDBOX_ENV,
    TIMEOUT_CHOICES, by_board, duration_label, durations,
};
use crate::view::View;

/// The dialog's height: fixed, so that it stays in place from one page to
/// the next; smaller windows shrink it.
const DIALOG_HEIGHT: i32 = 760;

/// What Settings says when the dashboard did not say which files it
/// follows.
const FILES_UNKNOWN: &str = "The dashboard has not said which config file it follows, so its \
                             sources cannot be changed here. A daemon older than 0.10.0 does not \
                             say: restart it after updating.";

/// Settings while it is open: the files it changes and its pages.
pub struct Open {
    pub ctx: Rc<Ctx>,
    /// The config file; `None` while the dashboard has not said which.
    pub config: RefCell<Option<ConfigFile>>,
    /// The env file; `None` while the dashboard has not said which.
    pub env: RefCell<Option<EnvEdit>>,
    pub nav: adw::NavigationView,
    main: gtk::Box,
    /// The page of the source over the main one, while there is one.
    pub source: RefCell<Option<sources::SourcePage>>,
}

impl Open {
    /// Fills the main page and the source's page again from the files.
    /// A stack's page keeps what was typed into it.
    pub fn fill(self: &Rc<Self>) {
        self.fill_main();
        let page = self.source.borrow().clone();
        if let Some(page) = page {
            sources::fill(self, &page);
        }
    }

    /// What the config file says, or why it says nothing Settings can
    /// show; `None` while the dashboard has not said which file it is.
    pub fn config(&self) -> Option<Result<AppConfig, String>> {
        self.config.borrow().as_ref().map(ConfigFile::config)
    }

    /// Makes `change` to the config file, saying on `at` (the page it was
    /// made on) why it was not made. The caller fills the pages again
    /// once it has done its part; see [`Open::changed`].
    pub fn change<T>(
        self: &Rc<Self>,
        at: &gtk::Box,
        change: impl FnOnce(&mut ConfigDoc) -> Result<T, EditError>,
    ) -> Option<T> {
        let outcome = match self.config.borrow_mut().as_mut() {
            Some(file) => file.change(change),
            None => Err(Refusal::Failed(FILES_UNKNOWN.to_owned())),
        };
        self.outcome(at, outcome, true)
    }

    /// Makes `change` to the config file and fills the pages again.
    pub fn changed<T>(
        self: &Rc<Self>,
        at: &gtk::Box,
        change: impl FnOnce(&mut ConfigDoc) -> Result<T, EditError>,
    ) -> Option<T> {
        let made = self.change(at, change)?;
        self.fill();
        Some(made)
    }

    /// Sets `var` in the env file (or removes it), saying on `at` why not.
    pub fn set_var(self: &Rc<Self>, at: &gtk::Box, var: &Var, value: Option<&str>) -> bool {
        let outcome = match self.env.borrow_mut().as_mut() {
            Some(env) => env.set(var, value),
            None => Err(Refusal::Failed(FILES_UNKNOWN.to_owned())),
        };
        self.outcome(at, outcome, false).is_some()
    }

    /// What a change came to. A change to the config refused because the
    /// file changed outside Settings goes back to the main page, which
    /// shows the file as it is now: the pages over it may name sources
    /// and stacks by places that moved. Any other refusal is said on the
    /// page the change was made on.
    fn outcome<T>(
        self: &Rc<Self>,
        at: &gtk::Box,
        outcome: Result<T, Refusal>,
        of_config: bool,
    ) -> Option<T> {
        let refusal = match outcome {
            Ok(made) => return Some(made),
            Err(refusal) => refusal,
        };
        if of_config && refusal == Refusal::Changed {
            self.nav.pop_to_tag("main");
            self.source.take();
            self.fill();
            say(&self.main, &refusal.message());
        } else {
            self.fill();
            say(at, &refusal.message());
        }
        None
    }

    /// Settings' files once the dashboard says which they are.
    fn follow(self: &Rc<Self>, places: (Option<Place>, Option<Place>)) {
        let same = |shown: Option<&Place>, now: Option<&Place>| {
            shown.and_then(Place::path) == now.and_then(Place::path)
                && shown.is_some() == now.is_some()
        };
        let (config, env) = places;
        let config_same = same(
            self.config.borrow().as_ref().map(ConfigFile::place),
            config.as_ref(),
        );
        let env_same = same(self.env.borrow().as_ref().map(EnvEdit::place), env.as_ref());
        if config_same && env_same {
            return;
        }
        self.config.replace(config.map(ConfigFile::open));
        self.env.replace(env.map(EnvEdit::open));
        self.nav.pop_to_tag("main");
        self.source.take();
        self.fill();
    }

    fn fill_main(self: &Rc<Self>) {
        let ctx = &self.ctx;
        let content = &self.main;
        clear(content);
        let page = ctx.current_page();
        if let Some(error) = page.as_ref().and_then(|page| page.config_error.as_ref()) {
            content.append(&notice(
                "stale",
                "Config not reloaded (the previous one is still running)",
                error,
            ));
        }
        content.append(&heading("Sources"));
        let config = self.config();
        match &config {
            None => content.append(&text(FILES_UNKNOWN, &["dialog-note"])),
            Some(Err(why)) => {
                content.append(&notice(
                    "stale",
                    "The config file does not load, so Settings cannot change it",
                    why,
                ));
                self.append_place();
            }
            Some(Ok(config)) => {
                self.append_sources(config, page.as_ref());
                self.append_place();
            }
        }

        let view = ctx.model.borrow().view.clone();
        let snapshot = ctx.model.borrow().state.snapshot().cloned();
        if let Some(snapshot) = &snapshot {
            content.append(&heading("Board"));
            content.append(
                &self.view_pills(board_choices(snapshot, &view), |view, board| {
                    view.board = board;
                }),
            );
        }
        if let Some(Ok(config)) = &config {
            content.append(&heading("Refresh every"));
            content.append(&self.global_pills(
                "refresh_seconds",
                &durations(&REFRESH_CHOICES, config.refresh_seconds),
                config.refresh_seconds,
            ));
            content.append(&text(
                "How often each source is asked for its cards; a source can have its own.",
                &["dialog-note"],
            ));
            content.append(&heading("Give up on a refresh after"));
            content.append(&self.global_pills(
                "timeout_seconds",
                &durations(&TIMEOUT_CHOICES, config.timeout_seconds),
                config.timeout_seconds,
            ));
        }
        content.append(&heading("Theme"));
        content.append(&self.view_pills(theme_choices(&view), |view, theme| {
            view.theme = theme;
        }));
        if let Some(note) = theme_note(&view, ctx.model.borrow().omarchy.as_ref()) {
            content.append(&text(&note, &["dialog-note"]));
        }
        if let Some((on, state)) = bar_state(ctx) {
            content.append(&heading("Omarchy bar"));
            content.append(&self.bar_pills(on));
            content.append(&text(&bar_note(&state), &["dialog-note"]));
        }
    }

    /// The sources by board: each board with a switch for all its
    /// sources, each source with its own and the way to its page.
    fn append_sources(self: &Rc<Self>, config: &AppConfig, page: Option<&Page>) {
        let content = &self.main;
        if config.sources.is_empty() {
            content.append(&text(
                "No sources yet: the board shows sample cards until there is one.",
                &["dialog-note"],
            ));
        }
        for board in by_board(config) {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            line.add_css_class("board-line");
            let name = text(&board.board, &["board-name"]);
            name.set_hexpand(true);
            line.append(&name);
            let all = switch(board.on, &format!("Every source of {}", board.board));
            {
                let open = Rc::downgrade(self);
                let indices = board.sources.clone();
                all.connect_state_set(move |_, on| {
                    if let Some(open) = open.upgrade() {
                        open.changed(&open.main.clone(), |doc| {
                            for &index in &indices {
                                doc.set_source(index, "enabled", enabled(on))?;
                            }
                            Ok(())
                        });
                    }
                    gtk::glib::Propagation::Proceed
                });
            }
            line.append(&all);
            content.append(&line);
            for index in board.sources {
                let source = &config.sources[index];
                let live =
                    page.and_then(|page| page.sources.iter().find(|s| s.name == source.name));
                content.append(&self.source_row(index, source, live));
            }
        }
        let add = gtk::Button::with_label("Add a source…");
        add.add_css_class("control");
        add.add_css_class("wake");
        add.set_halign(gtk::Align::Start);
        add.set_margin_top(4);
        {
            let open = Rc::downgrade(self);
            add.connect_clicked(move |_| {
                if let Some(open) = open.upgrade() {
                    sources::push_kinds(&open);
                }
            });
        }
        content.append(&add);
    }

    /// A source: its logo, name and kind, how its last refresh went, its
    /// switch, and the way to its page.
    fn source_row(
        self: &Rc<Self>,
        index: usize,
        source: &pending_core::config::SourceConfig,
        live: Option<&SourceView>,
    ) -> gtk::Box {
        let ctx = &self.ctx;
        let kind = schema::kind(source.kind);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("row-card");
        row.add_css_class("source-row");
        if !source.enabled {
            row.add_css_class("off");
        }
        row.append(&source_icon(ctx, source.icon().as_ref(), size::LOGO));
        let texts = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = text(&source.name, &["card-title"]);
        top.append(&name);
        let badge = gtk::Label::new(Some(kind.label));
        badge.add_css_class("badge");
        top.append(&badge);
        texts.append(&top);
        match (source.enabled, live) {
            (false, _) => texts.append(&text("Off: not asked for, and not shown.", &["row-time"])),
            (true, Some(live)) => {
                texts.append(&text(&live.time, &["row-time"]));
                if let Some(message) = &live.message {
                    texts.append(&text(message, &["row-body"]));
                }
            }
            (true, None) => texts.append(&text("Not refreshed yet.", &["row-time"])),
        }
        row.append(&texts);
        if let Some(live) = live.filter(|_| source.enabled) {
            row.append(&status(live));
        }
        let on = switch(
            source.enabled,
            &format!("Ask {} for its cards", source.name),
        );
        {
            let open = Rc::downgrade(self);
            on.connect_state_set(move |_, on| {
                if let Some(open) = open.upgrade() {
                    open.changed(&open.main.clone(), |doc| {
                        doc.set_source(index, "enabled", enabled(on))
                    });
                }
                gtk::glib::Propagation::Proceed
            });
        }
        row.append(&on);
        let edit = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text(format!("Change {}", source.name))
            .build();
        edit.add_css_class("control");
        edit.add_css_class("wake");
        {
            let open = Rc::downgrade(self);
            edit.connect_clicked(move |_| {
                if let Some(open) = open.upgrade() {
                    sources::push(&open, index);
                }
            });
        }
        row.append(&edit);
        row
    }

    /// Where the config is written.
    fn append_place(&self) {
        let place = self
            .config
            .borrow()
            .as_ref()
            .map(|file| file.place().clone());
        let line = match place.as_ref().and_then(Place::path) {
            Some(path) => format!(
                "Written to {}, which the dashboard reads again within seconds.",
                path.display()
            ),
            None => "The sandbox keeps its config in memory: nothing is written, and its \
                     board stays as it is."
                .to_owned(),
        };
        self.main.append(&text(&line, &["dialog-note"]));
    }

    /// A pill for each of `choices` of the view; choosing one applies it.
    fn view_pills<T: Clone + 'static>(
        self: &Rc<Self>,
        choices: Vec<Choice<T>>,
        set: fn(&mut View, T),
    ) -> gtk::FlowBox {
        let open = Rc::downgrade(self);
        pills(choices, move |value| {
            if let Some(open) = open.upgrade() {
                open.ctx.change_view(|view| set(view, value));
                open.fill_main();
            }
        })
    }

    /// A pill for each of `choices`, in seconds, of the file's `key`.
    fn global_pills(
        self: &Rc<Self>,
        key: &'static str,
        choices: &[u64],
        current: u64,
    ) -> gtk::FlowBox {
        let choices = choices
            .iter()
            .map(|&secs| Choice {
                label: duration_label(secs),
                value: secs,
                active: secs == current,
            })
            .collect();
        let open = Rc::downgrade(self);
        pills(choices, move |secs| {
            if let Some(open) = open.upgrade() {
                let value = Value::Integer(i64::try_from(secs).unwrap_or(i64::MAX));
                open.changed(&open.main.clone(), |doc| {
                    doc.set_global(key, Some(value));
                    Ok(())
                });
            }
        })
    }

    /// Off and On for the icon on Omarchy's bar.
    fn bar_pills(self: &Rc<Self>, on: bool) -> gtk::FlowBox {
        let ctx = &self.ctx;
        let changing = matches!(ctx.model.borrow().bar_change, Some(BarState::Changing(_)));
        let open = Rc::downgrade(self);
        let row = pills(bar_choices(on), move |wanted| {
            if let Some(open) = open.upgrade() {
                if wanted == on {
                    // Already so: the pill stays as it was.
                    open.fill_main();
                } else {
                    set_bar(&open, wanted);
                }
            }
        });
        row.set_sensitive(!changing);
        row
    }
}

/// A pill for each of `choices`; choosing one calls `choose` with it.
pub(super) fn pills<T: Clone + 'static>(
    choices: Vec<Choice<T>>,
    choose: impl Fn(T) + 'static,
) -> gtk::FlowBox {
    let row = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(10)
        .build();
    let choose = Rc::new(choose);
    for choice in choices {
        let button = pill(&choice.label, choice.active);
        let choose = choose.clone();
        let value = choice.value;
        button.connect_clicked(move |_| choose(value.clone()));
        row.insert(&flow_child(&button), -1);
    }
    row
}

/// Opens Settings.
pub fn show(ctx: &Rc<Ctx>) {
    if ctx.model.borrow().files.is_none() && ctx.sandbox_files.is_none() {
        ctx.send(Command::Files);
    }
    let (config, env) = places(ctx);
    let main = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let nav = adw::NavigationView::new();
    nav.add(&page("Settings", "main", &main));
    let open = Rc::new(Open {
        ctx: ctx.clone(),
        config: RefCell::new(config.map(ConfigFile::open)),
        env: RefCell::new(env.map(EnvEdit::open)),
        nav: nav.clone(),
        main,
        source: RefCell::new(None),
    });
    {
        let open = Rc::downgrade(&open);
        nav.connect_popped(move |_, page| {
            if page.tag().as_deref() == Some("source")
                && let Some(open) = open.upgrade()
            {
                open.source.take();
            }
        });
    }
    let dialog = adw::Dialog::builder()
        .title("Settings")
        .content_width(DIALOG_WIDTH)
        .content_height(DIALOG_HEIGHT)
        .child(&nav)
        .build();
    dialog.add_css_class("tasks-pending");
    {
        let ctx = Rc::downgrade(ctx);
        dialog.connect_closed(move |_| {
            if let Some(ctx) = ctx.upgrade() {
                ctx.settings.take();
            }
        });
    }
    ctx.settings.replace(Some(open.clone()));
    open.fill();
    show_over(ctx, &dialog);
}

/// Opens Settings at the page of the source named `source`, or of its
/// stack at `stack`; for a screenshot.
pub fn show_at(ctx: &Rc<Ctx>, source: &str, stack: Option<usize>) {
    show(ctx);
    let open = ctx.settings.borrow().clone();
    let Some(open) = open else {
        return;
    };
    let Some(Ok(config)) = open.config() else {
        return;
    };
    let Some(index) = config.sources.iter().position(|s| s.name == source) else {
        return;
    };
    sources::push(&open, index);
    if let Some(stack) = stack {
        sources::push_stack(&open, index, Some(stack));
    }
}

/// Fills the open Settings again, for a change made outside it (Omarchy
/// switching themes, the dashboard naming its files).
pub fn refresh(ctx: &Rc<Ctx>) {
    let open = ctx.settings.borrow().clone();
    if let Some(open) = open {
        open.follow(places(ctx));
        open.fill_main();
    }
}

/// The files Settings changes: the ones the dashboard follows, or the
/// sandbox's in memory.
fn places(ctx: &Ctx) -> (Option<Place>, Option<Place>) {
    if let Some((config, env)) = &ctx.sandbox_files {
        return (Some(config.clone()), Some(env.clone()));
    }
    let files = ctx.model.borrow().files.clone().unwrap_or_default();
    (files.config.map(Place::File), files.env.map(Place::File))
}

/// The sandbox's files, in memory for as long as the window.
pub fn sandbox_files() -> (Place, Place) {
    (
        Place::memory(Some(SANDBOX_CONFIG)),
        Place::memory(Some(SANDBOX_ENV)),
    )
}

/// A page of Settings titled `title`, holding `content`.
pub(super) fn page(title: &str, tag: &str, content: &gtk::Box) -> adw::NavigationPage {
    content.add_css_class("dialog-page");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(content)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    adw::NavigationPage::builder()
        .title(title)
        .tag(tag)
        .child(&view)
        .build()
}

pub(super) fn heading(label: &str) -> gtk::Label {
    tracked(label, &["dialog-heading"])
}

/// Says `message` at the top of `page`, in place of what was said there.
pub(super) fn say(page: &gtk::Box, message: &str) {
    if let Some(first) = page.first_child()
        && first.has_css_class("said")
    {
        page.remove(&first);
    }
    let said = notice("stale", message, "");
    said.add_css_class("said");
    page.prepend(&said);
}

/// A switch, centred in its row.
pub(super) fn switch(on: bool, tooltip: &str) -> gtk::Switch {
    gtk::Switch::builder()
        .active(on)
        .valign(gtk::Align::Center)
        .tooltip_text(tooltip)
        .build()
}

/// `enabled` as written: left out when on, its default.
pub(super) fn enabled(on: bool) -> Option<Value> {
    (!on).then_some(Value::Boolean(false))
}

/// How a source's last refresh went, as a pill.
fn status(live: &SourceView) -> gtk::Box {
    let status = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    status.add_css_class("status");
    status.add_css_class(live.status_class);
    status.append(&symbolic(live.status_icon, size::TEXT));
    status.append(&gtk::Label::new(Some(live.status_text)));
    status
}

/// Whether the icon is on Omarchy's bar, and what to say about it; `None`
/// without Omarchy's shell. Omarchy's settings say where it is
/// ([`bar_view`]); the sandbox reads none, and the icon is off there.
fn bar_state(ctx: &Rc<Ctx>) -> Option<(bool, BarState)> {
    if !ctx.omarchy_shell {
        return None;
    }
    let placed = ctx.home.as_ref().is_some_and(|home| {
        std::fs::read_to_string(omarchy_bar::shell_config(home))
            .is_ok_and(|settings| omarchy_bar::in_bar(&settings))
    });
    let mut model = ctx.model.borrow_mut();
    let asked = model.bar_wanted.map(|(on, at)| (on, at.elapsed()));
    let view = bar_view(placed, asked, model.bar_change.as_ref());
    if !view.keep_asked {
        model.bar_wanted = None;
    }
    Some((view.on, view.state))
}

/// Puts the icon on Omarchy's bar (writing the plugin first) or takes it
/// off, with Omarchy's own commands, off the main loop; Settings says it
/// is under way, then how it went.
fn set_bar(open: &Rc<Open>, on: bool) {
    let ctx = &open.ctx;
    let Some(home) = ctx.home.clone() else {
        ctx.model.borrow_mut().bar_change = Some(BarState::Failed(
            "the sandbox leaves Omarchy's bar alone".to_owned(),
        ));
        open.fill_main();
        return;
    };
    if on && let Err(error) = omarchy_bar::write(&omarchy_bar::plugin_dir(&home)) {
        ctx.model.borrow_mut().bar_change = Some(BarState::Failed(format!(
            "could not write the plugin ({error})"
        )));
        open.fill_main();
        return;
    }
    ctx.model.borrow_mut().bar_change = Some(BarState::Changing(on));
    open.fill_main();
    let ctx = ctx.clone();
    gtk::glib::spawn_future_local(async move {
        let outcome = run_all(omarchy_bar::steps(on)).await;
        {
            let mut model = ctx.model.borrow_mut();
            match outcome {
                Ok(()) => {
                    model.bar_change = None;
                    model.bar_wanted = Some((on, std::time::Instant::now()));
                }
                Err(why) => model.bar_change = Some(BarState::Failed(why)),
            }
        }
        refresh(&ctx);
    });
}

/// How long one of Omarchy's commands may take before Settings gives up
/// on it (its shell gives up on itself after three seconds).
const OMARCHY_ANSWERS_WITHIN: std::time::Duration = std::time::Duration::from_secs(15);

/// Runs the steps in turn until a command fails, saying why: the last
/// line it wrote to its error output, or that it could not run or did
/// not answer in time.
async fn run_all(steps: Vec<omarchy_bar::Step>) -> Result<(), String> {
    for step in steps {
        match step {
            omarchy_bar::Step::Run(argv) => {
                let (ok, _, errors) = run(&argv).await?;
                if !ok {
                    let line = argv.join(" ");
                    return Err(omarchy_bar::refusal(&errors)
                        .map_or_else(|| format!("{line} failed"), str::to_owned));
                }
            }
            omarchy_bar::Step::AwaitListed => {
                // As Omarchy's `plugin add` does: 40 looks, 50 ms apart.
                // Enabling then says plainly if it is still not found.
                for _ in 0..40 {
                    if let Ok((true, list, _)) = run(&omarchy_bar::LIST).await
                        && omarchy_bar::listed(&list)
                    {
                        break;
                    }
                    gtk::glib::timeout_future(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }
    Ok(())
}

/// Runs one command off the main loop: whether it succeeded, and what it
/// wrote to its output and its error output.
async fn run(argv: &[&str]) -> Result<(bool, String, String), String> {
    let line = argv.join(" ");
    let args: Vec<&std::ffi::OsStr> = argv.iter().map(std::ffi::OsStr::new).collect();
    let process = gtk::gio::Subprocess::newv(
        &args,
        gtk::gio::SubprocessFlags::STDOUT_PIPE | gtk::gio::SubprocessFlags::STDERR_PIPE,
    )
    .map_err(|error| format!("{line} did not run ({error})"))?;
    let answered = gtk::glib::future_with_timeout(
        OMARCHY_ANSWERS_WITHIN,
        process.communicate_utf8_future(None),
    )
    .await;
    let Ok(outcome) = answered else {
        process.force_exit();
        return Err(format!("{line} did not answer"));
    };
    let (output, errors) = outcome.map_err(|error| format!("{line} did not finish ({error})"))?;
    Ok((
        process.is_successful(),
        output.map(String::from).unwrap_or_default(),
        errors.map(String::from).unwrap_or_default(),
    ))
}
