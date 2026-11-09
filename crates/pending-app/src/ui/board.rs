//! The page under the tabs: notices, then the Activity area (the picked
//! cards by stage) or the Board (the groups of stacks of cards). Rebuilt
//! whenever what it shows changes.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use super::dialogs;
use super::hero::show_note;
use super::icons::{size, source_icon, symbolic};
use super::{Ctx, wrapped};
use crate::keymap::{Command as KeyCommand, hinted};
use crate::present::{
    ActivityItem, ActivityView, BoardView, CardAction, CardView, GroupView, MenuItem, Notice,
    Place, StackView, StageView, TODO_UNSAVED, Tab, card_menu, link_note,
};
use crate::view::flip;

/// Narrowest a group gets; the board wraps groups onto more rows instead.
const GROUP_MIN_WIDTH: i32 = 290;
/// Narrowest a stage of the Activity area gets: the three stay side by
/// side, sharing the width, so each one may be narrower than a group.
const STAGE_MIN_WIDTH: i32 = 240;
/// Widest a card's text asks to be, in characters: wrapped labels would
/// otherwise ask for their whole text on one line and make every group as
/// wide as the window.
const CARD_TEXT_CHARS: i32 = 40;

pub fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// A label that wraps onto more lines, asking for at most
/// [`CARD_TEXT_CHARS`] of width.
fn text(label: &str, classes: &[&str]) -> gtk::Label {
    let widget = wrapped(label, classes);
    widget.set_max_width_chars(CARD_TEXT_CHARS);
    widget
}

/// Tracking for small upper-case labels, in Pango units (1024 per point):
/// about 0.04em. Set as a text attribute rather than in CSS, because GTK
/// measures a label without its CSS letter spacing and would cut it.
pub const TRACKING: i32 = 400;

/// A one-line upper-case label with tracking, at its natural width.
pub fn tracked(label: &str, classes: &[&str]) -> gtk::Label {
    let widget = gtk::Label::builder()
        .label(label.to_uppercase())
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_letter_spacing(TRACKING));
    widget.set_attributes(Some(&attributes));
    for class in classes {
        widget.add_css_class(class);
    }
    widget
}

/// A label that stays on one line and is cut with an ellipsis instead.
fn line(label: &str, classes: &[&str]) -> gtk::Label {
    let widget = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
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

/// A notice above the board or a dialog's content.
pub fn notice(kind: &str, title: &str, body: &str) -> gtk::Box {
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
    line.append(&symbolic("object-select-symbolic", size::TEXT));
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
    tile.append(&source_icon(ctx, icon, size::LOGO));
    tile
}

/// Lets go of the keyboard focus when it is inside `page_box`, before the
/// page is torn down. Otherwise the window keeps pointing at the destroyed
/// widget that had it (a card's menu button, or an entry of its menu), and
/// the next focus move reads that pointer.
fn release_focus(page_box: &gtk::Box) {
    let Some(root) = page_box.root() else {
        return;
    };
    if root
        .focus()
        .is_some_and(|focus| focus.is_ancestor(page_box))
    {
        root.set_focus(None::<&gtk::Widget>);
    }
}

/// Draws `page` into `page_box`.
pub fn render(ctx: &Rc<Ctx>, page_box: &gtk::Box, page: &BoardView) {
    release_focus(page_box);
    clear(page_box);
    match &page.notice {
        Some(Notice::Loading) => {
            page_box.append(&text("Loading…", &["empty"]));
        }
        Some(Notice::Unavailable(error)) => {
            page_box.append(&notice("unavailable", "Dashboard unavailable", error));
        }
        Some(Notice::Stale(title)) => {
            page_box.append(&notice("stale", title, ""));
        }
        None => {}
    }
    match page.tab {
        Tab::Activity => {
            if let Some(activity) = &page.activity {
                page_box.append(&activity_section(ctx, activity));
            }
            return;
        }
        Tab::Board => {}
    }
    if page.groups.is_empty() {
        return;
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
        // Not focusable itself, but focus goes into it: `can-focus` off
        // would keep the keyboard from every card inside.
        let child = gtk::FlowBoxChild::builder()
            .child(&group_widget(ctx, group))
            .focusable(false)
            .valign(gtk::Align::Start)
            .build();
        board.insert(&child, -1);
    }
    page_box.append(&board);
}

/// The Activity area: a column per stage, side by side and as tall as the
/// tallest, and a hint pointing to the Board while nothing is picked. The
/// headers share one height, so Picked's New button does not push its
/// cards below the others.
fn activity_section(ctx: &Rc<Ctx>, activity: &ActivityView) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 16);
    if let Some(why) = &activity.unsaved {
        section.append(&notice("stale", TODO_UNSAVED, why));
    }
    let stages = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .spacing(20)
        .build();
    let headers = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
    for stage in &activity.stages {
        stages.append(&stage_widget(ctx, stage, &headers));
    }
    section.append(&stages);
    if let Some(hint) = activity.hint {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::Center)
            .build();
        row.add_css_class("empty");
        row.append(&text(hint, &[]));
        let open = gtk::Button::builder().label("Open the Board").build();
        open.add_css_class("link-button");
        let ctx = ctx.clone();
        open.connect_clicked(move |_| ctx.show_tab(Tab::Board));
        row.append(&open);
        section.append(&row);
    }
    section
}

/// One stage of the Activity area: its name and count (and, on Picked,
/// the button that writes down a todo), then its cards.
fn stage_widget(ctx: &Rc<Ctx>, stage: &StageView, headers: &gtk::SizeGroup) -> gtk::Box {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .width_request(STAGE_MIN_WIDTH)
        .hexpand(true)
        .build();
    widget.add_css_class("group");
    widget.add_css_class("stage");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("stage-title");
    header.append(&tracked(stage.name, &[]));
    let count = gtk::Label::new(Some(&stage.cards.len().to_string()));
    count.add_css_class("count");
    header.append(&count);
    if stage.adds_todo {
        header.append(&new_todo_button(ctx));
    }
    headers.add_widget(&header);
    widget.append(&header);
    if stage.cards.is_empty() {
        widget.append(&empty_line(stage.empty_note));
        return widget;
    }
    let cards = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for item in &stage.cards {
        cards.append(&activity_card(ctx, item));
    }
    widget.append(&cards);
    widget
}

/// "New" at the end of a header, which opens the dialog for a todo.
fn new_todo_button(ctx: &Rc<Ctx>) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&symbolic("list-add-symbolic", size::SMALL));
    content.append(&gtk::Label::new(Some("New")));
    let add = gtk::Button::builder()
        .child(&content)
        .halign(gtk::Align::End)
        .hexpand(true)
        .tooltip_text(hinted("Write down a todo", KeyCommand::NewTodo))
        .build();
    add.add_css_class("empty-toggle");
    let ctx = ctx.clone();
    add.connect_clicked(move |_| dialogs::show_todo(&ctx, None));
    add
}

/// A picked card, with its source under it.
fn activity_card(ctx: &Rc<Ctx>, item: &ActivityItem) -> gtk::Box {
    let card = card_widget(ctx, &item.card, Place::Activity);
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    footer.add_css_class("card-source");
    footer.append(&source_icon(ctx, item.icon.as_ref(), size::SMALL));
    footer.append(&line(&item.source, &[]));
    card.append(&footer);
    card
}

fn group_widget(ctx: &Rc<Ctx>, group: &GroupView) -> gtk::Box {
    // Set explicitly, so the expanding labels inside do not make the group
    // claim a whole row of the board.
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .width_request(GROUP_MIN_WIDTH)
        .hexpand(false)
        .build();
    widget.add_css_class("group");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title.add_css_class("group-title");
    title.append(&source_tile(ctx, group.icon.as_ref()));
    title.append(&line(&group.source, &[]));
    header.append(&title);
    let board = gtk::Label::new(Some(&group.board));
    board.add_css_class("badge");
    header.append(&board);
    if let Some(empty) = &group.empty_toggle {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&symbolic(empty.icon, size::SMALL));
        content.append(&gtk::Label::new(Some(&empty.label)));
        let toggle = gtk::Button::builder()
            .child(&content)
            .halign(gtk::Align::End)
            .hexpand(true)
            .tooltip_text(empty.tooltip)
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
        stacks.append(&stack_widget(ctx, stack, index > 0));
    }
    widget.append(&stacks);
    widget
}

/// One stack: a header that collapses it, then its cards.
fn stack_widget(ctx: &Rc<Ctx>, stack: &StackView, after: bool) -> gtk::Box {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
    widget.add_css_class("stack");
    if after {
        widget.add_css_class("after");
    }

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.append(&symbolic("pan-down-symbolic", size::TEXT));
    content.append(&tracked(&stack.name, &[]));
    let count = gtk::Label::new(Some(&stack.count.to_string()));
    count.add_css_class("count");
    content.append(&count);
    if stack.changed {
        content.append(&dot());
    }
    let header = gtk::Button::builder()
        .child(&content)
        .halign(gtk::Align::Fill)
        .build();
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
        cards.append(&card_widget(ctx, card, Place::Board));
    }
    if let Some(label) = &stack.more_label {
        let more = gtk::Button::builder()
            .label(label)
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

/// A card: its title (a link when it has one), the three-dot menu, and two
/// lines of its body with the whole text as a tooltip.
fn card_widget(ctx: &Rc<Ctx>, card: &CardView, place: Place) -> gtk::Box {
    let widget = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(false)
        .build();
    widget.add_css_class("card");
    if card.picked() {
        widget.add_css_class("marked");
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if card.changed && place == Place::Board {
        header.append(&dot());
    }
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(CARD_TEXT_CHARS)
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
                let ctx = ctx.clone();
                launcher.launch(
                    ctx.window().as_ref(),
                    gtk::gio::Cancellable::NONE,
                    move |result| {
                        // Closing the app chooser is not a failure.
                        let Err(error) = result else { return };
                        if error.matches(gtk::DialogError::Dismissed)
                            || error.matches(gtk::DialogError::Cancelled)
                        {
                            return;
                        }
                        if let Some(widgets) = ctx.widgets.borrow().as_ref() {
                            show_note(widgets, &link_note(error.message()));
                        }
                    },
                );
                gtk::glib::Propagation::Stop
            });
        }
        None => title.set_label(&card.title),
    }
    header.append(&title);

    header.append(&menu_button(ctx, card, place));
    widget.append(&header);

    if !card.body.is_empty() {
        let body = text(&card.body, &["card-body"]);
        body.set_ellipsize(gtk::pango::EllipsizeMode::End);
        body.set_lines(2);
        body.set_tooltip_text(Some(&card.body));
        widget.append(&body);
    }
    if let Some(alert) = &card.alert {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("card-alert");
        row.append(&symbolic("alarm-symbolic", size::SMALL));
        row.append(&line(alert, &[]));
        widget.append(&row);
    }
    widget
}

/// The three dots at a card's corner, with the menu of what can be done to
/// it. Each entry is an action of the button's own group, so the popover
/// finds them whatever card the button is on.
fn menu_button(ctx: &Rc<Ctx>, card: &CardView, place: Place) -> gtk::MenuButton {
    let model = gtk::gio::Menu::new();
    let actions = gtk::gio::SimpleActionGroup::new();
    let mut entries = 0;
    let mut add = |menu: &gtk::gio::Menu, label: &str, action: CardAction| {
        let name = format!("item-{entries}");
        entries += 1;
        let simple = gtk::gio::SimpleAction::new(&name, None);
        let ctx = ctx.clone();
        let card = card.clone();
        simple.connect_activate(move |_, _| {
            // A window-side action redraws the page, menu included: that
            // waits until the popover is done with the click.
            let ctx = ctx.clone();
            let card = card.clone();
            gtk::glib::idle_add_local_once(move || ctx.act(&card, action));
        });
        actions.add_action(&simple);
        menu.append(Some(label), Some(&format!("card.{name}")));
    };
    for item in card_menu(card, place) {
        match item {
            MenuItem::Action { label, action } => add(&model, &label, action),
            MenuItem::Submenu { label, items } => {
                let submenu = gtk::gio::Menu::new();
                for (label, action) in items {
                    add(&submenu, &label, action);
                }
                model.append_submenu(Some(&label), &submenu);
            }
        }
    }
    let popover = gtk::PopoverMenu::from_model(Some(&model));
    popover.add_css_class("card-menu");
    popover.set_has_arrow(false);
    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .popover(&popover)
        .tooltip_text("Card menu")
        .valign(gtk::Align::Start)
        .build();
    button.add_css_class("card-menu");
    button.insert_action_group("card", Some(&actions));
    button
}
