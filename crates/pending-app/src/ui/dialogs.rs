//! The dialogs: sources, settings and the snoozed cards.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use super::icons::{source_icon, symbolic};
use super::{Ctx, wrapped as text};
use crate::backend::Command;
use crate::view::{AUTO_REFRESH_CHOICES, View, flip, hide_key};

/// Every dialog's width: enough for a source's message or a stack's name
/// on one line, and the same for all three so they open in one place.
const DIALOG_WIDTH: i32 = 600;

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
        let notice = gtk::Box::new(gtk::Orientation::Vertical, 2);
        notice.add_css_class("notice");
        notice.add_css_class("stale");
        notice.append(&text(
            "Config not reloaded (the previous one is still running)",
            &["notice-title"],
        ));
        notice.append(&text(error, &["notice-body"]));
        content.append(&notice);
    }
    for source in &page.sources {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.add_css_class("row-card");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.append(&source_icon(ctx, source.icon.as_ref(), 18));
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
        status.append(&symbolic(source.status_icon, 14));
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
        footer.append(&source_icon(ctx, entry.icon.as_ref(), 13));
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
    super::board::tracked(label, &["dialog-heading"])
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
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
    let Some(snapshot) = ctx.model.borrow().state.snapshot().cloned() else {
        content.append(&text("Nothing to set up yet.", &["empty"]));
        return;
    };
    let view = ctx.model.borrow().view.clone();

    content.append(&heading("Board"));
    let boards = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(10)
        .build();
    let mut choices = vec![("All".to_owned(), None)];
    choices.extend(
        snapshot
            .boards
            .iter()
            .map(|board| (board.name.clone(), Some(board.name.clone()))),
    );
    for (label, value) in choices {
        let button = pill(&label, view.board == value);
        let ctx = ctx.clone();
        let rebuild = rebuild.clone();
        button.connect_clicked(move |_| {
            let value = value.clone();
            ctx.change_view(|view| view.board = value);
            rebuild();
        });
        boards.insert(&flow_child(&button), -1);
    }
    content.append(&boards);

    content.append(&heading("Auto-refresh"));
    let intervals = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(10)
        .build();
    for minutes in AUTO_REFRESH_CHOICES {
        let label = if minutes == 0 {
            "Off".to_owned()
        } else {
            format!("{minutes} min")
        };
        let button = pill(&label, view.auto_refresh_minutes == minutes);
        let ctx = ctx.clone();
        let rebuild = rebuild.clone();
        button.connect_clicked(move |_| {
            ctx.change_view(|view| view.auto_refresh_minutes = minutes);
            rebuild();
        });
        intervals.insert(&flow_child(&button), -1);
    }
    content.append(&intervals);

    content.append(&heading("Show or hide"));
    let tree = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for (index, board) in snapshot.boards.iter().enumerate() {
        let board_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        board_box.add_css_class("visibility-board");
        if index > 0 {
            board_box.add_css_class("after");
        }
        let board_off = view.hidden.contains(&hide_key::board(&board.name));
        board_box.append(&switch_row(
            ctx,
            &rebuild,
            &view,
            SwitchRow {
                label: &board.name,
                key: hide_key::board(&board.name),
                inherited: false,
                prefix: None,
                is_board: true,
            },
        ));
        for group in &board.groups {
            let group_off = view
                .hidden
                .contains(&hide_key::group(&board.name, &group.source));
            let group_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_start(18)
                .build();
            group_box.append(&switch_row(
                ctx,
                &rebuild,
                &view,
                SwitchRow {
                    label: &group.source,
                    key: hide_key::group(&board.name, &group.source),
                    inherited: board_off,
                    prefix: Some(source_icon(ctx, group.icon.as_ref(), 16)),
                    is_board: false,
                },
            ));
            for column in &group.columns {
                let row = switch_row(
                    ctx,
                    &rebuild,
                    &view,
                    SwitchRow {
                        label: &column.name,
                        key: hide_key::column(&board.name, &group.source, &column.name),
                        inherited: board_off || group_off,
                        prefix: None,
                        is_board: false,
                    },
                );
                row.set_margin_start(18);
                group_box.append(&row);
            }
            board_box.append(&group_box);
        }
        tree.append(&board_box);
    }
    content.append(&tree);
}

fn flow_child(widget: &impl IsA<gtk::Widget>) -> gtk::FlowBoxChild {
    gtk::FlowBoxChild::builder()
        .child(widget)
        .focusable(false)
        .can_focus(false)
        .build()
}

/// One thing the settings can show or hide.
struct SwitchRow<'a> {
    label: &'a str,
    /// Its entry in the view's hidden set (see [`hide_key`]).
    key: String,
    /// A parent is hidden: the switch is greyed out, keeping its own
    /// setting for when the parent comes back.
    inherited: bool,
    /// A logo before the label.
    prefix: Option<gtk::Widget>,
    is_board: bool,
}

/// The show/hide switch for `row`.
fn switch_row(ctx: &Rc<Ctx>, rebuild: &Rc<dyn Fn()>, view: &View, row: SwitchRow) -> gtk::Box {
    let SwitchRow {
        label,
        key,
        inherited,
        prefix,
        is_board,
    } = row;
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
    let text = text(label, &[]);
    text.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.set_wrap(false);
    name.append(&text);
    row.append(&name);
    let switch = gtk::Switch::builder()
        .active(!view.hidden.contains(&key))
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
