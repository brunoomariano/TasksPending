//! The dialogs: sources, settings and the snoozed cards.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use pending_core::SourceStatus;

use super::Ctx;
use super::icons::{source_icon, symbolic};
use crate::backend::Command;
use crate::view::{AUTO_REFRESH_CHOICES, View, flip, hide_key};

fn text(label: &str, classes: &[&str]) -> gtk::Label {
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

/// A dialog titled `title` over the window, holding `content`.
fn present(ctx: &Rc<Ctx>, title: &str, content: &gtk::Box) -> adw::Dialog {
    content.add_css_class("dialog-page");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(content)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(600)
        .content_height(640)
        .child(&view)
        .build();
    dialog.add_css_class("tasks-pending");
    dialog.present(ctx.window().as_ref());
    dialog
}

fn status_icon(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Ready => "object-select-symbolic",
        SourceStatus::Refreshing => "view-refresh-symbolic",
        SourceStatus::Degraded => "dialog-warning-symbolic",
        SourceStatus::Failed => "process-stop-symbolic",
    }
}

fn status_class(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Ready => "ready",
        SourceStatus::Refreshing => "refreshing",
        SourceStatus::Degraded => "degraded",
        SourceStatus::Failed => "failed",
    }
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
        status.add_css_class(status_class(source.status));
        status.append(&symbolic(status_icon(source.status), 14));
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
    let Some(page) = ctx.current_page() else {
        return;
    };
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    if page.snoozed.is_empty() {
        content.append(&text("Nothing snoozed.", &["empty"]));
    }
    let dialog = Rc::new(std::cell::RefCell::new(None::<adw::Dialog>));
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
        footer.append(&text(&format!("{} · {}", entry.source, entry.until), &[]));
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
                if let Some(dialog) = dialog.borrow().as_ref() {
                    dialog.close();
                }
            });
        }
        row.append(&wake);
        content.append(&row);
    }
    *dialog.borrow_mut() = Some(present(ctx, "Snoozed", &content));
}

/// Settings: which board to show, the auto-refresh interval, and what to
/// show or hide. Rebuilt on each change, so a hidden board greys out its
/// groups and stacks.
pub fn show_settings(ctx: &Rc<Ctx>) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let rebuild: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let content = content.clone();
        Rc::new(move || fill_settings(&ctx, &content))
    };
    fill_settings_with(ctx, &content, rebuild);
    present(ctx, "Settings", &content);
}

fn fill_settings(ctx: &Rc<Ctx>, content: &gtk::Box) {
    let rebuild: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        let content = content.clone();
        Rc::new(move || fill_settings(&ctx, &content))
    };
    fill_settings_with(ctx, content, rebuild);
}

fn heading(label: &str) -> gtk::Label {
    text(&label.to_uppercase(), &["dialog-heading"])
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
            &board.name,
            hide_key::board(&board.name),
            &view,
            false,
            None,
            true,
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
                &group.source,
                hide_key::group(&board.name, &group.source),
                &view,
                board_off,
                Some(source_icon(ctx, group.icon.as_ref(), 16)),
                false,
            ));
            for column in &group.columns {
                let row = switch_row(
                    ctx,
                    &rebuild,
                    &column.name,
                    hide_key::column(&board.name, &group.source, &column.name),
                    &view,
                    board_off || group_off,
                    None,
                    false,
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

/// A show/hide switch; `inherited` greys it out when a parent is hidden,
/// keeping its own setting for when the parent comes back.
#[allow(clippy::too_many_arguments)]
fn switch_row(
    ctx: &Rc<Ctx>,
    rebuild: &Rc<dyn Fn()>,
    label: &str,
    key: String,
    view: &View,
    inherited: bool,
    prefix: Option<gtk::Widget>,
    is_board: bool,
) -> gtk::Box {
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
