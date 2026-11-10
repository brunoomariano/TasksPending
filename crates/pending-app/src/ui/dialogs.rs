//! The dialogs: the snoozed cards and a todo, and what Settings and the
//! keys dialog build theirs with.

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use super::board::{self, tracked};
use super::icons::{size, source_icon};
use super::{Ctx, wrapped as text};
use crate::backend::Command;
use crate::present::{todo_dialog_words, todo_note};

/// Every dialog's width: enough for a source's message or a stack's name
/// on one line, and the same for all three so they open in one place.
pub(super) const DIALOG_WIDTH: i32 = 600;

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

fn heading(label: &str) -> gtk::Label {
    tracked(label, &["dialog-heading"])
}

/// A pill in Settings, on when `active`.
pub(super) fn pill(label: &str, active: bool) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .label(label)
        .active(active)
        .build();
    button.add_css_class("filter");
    button
}

/// `widget` in a row of pills, which leaves the focus to the widget.
pub(super) fn flow_child(widget: &impl IsA<gtk::Widget>) -> gtk::FlowBoxChild {
    gtk::FlowBoxChild::builder()
        .child(widget)
        .focusable(false)
        .build()
}
