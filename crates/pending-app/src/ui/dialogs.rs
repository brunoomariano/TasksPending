//! The dialogs: settings (the sources first), the snoozed cards, and a
//! todo.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use super::board::{self, clear, notice, tracked};
use super::icons::{size, source_icon, symbolic};
use super::{Ctx, wrapped as text};
use crate::backend::Command;
use crate::omarchy_bar;
use crate::present::{
    BarState, BoardSwitches, Choice, Page, SwitchView, bar_choices, bar_note, bar_view,
    board_choices, interval_choices, switches, theme_choices, theme_note, todo_dialog_words,
    todo_note,
};
use crate::view::{View, flip};

/// Every dialog's width: enough for a source's message or a stack's name
/// on one line, and the same for all three so they open in one place.
const DIALOG_WIDTH: i32 = 600;
/// How far each level of the show/hide tree is set in from the one above.
const INDENT: i32 = 18;

/// A dialog titled `title` over the window, holding `content`.
fn present(ctx: &Rc<Ctx>, title: &str, content: &gtk::Box) {
    show_over(ctx, &dialog(title, content));
}

/// Shows `dialog` over the window. The card that had the keyboard focus
/// has it back once the dialog closes, also when a redraw while it was
/// open (a todo saved, a refresh) made that card anew.
pub(super) fn show_over(ctx: &Rc<Ctx>, dialog: &adw::Dialog) {
    board::keep_focus_through(ctx, dialog);
    dialog.present(ctx.window().as_ref());
}

/// A dialog titled `title` holding `content`, not shown yet.
pub(super) fn dialog(title: &str, content: &gtk::Box) -> adw::Dialog {
    content.add_css_class("dialog-page");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(content)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    // A fixed width and the content's natural height, up to the window: a
    // short list is a short dialog, and the settings scroll. (Following
    // the content size would shrink wrapped labels to a word a line.)
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(DIALOG_WIDTH)
        .child(&view)
        .build();
    dialog.add_css_class("tasks-pending");
    dialog
}

/// Every source with its state and last fetch, and the config error when
/// the file on disk was rejected: the top of Settings.
fn append_sources(ctx: &Rc<Ctx>, page: &Page, content: &gtk::Box) {
    if let Some(error) = &page.config_error {
        content.append(&notice(
            "stale",
            "Config not reloaded (the previous one is still running)",
            error,
        ));
    }
    for source in &page.sources {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.add_css_class("row-card");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.append(&source_icon(ctx, source.icon.as_ref(), size::LOGO));
        let name = text(&source.name, &[]);
        name.add_css_class("card-title");
        top.append(&name);
        let boards = gtk::Label::new(Some(&source.boards));
        boards.add_css_class("badge");
        top.append(&boards);
        let status = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::End)
            .hexpand(true)
            .build();
        status.add_css_class("status");
        status.add_css_class(source.status_class);
        status.append(&symbolic(source.status_icon, size::TEXT));
        status.append(&gtk::Label::new(Some(source.status_text)));
        top.append(&status);
        row.append(&top);
        row.append(&text(&source.time, &["row-time"]));
        if let Some(message) = &source.message {
            row.append(&text(message, &["row-body"]));
        }
        content.append(&row);
    }
}

/// The snoozed cards, each with when it comes back and a button to wake it.
pub fn show_snoozed(ctx: &Rc<Ctx>) {
    if let Some(dialog) = snoozed_dialog(ctx) {
        show_over(ctx, &dialog);
    }
}

fn snoozed_dialog(ctx: &Rc<Ctx>) -> Option<adw::Dialog> {
    let page = ctx.current_page()?;
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    if page.snoozed.is_empty() {
        content.append(&text("Nothing snoozed.", &["empty"]));
    }
    // Filled in once the dialog exists; weak, because its Wake buttons
    // live inside it.
    let dialog = Rc::new(gtk::glib::WeakRef::<adw::Dialog>::new());
    for entry in &page.snoozed {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("row-card");
        let texts = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .build();
        let title = text(&entry.card.title, &["card-title"]);
        texts.append(&title);
        let body = text(&entry.card.body, &["row-body"]);
        body.set_ellipsize(gtk::pango::EllipsizeMode::End);
        body.set_lines(1);
        texts.append(&body);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.add_css_class("card-source");
        footer.append(&source_icon(ctx, entry.icon.as_ref(), size::SMALL));
        footer.append(&text(&entry.footer(), &[]));
        texts.append(&footer);
        row.append(&texts);
        let wake = gtk::Button::builder()
            .label("Wake")
            .valign(gtk::Align::Center)
            .build();
        wake.add_css_class("control");
        wake.add_css_class("wake");
        {
            let ctx = ctx.clone();
            let id = entry.card.id.clone();
            let dialog = dialog.clone();
            wake.connect_clicked(move |button| {
                button.set_sensitive(false);
                ctx.send(Command::Wake { id: id.clone() });
                // The card is back on the board; the list is stale.
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            });
        }
        row.append(&wake);
        content.append(&row);
    }
    let built = self::dialog("Snoozed", &content);
    dialog.set(Some(&built));
    Some(built)
}

/// Writing down a todo, or rewriting the one with id `editing`: a title,
/// optional details, and a note on whether todos are kept. Enter in a
/// field is the button.
pub fn show_todo(ctx: &Rc<Ctx>, editing: Option<&str>) {
    let existing = editing.and_then(|id| ctx.model.borrow().session.todos.get(id).cloned());
    if editing.is_some() && existing.is_none() {
        return;
    }
    let (title, button) = todo_dialog_words(existing.is_some());
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&heading("Todo"));
    let what = gtk::Entry::builder()
        .placeholder_text("What needs doing")
        .text(existing.as_ref().map_or("", |todo| todo.title.as_str()))
        .activates_default(true)
        .build();
    content.append(&what);
    content.append(&heading("Details"));
    let details = gtk::Entry::builder()
        .placeholder_text("Optional")
        .text(existing.as_ref().map_or("", |todo| todo.body.as_str()))
        .activates_default(true)
        .build();
    content.append(&details);
    let note = {
        let model = ctx.model.borrow();
        todo_note(model.session.todos.kept(), model.session.todos.unsaved())
    };
    content.append(&text(&note, &["dialog-note"]));

    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(8)
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("control");
    cancel.add_css_class("wake");
    let save = gtk::Button::with_label(button);
    save.add_css_class("control");
    save.add_css_class("wake");
    save.add_css_class("primary");
    save.set_sensitive(!what.text().trim().is_empty());
    buttons.append(&cancel);
    buttons.append(&save);
    content.append(&buttons);

    let dialog = self::dialog(title, &content);
    dialog.set_default_widget(Some(&save));
    dialog.set_focus(Some(&what));
    {
        // The button's own closure holds the entries: this one holds the
        // button weakly so the dialog's widgets are freed when it closes.
        let save = save.downgrade();
        what.connect_changed(move |what| {
            if let Some(save) = save.upgrade() {
                save.set_sensitive(!what.text().trim().is_empty());
            }
        });
    }
    {
        let dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    {
        let ctx = ctx.clone();
        let dialog = dialog.downgrade();
        let id = existing.map(|todo| todo.id);
        save.connect_clicked(move |_| {
            let title = what.text();
            let body = details.text();
            match &id {
                Some(id) => ctx.change_session(|session| session.edit_todo(id, &title, &body)),
                None => ctx.change_todos(|todos| todos.add(&title, &body).is_some()),
            }
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
        });
    }
    show_over(ctx, &dialog);
}

/// Settings: which board to show, the auto-refresh interval, the theme,
/// the icon on Omarchy's bar (where Omarchy's shell is), and what to show
/// or hide. Rebuilt on each change, so a hidden board greys out its
/// groups and stacks.
pub fn show_settings(ctx: &Rc<Ctx>) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fill_settings(ctx, &content);
    ctx.settings_page.set(Some(&content));
    present(ctx, "Settings", &content);
}

/// Fills the open Settings again, for a change made outside it (Omarchy
/// switching themes).
pub fn refresh_settings(ctx: &Rc<Ctx>) {
    if let Some(content) = ctx.settings_page.upgrade() {
        fill_settings(ctx, &content);
    }
}

/// Fills `content` with the settings, and again on each change.
fn fill_settings(ctx: &Rc<Ctx>, content: &gtk::Box) {
    // Weakly: the switches inside `content` hold `rebuild`, and a strong
    // reference back would keep the dialog's widgets alive once it closes.
    let rebuild: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let content = content.downgrade();
        Rc::new(move || {
            if let Some(content) = content.upgrade() {
                fill_settings(&ctx, &content);
            }
        })
    };
    fill_settings_with(ctx, content, rebuild);
}

fn heading(label: &str) -> gtk::Label {
    tracked(label, &["dialog-heading"])
}

fn pill(label: &str, active: bool) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .label(label)
        .active(active)
        .build();
    button.add_css_class("filter");
    button
}

fn fill_settings_with(ctx: &Rc<Ctx>, content: &gtk::Box, rebuild: Rc<dyn Fn()>) {
    clear(content);
    if let Some(page) = ctx.current_page() {
        content.append(&heading("Sources"));
        append_sources(ctx, &page, content);
    }
    let Some(snapshot) = ctx.model.borrow().state.snapshot().cloned() else {
        content.append(&text("Nothing to set up yet.", &["empty"]));
        return;
    };
    let view = ctx.model.borrow().view.clone();
    content.append(&heading("Board"));
    content.append(&pills(
        ctx,
        &rebuild,
        board_choices(&snapshot, &view),
        |view, board| view.board = board,
    ));
    content.append(&heading("Auto-refresh"));
    content.append(&pills(
        ctx,
        &rebuild,
        interval_choices(&view),
        |view, minutes| view.auto_refresh_minutes = minutes,
    ));
    content.append(&heading("Theme"));
    content.append(&pills(
        ctx,
        &rebuild,
        theme_choices(&view),
        |view, theme| view.theme = theme,
    ));
    if let Some(note) = theme_note(&view, ctx.model.borrow().omarchy.as_ref()) {
        content.append(&text(&note, &["dialog-note"]));
    }
    if let Some((on, state)) = bar_state(ctx) {
        content.append(&heading("Omarchy bar"));
        content.append(&bar_pills(ctx, &rebuild, on));
        content.append(&text(&bar_note(&state), &["dialog-note"]));
    }
    content.append(&heading("Show or hide"));
    content.append(&visibility(ctx, &rebuild, &switches(&snapshot, &view)));
}

/// A pill for each of `choices`; choosing one applies it with `set`.
fn pills<T: Clone + 'static>(
    ctx: &Rc<Ctx>,
    rebuild: &Rc<dyn Fn()>,
    choices: Vec<Choice<T>>,
    set: fn(&mut View, T),
) -> gtk::FlowBox {
    let row = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(10)
        .build();
    for choice in choices {
        let button = pill(&choice.label, choice.active);
        let ctx = ctx.clone();
        let rebuild = rebuild.clone();
        let value = choice.value;
        button.connect_clicked(move |_| {
            let value = value.clone();
            ctx.change_view(|view| set(view, value));
            rebuild();
        });
        row.insert(&flow_child(&button), -1);
    }
    row
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

/// Off and On for the icon on Omarchy's bar.
fn bar_pills(ctx: &Rc<Ctx>, rebuild: &Rc<dyn Fn()>, on: bool) -> gtk::FlowBox {
    let row = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .build();
    let changing = matches!(ctx.model.borrow().bar_change, Some(BarState::Changing(_)));
    for choice in bar_choices(on) {
        let button = pill(&choice.label, choice.active);
        button.set_sensitive(!changing);
        let ctx = ctx.clone();
        let rebuild = rebuild.clone();
        let wanted = choice.value;
        button.connect_clicked(move |_| {
            if wanted == on {
                // Already so: the pill stays as it was.
                rebuild();
            } else {
                set_bar(&ctx, wanted, rebuild.clone());
            }
        });
        row.insert(&flow_child(&button), -1);
    }
    row
}

/// Puts the icon on Omarchy's bar (writing the plugin first) or takes it
/// off, with Omarchy's own commands, off the main loop; Settings says it
/// is under way, then how it went.
fn set_bar(ctx: &Rc<Ctx>, on: bool, rebuild: Rc<dyn Fn()>) {
    let Some(home) = ctx.home.clone() else {
        ctx.model.borrow_mut().bar_change = Some(BarState::Failed(
            "the sandbox leaves Omarchy's bar alone".to_owned(),
        ));
        rebuild();
        return;
    };
    if on && let Err(error) = omarchy_bar::write(&omarchy_bar::plugin_dir(&home)) {
        ctx.model.borrow_mut().bar_change = Some(BarState::Failed(format!(
            "could not write the plugin ({error})"
        )));
        rebuild();
        return;
    }
    ctx.model.borrow_mut().bar_change = Some(BarState::Changing(on));
    rebuild();
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
        rebuild();
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

/// The show/hide switches: each board, its groups under it, and their
/// stacks under those.
fn visibility(ctx: &Rc<Ctx>, rebuild: &Rc<dyn Fn()>, boards: &[BoardSwitches]) -> gtk::Box {
    let tree = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for (index, board) in boards.iter().enumerate() {
        let board_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        board_box.add_css_class("visibility-board");
        if index > 0 {
            board_box.add_css_class("after");
        }
        board_box.append(&switch_row(ctx, rebuild, &board.board, None, true));
        for group in &board.groups {
            let group_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_start(INDENT)
                .build();
            let logo = source_icon(ctx, group.icon.as_ref(), size::CONTROL);
            group_box.append(&switch_row(ctx, rebuild, &group.group, Some(logo), false));
            for stack in &group.stacks {
                let row = switch_row(ctx, rebuild, stack, None, false);
                row.set_margin_start(INDENT);
                group_box.append(&row);
            }
            board_box.append(&group_box);
        }
        tree.append(&board_box);
    }
    tree
}

fn flow_child(widget: &impl IsA<gtk::Widget>) -> gtk::FlowBoxChild {
    gtk::FlowBoxChild::builder()
        .child(widget)
        .focusable(false)
        .build()
}

/// The row for `switch`, after `prefix` (a logo) when there is one.
fn switch_row(
    ctx: &Rc<Ctx>,
    rebuild: &Rc<dyn Fn()>,
    switch: &SwitchView,
    prefix: Option<gtk::Widget>,
    is_board: bool,
) -> gtk::Box {
    let SwitchView {
        label,
        key,
        on,
        inherited,
    } = switch.clone();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("switch-row");
    if inherited {
        row.add_css_class("inherited");
    }
    if is_board {
        row.add_css_class("board-row");
    }
    let name = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    if let Some(prefix) = prefix {
        name.append(&prefix);
    }
    let text = text(&label, &[]);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_wrap(false);
    name.append(&text);
    row.append(&name);
    let switch = gtk::Switch::builder()
        .active(on)
        .sensitive(!inherited)
        .valign(gtk::Align::Center)
        .build();
    {
        let ctx = ctx.clone();
        let rebuild = rebuild.clone();
        switch.connect_state_set(move |_, _| {
            let key = key.clone();
            ctx.change_view(|view| flip(&mut view.hidden, &key));
            rebuild();
            gtk::glib::Propagation::Proceed
        });
    }
    row.append(&switch);
    row
}
