//! The window's keys: the handler that runs them and the dialog that lists
//! them, searched by what each does. Both read [`crate::keymap`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gtk4 as gtk;
use gtk4::glib::Propagation;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use super::board::{self, tracked};
use super::dialogs::{self, dialog};
use super::{Ctx, hero, wrapped as text};
use crate::keymap::{self, Binding, Command, DialogKey, Focus};
use crate::nav::{self, Move};
use crate::present::{self, Tab};

/// Listens for the window's keys before the focused widget does, so a
/// button with the focus does not take a letter, and leaves them alone
/// while a dialog is open, a menu has the focus or the focus is where one
/// types.
pub fn listen(ctx: &Rc<Ctx>, window: &adw::ApplicationWindow) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let ctx = Rc::downgrade(ctx);
        let window = window.downgrade();
        move |_, key, _, mods| {
            let (Some(ctx), Some(window)) = (ctx.upgrade(), window.upgrade()) else {
                return Propagation::Proceed;
            };
            if !takes_keys(&window) {
                return Propagation::Proceed;
            }
            let focus = match board::focused(&ctx) {
                Some((_, true)) => Focus::OnCard,
                Some((_, false)) => Focus::InCard,
                None => Focus::Elsewhere,
            };
            match keymap::lookup(key, mods, focus) {
                Some(command) => {
                    run(&ctx, command);
                    Propagation::Stop
                }
                None => Propagation::Proceed,
            }
        }
    });
    window.add_controller(keys);
}

/// Whether the window's own keys apply: not while a dialog is open, a
/// menu has the focus, or the focus is in a text field.
fn takes_keys(window: &adw::ApplicationWindow) -> bool {
    if window.visible_dialog().is_some() {
        return false;
    }
    let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else {
        return true;
    };
    let typing =
        focus.is::<gtk::Editable>() || focus.ancestor(gtk::Editable::static_type()).is_some();
    let in_menu = focus.ancestor(gtk::Popover::static_type()).is_some();
    !typing && !in_menu
}

/// Does `command`, as its key or the keys dialog asked.
pub fn run(ctx: &Rc<Ctx>, command: Command) {
    match command {
        Command::ShowKeys => show(ctx),
        Command::NewTodo => {
            // A todo lands in the Activity: it is in front when it does.
            ctx.show_tab(Tab::Activity);
            dialogs::show_todo(ctx, None);
        }
        Command::Refresh => {
            // What a click does, and only when a click would: not while
            // the button waits.
            let button = ctx.widgets.borrow().as_ref().map(hero::refresh_control);
            if let Some(button) = button.filter(|button| button.is_sensitive()) {
                button.emit_clicked();
            }
        }
        Command::Sources => dialogs::show_sources(ctx),
        Command::Settings => dialogs::show_settings(ctx),
        Command::ActivityTab => ctx.show_tab(Tab::Activity),
        Command::BoardTab => ctx.show_tab(Tab::Board),
        Command::SendAway => {
            ctx.escape();
        }
        Command::Quit => {
            if let Some(window) = ctx.window() {
                let _ = WidgetExt::activate_action(&window, "app.quit", None);
            }
        }
        Command::Up => move_focus(ctx, Move::Up),
        Command::Down => move_focus(ctx, Move::Down),
        Command::Left => move_focus(ctx, Move::Left),
        Command::Right => move_focus(ctx, Move::Right),
        Command::First => move_focus(ctx, Move::First),
        Command::Last => move_focus(ctx, Move::Last),
        Command::Pick
        | Command::NextStage
        | Command::PreviousStage
        | Command::OpenLink
        | Command::EditTodo
        | Command::Menu
        | Command::Alert
        | Command::Snooze
        | Command::Delete => on_card(ctx, command),
    }
}

/// Moves the keyboard focus between the cards, from the one that has it,
/// or to the first one.
fn move_focus(ctx: &Rc<Ctx>, step: Move) {
    let from = board::focused(ctx).map(|(spot, _)| spot);
    if let Some(spot) = nav::step(&board::column_lengths(ctx), from, step) {
        board::focus_card(ctx, spot, true);
    }
}

/// Does `command` to the card in focus, as its menu would.
fn on_card(ctx: &Rc<Ctx>, command: Command) {
    let Some((spot, _)) = board::focused(ctx) else {
        return;
    };
    let (card, place, widget, menu) = {
        let cards = ctx.cards.borrow();
        let slot = &cards[spot.0][spot.1];
        (
            slot.card.clone(),
            slot.place,
            slot.widget.clone(),
            slot.menu.clone(),
        )
    };
    match command {
        Command::OpenLink => {
            if let Some(url) = &card.url {
                board::open_link(ctx, url);
            }
        }
        Command::Menu => {
            if let Some(menu) = menu.upgrade() {
                menu.popup();
            }
        }
        Command::Alert | Command::Snooze => {
            let entries = present::quick_menu(&card, place, command);
            if let Some(widget) = widget.upgrade().filter(|_| !entries.is_empty()) {
                board::quick_menu_on(ctx, &card, place, &widget, entries);
            }
        }
        Command::Delete => {
            let Some(action) = present::key_action(&card, place, command) else {
                return;
            };
            if ctx.armed.borrow_mut().press(&card.id, Instant::now()) {
                ctx.act(&card, action);
            } else if let Some(widgets) = ctx.widgets.borrow().as_ref() {
                hero::show_note(widgets, &present::delete_note(&card.title));
            }
        }
        _ => {
            if let Some(action) = present::key_action(&card, place, command) {
                ctx.act(&card, action);
            }
        }
    }
}

/// The keys dialog: a search by what a key does over the list of keys.
/// The arrows pick one, Enter or a click runs it, and Esc (or Backspace
/// with nothing typed) closes it.
pub fn show(ctx: &Rc<Ctx>) {
    let Some(window) = ctx.window() else {
        return;
    };
    // The card in focus, which a card's command from the list acts on.
    let card = board::focused(ctx).and_then(|(spot, _)| {
        ctx.cards.borrow()[spot.0][spot.1]
            .widget
            .upgrade()
            .map(|card| card.downgrade())
    });
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search by what it does")
        .hexpand(true)
        .build();
    content.append(&search);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("keys");
    content.append(&list);
    let none = text("No key does that.", &["empty"]);
    none.set_visible(false);
    content.append(&none);
    let dialog = dialog("Keys", &content);

    // The rows that run something, in the order shown.
    let rows: Rc<RefCell<Vec<(gtk::ListBoxRow, Command)>>> = Rc::default();
    let refill = {
        let list = list.clone();
        let none = none.clone();
        let rows = rows.clone();
        move |query: &str| {
            let found = keymap::search(query);
            none.set_visible(found.is_empty());
            *rows.borrow_mut() = fill(&list, &found, query.trim().is_empty());
            if let Some((first, _)) = rows.borrow().first() {
                list.select_row(Some(first));
            }
        }
    };
    refill("");
    search.connect_search_changed({
        let refill = refill.clone();
        move |search| refill(&search.text())
    });

    let launch = {
        let ctx = Rc::downgrade(ctx);
        let dialog = dialog.downgrade();
        let rows = rows.clone();
        let card = card.clone();
        move |row: &gtk::ListBoxRow| {
            let command = rows
                .borrow()
                .iter()
                .find(|(shown, _)| shown == row)
                .map(|(_, command)| *command);
            let (Some(command), Some(ctx), Some(dialog)) =
                (command, ctx.upgrade(), dialog.upgrade())
            else {
                return;
            };
            dialog.close();
            // Once the dialog is gone, so that what the command opens is
            // not under it, and its keys reach the window again; on the
            // card that had the focus before it opened.
            let card = card.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(card) = card.and_then(|card| card.upgrade()) {
                    card.grab_focus();
                }
                run(&ctx, command);
            });
        }
    };
    list.connect_row_activated({
        let launch = launch.clone();
        move |_, row| launch(row)
    });

    let steer = gtk::EventControllerKey::new();
    steer.set_propagation_phase(gtk::PropagationPhase::Capture);
    steer.connect_key_pressed({
        let dialog = dialog.downgrade();
        let search = search.downgrade();
        let list = list.downgrade();
        let rows = rows.clone();
        move |_, key, _, mods| {
            let (Some(dialog), Some(search), Some(list)) =
                (dialog.upgrade(), search.upgrade(), list.upgrade())
            else {
                return Propagation::Proceed;
            };
            let Some(step) = keymap::dialog_key(key, mods, search.text().is_empty()) else {
                return Propagation::Proceed;
            };
            let rows = rows.borrow();
            let at = list
                .selected_row()
                .and_then(|row| rows.iter().position(|(shown, _)| *shown == row));
            match step {
                DialogKey::Close => {
                    dialog.close();
                }
                DialogKey::Previous | DialogKey::Next => {
                    let next = match (step, at) {
                        (DialogKey::Previous, Some(at)) => at.saturating_sub(1),
                        (DialogKey::Next, Some(at)) => (at + 1).min(rows.len().saturating_sub(1)),
                        _ => 0,
                    };
                    if let Some((row, _)) = rows.get(next) {
                        list.select_row(Some(row));
                        reveal(row);
                    }
                }
                DialogKey::Run => {
                    if let Some((row, _)) = at.and_then(|at| rows.get(at)) {
                        let row = row.clone();
                        drop(rows);
                        launch(&row);
                    }
                }
            }
            Propagation::Stop
        }
    });
    dialog.add_controller(steer);
    dialog.set_focus(Some(&search));
    dialog.present(Some(&window));
}

/// Scrolls the dialog, when it scrolls, until `row` is in view; the focus
/// stays in the search.
fn reveal(row: &gtk::ListBoxRow) {
    let Some(scroller) = row
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
    else {
        return;
    };
    let Some(point) = row.compute_point(&scroller, &gtk::graphene::Point::new(0.0, 0.0)) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let top = adjustment.value() + f64::from(point.y());
    adjustment.clamp_page(top, top + f64::from(row.height()));
}

/// Fills `list` with `found`, each section under its title while the list
/// is whole (a search ranks across them), and gives the rows that run
/// something.
fn fill(
    list: &gtk::ListBox,
    found: &[&'static Binding],
    whole: bool,
) -> Vec<(gtk::ListBoxRow, Command)> {
    list.remove_all();
    let mut rows = Vec::new();
    let mut section = None;
    for binding in found {
        if whole && section != Some(binding.section) {
            section = Some(binding.section);
            let title = gtk::ListBoxRow::builder()
                .child(&tracked(binding.section.title(), &["dialog-heading"]))
                .activatable(false)
                .selectable(false)
                .focusable(false)
                .build();
            title.add_css_class("keys-section");
            list.append(&title);
        }
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let label = gtk::Label::builder()
            .label(binding.label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        line.append(&label);
        let keys: Vec<String> = binding.chords.iter().map(|chord| chord.text()).collect();
        for key in &keys {
            let cap = gtk::Label::new(Some(key));
            cap.add_css_class("keycap");
            line.append(&cap);
        }
        let row = gtk::ListBoxRow::builder().child(&line).build();
        row.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}: {}",
            binding.label,
            keys.join(" or ")
        ))]);
        list.append(&row);
        rows.push((row, binding.command));
    }
    rows
}
