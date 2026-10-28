//! The dialogs: sources, settings and the snoozed cards.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use super::board::{clear, notice, tracked};
use super::icons::{size, source_icon, symbolic};
use super::{Ctx, wrapped as text};
use crate::backend::Command;
use crate::present::{
    BoardSwitches, Choice, SwitchView, board_choices, interval_choices, switches,
};
use crate::view::{View, flip};

/// Every dialog's width: enough for a source's message or a stack's name
/// on one line, and the same for all three so they open in one place.
const DIALOG_WIDTH: i32 = 600;
/// How far each level of the show/hide tree is set in from the one above.
const INDENT: i32 = 18;

/// A dialog titled `title` over the window, holding `content`.
fn present(ctx: &Rc<Ctx>, title: &str, content: &gtk::Box) {
    dialog(title, content).present(ctx.window().as_ref());
}

/// A dialog titled `title` holding `content`, not shown yet.
fn dialog(title: &str, content: &gtk::Box) -> adw::Dialog {
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
/// the file on disk was rejected.
pub fn show_sources(ctx: &Rc<Ctx>) {
    let Some(page) = ctx.current_page() else {
        return;
    };
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
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
    present(ctx, "Sources", &content);
}

/// The snoozed cards, each with when it comes back and a button to wake it.
pub fn show_snoozed(ctx: &Rc<Ctx>) {
    if let Some(dialog) = snoozed_dialog(ctx) {
        dialog.present(ctx.window().as_ref());
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

/// Settings: which board to show, the auto-refresh interval, and what to
/// show or hide. Rebuilt on each change, so a hidden board greys out its
/// groups and stacks.
pub fn show_settings(ctx: &Rc<Ctx>) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fill_settings(ctx, &content);
    present(ctx, "Settings", &content);
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
