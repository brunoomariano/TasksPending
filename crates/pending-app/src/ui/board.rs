//! The page under the hero: notices, the marked cards, and the groups of
//! stacks of cards. Rebuilt whenever what it shows changes.

use std::rc::Rc;

use chrono::{Local, Utc};
use gtk4 as gtk;
use gtk4::prelude::*;

use super::Ctx;
use super::icons::{source_icon, symbolic};
use crate::backend::Command;
use crate::present::{CardView, GroupView, NowItem, Page, StackView, TIME_FORMAT};
use crate::snooze::{CHOICES, until};
use crate::state::ViewState;
use crate::view::{View, flip};

/// Narrowest a group gets; the board wraps groups onto more rows instead.
const GROUP_MIN_WIDTH: i32 = 290;

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

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

fn dot() -> gtk::Box {
    let dot = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .tooltip_text("Changed since you last looked")
        .build();
    dot.add_css_class("changed-dot");
    dot
}

/// A notice above the board.
fn notice(kind: &str, title: &str, body: &str) -> gtk::Box {
    let notice = gtk::Box::new(gtk::Orientation::Vertical, 4);
    notice.add_css_class("notice");
    notice.add_css_class(kind);
    notice.append(&text(title, &["notice-title"]));
    if !body.is_empty() {
        notice.append(&text(body, &["notice-body"]));
    }
    notice
}

fn empty_line(message: &str) -> gtk::Box {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    line.add_css_class("empty");
    line.append(&symbolic("object-select-symbolic", 14));
    line.append(&text(message, &[]));
    line
}

/// "Nothing pending." for a whole group: a dashed box, so the quiet is
/// visibly on purpose.
fn empty_state(message: &str) -> gtk::Box {
    let state = empty_line(message);
    state.add_css_class("empty-state");
    state
}

/// The source's logo in a small tile, for group headers.
fn source_tile(ctx: &Rc<Ctx>, icon: Option<&pending_core::Icon>) -> gtk::Box {
    let tile = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    tile.add_css_class("source-tile");
    tile.append(&source_icon(ctx, icon, 18));
    tile
}

/// Draws `state` into `page_box`.
pub fn render(
    ctx: &Rc<Ctx>,
    page_box: &gtk::Box,
    state: &ViewState,
    page: Option<&Page>,
    view: &View,
) {
    clear(page_box);
    match state {
        ViewState::Loading => {
            page_box.append(&text("Loading…", &["empty"]));
        }
        ViewState::Unavailable { error } => {
            page_box.append(&notice("unavailable", "Dashboard unavailable", error));
        }
        ViewState::Stale {
            error, fetched_at, ..
        } => {
            page_box.append(&notice(
                "stale",
                &format!(
                    "Daemon unreachable ({error}); showing data from {}",
                    fetched_at.format(TIME_FORMAT)
                ),
                "",
            ));
        }
        ViewState::Ready { .. } => {}
    }
    let Some(page) = page else {
        return;
    };
    if !page.now.is_empty() {
        page_box.append(&now_section(ctx, &page.now));
    }
    let board = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(false)
        .column_spacing(20)
        .row_spacing(20)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .valign(gtk::Align::Start)
        .build();
    board.add_css_class("board");
    for group in &page.groups {
        let child = gtk::FlowBoxChild::builder()
            .child(&group_widget(ctx, group, view))
            .focusable(false)
            .can_focus(false)
            .valign(gtk::Align::Start)
            .build();
        board.insert(&child, -1);
    }
    page_box.append(&board);
}

/// The cards marked as in progress, above the groups.
fn now_section(ctx: &Rc<Ctx>, items: &[NowItem]) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 10);
    section.add_css_class("now");
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title.add_css_class("now-title");
    title.append(&symbolic("view-pin-symbolic", 14));
    title.append(&text("NOW", &[]));
    let count = gtk::Label::new(Some(&items.len().to_string()));
    count.add_css_class("count");
    title.append(&count);
    section.append(&title);
    let cards = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .column_spacing(8)
        .row_spacing(8)
        .min_children_per_line(1)
        .max_children_per_line(6)
        .build();
    for item in items {
        let card = card_widget(ctx, &item.card, true);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.add_css_class("card-source");
        footer.append(&source_icon(ctx, item.icon.as_ref(), 13));
        footer.append(&text(&item.source, &[]));
        card.append(&footer);
        let child = gtk::FlowBoxChild::builder()
            .child(&card)
            .focusable(false)
            .can_focus(false)
            .build();
        cards.insert(&child, -1);
    }
    section.append(&cards);
    section
}

fn group_widget(ctx: &Rc<Ctx>, group: &GroupView, view: &View) -> gtk::Box {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .width_request(GROUP_MIN_WIDTH)
        .build();
    widget.add_css_class("group");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title.add_css_class("group-title");
    title.append(&source_tile(ctx, group.icon.as_ref()));
    title.append(&text(&group.source, &[]));
    header.append(&title);
    let board = gtk::Label::new(Some(&group.board));
    board.add_css_class("badge");
    header.append(&board);
    if group.empty > 0 {
        let (icon, label) = if group.expanded {
            ("view-conceal-symbolic", "hide empty".to_owned())
        } else {
            ("view-reveal-symbolic", format!("{} empty", group.empty))
        };
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&symbolic(icon, 13));
        content.append(&gtk::Label::new(Some(&label)));
        let toggle = gtk::Button::builder()
            .child(&content)
            .halign(gtk::Align::End)
            .hexpand(true)
            .tooltip_text(if group.expanded {
                "Hide empty stacks"
            } else {
                "Show empty stacks"
            })
            .build();
        toggle.add_css_class("empty-toggle");
        let ctx = ctx.clone();
        let key = group.key.clone();
        toggle.connect_clicked(move |_| {
            let key = key.clone();
            ctx.change_view(|view| flip(&mut view.expanded, &key));
        });
        header.append(&toggle);
    }
    widget.append(&header);

    if group.stacks.is_empty() {
        widget.append(&empty_state("Nothing pending."));
        return widget;
    }
    let stacks = gtk::Box::new(gtk::Orientation::Vertical, 14);
    for (index, stack) in group.stacks.iter().enumerate() {
        stacks.append(&stack_widget(ctx, stack, index > 0, view));
    }
    widget.append(&stacks);
    widget
}

/// One stack: a header that collapses it, then its cards.
fn stack_widget(ctx: &Rc<Ctx>, stack: &StackView, after: bool, _view: &View) -> gtk::Box {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
    widget.add_css_class("stack");
    if after {
        widget.add_css_class("after");
    }

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&symbolic("pan-down-symbolic", 14));
    content.append(&text(&stack.name.to_uppercase(), &[]));
    let count = gtk::Label::new(Some(&stack.count.to_string()));
    count.add_css_class("count");
    content.append(&count);
    if stack.changed {
        content.append(&dot());
    }
    let header = gtk::Button::builder().child(&content).build();
    header.add_css_class("stack-header");
    if stack.collapsed {
        header.add_css_class("collapsed");
    }
    {
        let ctx = ctx.clone();
        let key = stack.key.clone();
        header.connect_clicked(move |_| {
            let key = key.clone();
            ctx.change_view(|view| flip(&mut view.collapsed, &key));
        });
    }
    widget.append(&header);
    if stack.collapsed {
        return widget;
    }

    if let Some(note) = &stack.empty_note {
        widget.append(&empty_line(note));
        return widget;
    }
    let cards = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for card in &stack.cards {
        cards.append(&card_widget(ctx, card, false));
    }
    if stack.more > 0 || (stack.open && stack.count > crate::present::COLUMN_LIMIT) {
        let more = gtk::Button::builder()
            .label(if stack.open {
                "Show less".to_owned()
            } else {
                format!("Show {} more", stack.more)
            })
            .halign(gtk::Align::Center)
            .build();
        more.add_css_class("link-button");
        let ctx = ctx.clone();
        let key = stack.key.clone();
        more.connect_clicked(move |_| {
            let key = key.clone();
            ctx.change_view(|view| flip(&mut view.open_columns, &key));
        });
        cards.append(&more);
    }
    widget.append(&cards);
    widget
}

/// A card: its title (a link when it has one), the snooze and pin buttons,
/// and two lines of its body with the whole text as a tooltip.
fn card_widget(ctx: &Rc<Ctx>, card: &CardView, in_now: bool) -> gtk::Box {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 4);
    widget.add_css_class("card");
    if card.marked {
        widget.add_css_class("marked");
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if card.changed && !in_now {
        header.append(&dot());
    }
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .hexpand(true)
        .valign(gtk::Align::Start)
        .build();
    title.add_css_class("card-title");
    match &card.url {
        Some(url) => {
            title.set_use_markup(true);
            title.set_markup(&format!(
                "<a href=\"{}\">{}</a>",
                gtk::glib::markup_escape_text(url),
                gtk::glib::markup_escape_text(&card.title)
            ));
            let ctx = ctx.clone();
            title.connect_activate_link(move |_, uri| {
                let launcher = gtk::UriLauncher::new(uri);
                launcher.launch(ctx.window().as_ref(), gtk::gio::Cancellable::NONE, |_| {});
                gtk::glib::Propagation::Stop
            });
        }
        None => title.set_label(&card.title),
    }
    header.append(&title);

    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(2)
        .valign(gtk::Align::Start)
        .build();
    if !in_now {
        actions.append(&snooze_button(ctx, &card.id));
    }
    let mark = gtk::ToggleButton::builder()
        .icon_name("view-pin-symbolic")
        .active(card.marked)
        .tooltip_text(if card.marked {
            "Unmark as in progress"
        } else {
            "Mark as in progress"
        })
        .build();
    mark.add_css_class("mark");
    {
        let ctx = ctx.clone();
        let id = card.id.clone();
        mark.connect_clicked(move |button| {
            // The snapshot holds the marks: ask, then read it again. The
            // button stays off until the page redraws, so a second click
            // cannot send the same change twice.
            button.set_sensitive(false);
            ctx.send(Command::SetMark {
                id: id.clone(),
                marked: button.is_active(),
            });
        });
    }
    actions.append(&mark);
    header.append(&actions);
    widget.append(&header);

    if !card.body.is_empty() {
        let body = text(&card.body, &["card-body"]);
        body.set_ellipsize(gtk::pango::EllipsizeMode::End);
        body.set_lines(2);
        body.set_tooltip_text(Some(&card.body));
        widget.append(&body);
    }
    widget
}

/// The moon button with the snooze choices in a popover.
fn snooze_button(ctx: &Rc<Ctx>, id: &str) -> gtk::MenuButton {
    let choices = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let popover = gtk::Popover::builder()
        .child(&choices)
        .has_arrow(false)
        .build();
    popover.add_css_class("snooze-menu");
    for (choice, label) in CHOICES {
        let button = gtk::Button::with_label(label);
        let ctx = ctx.clone();
        let id = id.to_owned();
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            popover.popdown();
            ctx.send(Command::Snooze {
                id: id.clone(),
                until: until(choice, &Local::now()).map(|at| at.with_timezone(&Utc)),
            });
        });
        choices.append(&button);
    }
    let menu = gtk::MenuButton::builder()
        .icon_name("weather-clear-night-symbolic")
        .popover(&popover)
        .tooltip_text("Snooze")
        .valign(gtk::Align::Start)
        .build();
    menu.add_css_class("mark");
    menu
}
