//! Settings' pages over the main one: a source's, one of its stacks', and
//! the kinds a new source can be.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use pending_core::config::{AppConfig, SourceConfig, SourceKind, StackSort};
use pending_core::schema::{self, Field, Input, KINDS, Kind, Var};
use toml::Value;

use super::board::clear;
use super::dialogs::{flow_child, pill};
use super::icons::{size, source_icon};
use super::settings::{Open, enabled, heading, page, say, switch};
use super::wrapped as text;
use crate::present::Choice;
use crate::settings::{
    REFRESH_CHOICES, SourceForm, StackForm, TIMEOUT_CHOICES, duration_label, durations, neighbour,
    new_source, stack_summary, text_value, word_label, words_value,
};

/// The page of a source: which source it is (its place in the file, which
/// moves with it), and what it holds.
#[derive(Clone)]
pub struct SourcePage {
    pub index: Rc<Cell<usize>>,
    content: gtk::Box,
    page: adw::NavigationPage,
}

/// Opens the page of the source at `index`.
pub fn push(open: &Rc<Open>, index: usize) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let shown = SourcePage {
        index: Rc::new(Cell::new(index)),
        page: page("Source", "source", &content),
        content,
    };
    open.source.replace(Some(shown.clone()));
    fill(open, &shown);
    open.nav.push(&shown.page);
}

/// Back to the main page, the source's being gone.
fn back(open: &Rc<Open>) {
    open.source.take();
    open.nav.pop_to_tag("main");
}

/// Fills the source's page from the config as it is now.
pub fn fill(open: &Rc<Open>, shown: &SourcePage) {
    let index = shown.index.get();
    let Some(Ok(config)) = open.config() else {
        return back(open);
    };
    let Some(source) = config.sources.get(index).cloned() else {
        return back(open);
    };
    let content = &shown.content;
    let kind = schema::kind(source.kind);
    shown.page.set_title(&source.name);
    clear(content);
    content.append(&text(
        &format!("{}: {}", kind.label, kind.about),
        &["dialog-note"],
    ));
    append_form(open, content, &config, &source, index);
    if !kind.vars.is_empty() {
        append_credentials(open, content, kind);
    }
    append_stacks(open, content, &source, kind, index);
    append_order(open, shown, &config, &source, index);
}

/// The source's own keys, written together by the Save button.
fn append_form(
    open: &Rc<Open>,
    content: &gtk::Box,
    config: &AppConfig,
    source: &SourceConfig,
    index: usize,
) {
    let form = Rc::new(RefCell::new(SourceForm::of(source)));
    content.append(&heading("Name"));
    content.append(&entry(&source.name, "", {
        let form = form.clone();
        move |typed| form.borrow_mut().name = typed.to_owned()
    }));
    content.append(&heading("Board"));
    content.append(&entry(&source.board, "", {
        let form = form.clone();
        move |typed| form.borrow_mut().board = typed.to_owned()
    }));
    content.append(&text(
        "The group is labelled with it, and Settings can show one board alone; a new name \
         makes a new board.",
        &["dialog-note"],
    ));

    let own = |choices: &[u64], current: Option<u64>, global: u64| {
        let mut all = vec![Choice {
            label: format!("As the others ({})", duration_label(global)),
            value: None,
            active: current.is_none(),
        }];
        all.extend(
            durations(choices, current.unwrap_or(global))
                .into_iter()
                .map(|secs| Choice {
                    label: duration_label(secs),
                    value: Some(secs),
                    active: current == Some(secs),
                }),
        );
        all
    };
    content.append(&heading("Refresh every"));
    content.append(&radio_pills(
        own(
            &REFRESH_CHOICES,
            source.refresh_seconds,
            config.refresh_seconds,
        ),
        {
            let form = form.clone();
            move |secs| form.borrow_mut().refresh_seconds = secs
        },
    ));
    content.append(&heading("Give up on a refresh after"));
    content.append(&radio_pills(
        own(
            &TIMEOUT_CHOICES,
            source.timeout_seconds,
            config.timeout_seconds,
        ),
        {
            let form = form.clone();
            move |secs| form.borrow_mut().timeout_seconds = secs
        },
    ));

    content.append(&heading("Icon"));
    let icon = source.icon.clone().unwrap_or_default();
    content.append(&entry(&icon, "The kind's logo", {
        let form = form.clone();
        move |typed| form.borrow_mut().icon = typed.to_owned()
    }));
    let dark = source.icon_dark.clone().unwrap_or_default();
    content.append(&entry(&dark, "The same on dark themes", {
        let form = form.clone();
        move |typed| form.borrow_mut().icon_dark = typed.to_owned()
    }));
    content.append(&text(
        "An image's address, such as one from dashboardicons.com.",
        &["dialog-note"],
    ));

    let save = primary("Save");
    save.set_halign(gtk::Align::End);
    {
        let open = Rc::downgrade(open);
        let content = content.downgrade();
        let source = source.clone();
        save.connect_clicked(move |_| {
            let (Some(open), Some(content)) = (open.upgrade(), content.upgrade()) else {
                return;
            };
            let changes = match form.borrow().changes(&source) {
                Ok(changes) => changes,
                Err(why) => return say(&content, &format!("Not saved: {why}.")),
            };
            if changes.is_empty() {
                return;
            }
            open.changed(&content, |doc| {
                for (key, value) in changes {
                    doc.set_source(index, key, value)?;
                }
                Ok(())
            });
        });
    }
    content.append(&save);
}

/// The kind's variables in the env file: whether each is set, a secret's
/// value never shown.
fn append_credentials(open: &Rc<Open>, content: &gtk::Box, kind: &'static Kind) {
    content.append(&heading("Credentials"));
    let env = open.env.borrow();
    let Some(env) = env.as_ref() else {
        content.append(&text(
            "The dashboard has not said which env file it reads, so its credentials cannot be \
             set here.",
            &["dialog-note"],
        ));
        return;
    };
    if let Some(why) = env.unread() {
        content.append(&text(why, &["dialog-note"]));
        return;
    }
    let place = match env.place().path() {
        Some(path) => format!(
            "In {}, which only you can read, and shared by every {} source. The dashboard reads \
             it again within seconds.",
            path.display(),
            kind.label
        ),
        None => "The sandbox keeps its env file in memory: nothing is written.".to_owned(),
    };
    for var in kind.vars {
        let set_as = env.set_as(var);
        content.append(&field_label(var.label));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let typed: gtk::Editable = if var.secret {
            let hint = match set_as {
                Some(name) if name != var.names[0] => format!("Set, as {name}: type to replace"),
                Some(_) => "Set: type to replace".to_owned(),
                None => "Not set".to_owned(),
            };
            gtk::PasswordEntry::builder()
                .show_peek_icon(true)
                .placeholder_text(hint)
                .hexpand(true)
                .build()
                .upcast()
        } else {
            gtk::Entry::builder()
                .text(env.value(var).unwrap_or_default())
                .placeholder_text("Not set")
                .hexpand(true)
                .build()
                .upcast()
        };
        line.append(&typed);
        let save = button("Save");
        save.set_sensitive(false);
        {
            let save = save.downgrade();
            let was = typed.text();
            let secret = var.secret;
            typed.connect_changed(move |typed| {
                if let Some(save) = save.upgrade() {
                    let now = typed.text();
                    save.set_sensitive(if secret { !now.is_empty() } else { now != was });
                }
            });
        }
        let write = {
            let open = Rc::downgrade(open);
            let content = content.downgrade();
            let typed = typed.downgrade();
            move || {
                let (Some(open), Some(content), Some(typed)) =
                    (open.upgrade(), content.upgrade(), typed.upgrade())
                else {
                    return;
                };
                let value = typed.text();
                if var.secret && value.is_empty() {
                    return;
                }
                if open.set_var(&content, var, Some(&value)) {
                    open.fill();
                }
            }
        };
        let write = Rc::new(write);
        {
            let write = write.clone();
            save.connect_clicked(move |_| write());
        }
        if let Some(entry) = typed.downcast_ref::<gtk::Entry>() {
            let write = write.clone();
            entry.connect_activate(move |_| write());
        } else if let Some(entry) = typed.downcast_ref::<gtk::PasswordEntry>() {
            let write = write.clone();
            entry.connect_activate(move |_| write());
        }
        line.append(&save);
        if var.secret && set_as.is_some() {
            line.append(&remove_var(open, content, var));
        }
        content.append(&line);
        content.append(&text(var.help, &["dialog-note"]));
    }
    content.append(&text(&place, &["dialog-note"]));
}

/// The button that takes `var` out of the env file.
fn remove_var(open: &Rc<Open>, content: &gtk::Box, var: &'static Var) -> gtk::Button {
    let remove = button("Remove");
    remove.set_tooltip_text(Some(&format!("Take the {} out of the env file", var.label)));
    let open = Rc::downgrade(open);
    let content = content.downgrade();
    remove.connect_clicked(move |_| {
        if let (Some(open), Some(content)) = (open.upgrade(), content.upgrade())
            && open.set_var(&content, var, None)
        {
            open.fill();
        }
    });
    remove
}

/// The source's stacks, each with its switch and the way to its page.
fn append_stacks(
    open: &Rc<Open>,
    content: &gtk::Box,
    source: &SourceConfig,
    kind: &'static Kind,
    index: usize,
) {
    content.append(&heading("Stacks"));
    if source.kind == SourceKind::Sample {
        content.append(&text(
            "The sample source has fixed stacks.",
            &["dialog-note"],
        ));
        return;
    }
    if source.stacks.is_empty() {
        let shown = shown_stacks(open, &source.name);
        let defaults = if shown.is_empty() {
            String::new()
        } else {
            format!(" ({})", shown.join(", "))
        };
        content.append(&text(
            &format!(
                "No stacks of its own: it shows its kind's{defaults}. A stack added here shows \
                 in their place."
            ),
            &["dialog-note"],
        ));
    }
    for (at, stack) in source.stacks.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("row-card");
        row.add_css_class("source-row");
        if !stack.enabled {
            row.add_css_class("off");
        }
        let texts = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        texts.append(&text(&stack.name, &["card-title"]));
        texts.append(&text(&stack_summary(kind, stack), &["row-time"]));
        row.append(&texts);
        let on = switch(stack.enabled, &format!("Show {}", stack.name));
        {
            let open = Rc::downgrade(open);
            let content = content.downgrade();
            on.connect_state_set(move |_, on| {
                if let (Some(open), Some(content)) = (open.upgrade(), content.upgrade()) {
                    open.changed(&content, |doc| {
                        doc.set_stack(index, at, "enabled", enabled(on))
                    });
                }
                gtk::glib::Propagation::Proceed
            });
        }
        row.append(&on);
        let edit = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text(format!("Change {}", stack.name))
            .build();
        edit.add_css_class("control");
        edit.add_css_class("wake");
        {
            let open = Rc::downgrade(open);
            edit.connect_clicked(move |_| {
                if let Some(open) = open.upgrade() {
                    push_stack(&open, index, Some(at));
                }
            });
        }
        row.append(&edit);
        content.append(&row);
    }
    let add = button("Add a stack…");
    add.set_halign(gtk::Align::Start);
    add.set_margin_top(4);
    {
        let open = Rc::downgrade(open);
        add.connect_clicked(move |_| {
            if let Some(open) = open.upgrade() {
                push_stack(&open, index, None);
            }
        });
    }
    content.append(&add);
}

/// The stacks the board shows for the source named `name`.
fn shown_stacks(open: &Open, name: &str) -> Vec<String> {
    let model = open.ctx.model.borrow();
    let Some(snapshot) = model.state.snapshot() else {
        return Vec::new();
    };
    snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .filter(|group| group.source == name)
        .flat_map(|group| group.columns.iter().map(|column| column.name.clone()))
        .collect()
}

/// Moving the source among its board's, and taking it out.
fn append_order(
    open: &Rc<Open>,
    shown: &SourcePage,
    config: &AppConfig,
    source: &SourceConfig,
    index: usize,
) {
    let content = &shown.content;
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.set_margin_top(16);
    for (label, up) in [("Move up", true), ("Move down", false)] {
        let to = neighbour(config, index, up);
        let moves = button(label);
        moves.set_sensitive(to.is_some());
        let open = Rc::downgrade(open);
        let content = content.downgrade();
        let place = shown.index.clone();
        moves.connect_clicked(move |_| {
            let (Some(open), Some(content), Some(to)) = (open.upgrade(), content.upgrade(), to)
            else {
                return;
            };
            if open
                .change(&content, |doc| doc.move_source(index, to))
                .is_some()
            {
                place.set(to);
                open.fill();
            }
        });
        line.append(&moves);
    }
    let spacer = gtk::Box::builder().hexpand(true).build();
    line.append(&spacer);
    let remove = button("Remove…");
    remove.add_css_class("destructive");
    {
        let open = Rc::downgrade(open);
        let content = content.downgrade();
        let name = source.name.clone();
        remove.connect_clicked(move |_| {
            let Some(open) = open.upgrade() else {
                return;
            };
            let confirmed = {
                let open = Rc::downgrade(&open);
                let content = content.clone();
                move || {
                    if let (Some(open), Some(content)) = (open.upgrade(), content.upgrade())
                        && open
                            .change(&content, |doc| doc.remove_source(index))
                            .is_some()
                    {
                        back(&open);
                        open.fill();
                    }
                }
            };
            confirm(
                &open,
                &format!("Remove {name}?"),
                "Its stacks go with it, and its cards leave the board. Its credentials stay in \
                 the env file.",
                confirmed,
            );
        });
    }
    line.append(&remove);
    content.append(&line);
}

/// The kinds a new source can be; choosing one adds it, named after its
/// kind and on its board, and opens its page.
pub fn push_kinds(open: &Rc<Open>) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&text(
        "It starts with its kind's stacks; its page then asks for its credentials.",
        &["dialog-note"],
    ));
    for kind in KINDS.iter().filter(|kind| kind.kind != SourceKind::Sample) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&source_icon(
            &open.ctx,
            kind.kind.default_icon().as_ref(),
            size::LOGO,
        ));
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        texts.append(&text(kind.label, &["card-title"]));
        texts.append(&text(kind.about, &["row-body"]));
        row.append(&texts);
        let choose = gtk::Button::builder().child(&row).build();
        choose.add_css_class("kind-choice");
        {
            let open = Rc::downgrade(open);
            let content = content.downgrade();
            choose.connect_clicked(move |_| {
                let (Some(open), Some(content)) = (open.upgrade(), content.upgrade()) else {
                    return;
                };
                let Some(Ok(config)) = open.config() else {
                    return;
                };
                let fields = new_source(&config, kind);
                if let Some(index) = open.change(&content, |doc| doc.add_source(&fields)) {
                    open.nav.pop_to_tag("main");
                    open.fill();
                    push(&open, index);
                }
            });
        }
        content.append(&choose);
    }
    open.nav.push(&page("Add a source", "kinds", &content));
}

/// Opens the page of the source's stack at `stack`, or of a new one.
pub(super) fn push_stack(open: &Rc<Open>, index: usize, stack: Option<usize>) {
    let Some(Ok(config)) = open.config() else {
        return;
    };
    let Some(source) = config.sources.get(index) else {
        return;
    };
    let kind = schema::kind(source.kind);
    let existing = stack.and_then(|at| source.stacks.get(at));
    let form = Rc::new(RefCell::new(match existing {
        Some(existing) => StackForm::of(kind, existing),
        None => StackForm::new(kind),
    }));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let title = existing.map_or("New stack", |existing| existing.name.as_str());

    content.append(&heading("Name"));
    content.append(&entry(&form.borrow().name, "As the board shows it", {
        let form = form.clone();
        move |typed| form.borrow_mut().name = typed.to_owned()
    }));
    let on = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    on.set_margin_top(8);
    let label = text("On: asked for, and shown", &[]);
    label.set_hexpand(true);
    on.append(&label);
    let toggle = switch(form.borrow().enabled, "Off: not asked for, and not shown");
    {
        let form = form.clone();
        toggle.connect_state_set(move |_, on| {
            form.borrow_mut().enabled = on;
            gtk::glib::Propagation::Proceed
        });
    }
    on.append(&toggle);
    content.append(&on);

    if !kind.modes.is_empty() {
        content.append(&heading("Shows"));
        let area = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let mode = form.borrow().mode;
        let choices = kind
            .modes
            .iter()
            .map(|field| Choice {
                label: field.label.to_owned(),
                value: field.key,
                active: mode == Some(field.key),
            })
            .collect();
        content.append(&radio_pills(choices, {
            let form = form.clone();
            let area = area.downgrade();
            move |key| {
                form.borrow_mut().mode = Some(key);
                if let Some(area) = area.upgrade() {
                    fill_mode(&area, &form, kind);
                }
            }
        }));
        fill_mode(&area, &form, kind);
        content.append(&area);
    }
    for field in kind.fields {
        content.append(&field_input(&form, field, false));
    }
    content.append(&heading("Order"));
    let sort = form.borrow().sort;
    content.append(&radio_pills(
        [
            (StackSort::Newest, "Newest first"),
            (StackSort::Oldest, "Oldest first"),
        ]
        .into_iter()
        .map(|(value, label)| Choice {
            label: label.to_owned(),
            value,
            active: sort == value,
        })
        .collect(),
        {
            let form = form.clone();
            move |sort| form.borrow_mut().sort = sort
        },
    ));
    content.append(&text(
        "Among cards of the same severity and due time, by when they last changed.",
        &["dialog-note"],
    ));
    content.append(&heading("Leave out"));
    let exclude = form.borrow().exclude.clone();
    content.append(&words_editor(exclude, "A pattern", {
        let form = form.clone();
        move |words| form.borrow_mut().exclude = words
    }));
    content.append(&text(
        "Cards whose title or text matches one of these regular expressions, in any case.",
        &["dialog-note"],
    ));

    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    line.set_margin_top(16);
    if let Some(at) = stack {
        let count = source.stacks.len();
        for (label, to) in [
            ("Move up", at.checked_sub(1)),
            ("Move down", Some(at + 1).filter(|to| *to < count)),
        ] {
            let moves = button(label);
            moves.set_sensitive(to.is_some());
            let open = Rc::downgrade(open);
            let content = content.downgrade();
            moves.connect_clicked(move |_| {
                let (Some(open), Some(content), Some(to)) = (open.upgrade(), content.upgrade(), to)
                else {
                    return;
                };
                if open
                    .changed(&content, |doc| doc.move_stack(index, at, to))
                    .is_some()
                {
                    open.nav.pop();
                }
            });
            line.append(&moves);
        }
        let remove = button("Remove…");
        remove.add_css_class("destructive");
        let open_weak = Rc::downgrade(open);
        let content_for_remove = content.downgrade();
        let name = title.to_owned();
        remove.connect_clicked(move |_| {
            let Some(open) = open_weak.upgrade() else {
                return;
            };
            let confirmed = {
                let open = Rc::downgrade(&open);
                let content = content_for_remove.clone();
                move || {
                    if let (Some(open), Some(content)) = (open.upgrade(), content.upgrade())
                        && open
                            .changed(&content, |doc| doc.remove_stack(index, at))
                            .is_some()
                    {
                        open.nav.pop();
                    }
                }
            };
            confirm(
                &open,
                &format!("Remove {name}?"),
                "Its cards leave the board; the source's other stacks stay.",
                confirmed,
            );
        });
        line.append(&remove);
    }
    let spacer = gtk::Box::builder().hexpand(true).build();
    line.append(&spacer);
    let save = primary(if stack.is_some() { "Save" } else { "Add" });
    {
        let open = Rc::downgrade(open);
        let content = content.downgrade();
        save.connect_clicked(move |_| {
            let (Some(open), Some(content)) = (open.upgrade(), content.upgrade()) else {
                return;
            };
            let fields = match form.borrow().fields(kind) {
                Ok(fields) => fields,
                Err(why) => return say(&content, &format!("Not saved: {why}.")),
            };
            let written = match stack {
                Some(at) => open
                    .changed(&content, |doc| doc.replace_stack(index, at, &fields))
                    .is_some(),
                None => open
                    .changed(&content, |doc| doc.add_stack(index, &fields).map(drop))
                    .is_some(),
            };
            if written {
                open.nav.pop();
            }
        });
    }
    line.append(&save);
    content.append(&line);
    open.nav.push(&page(title, "stack", &content));
}

/// The input of the mode the stack shows, in `area`.
fn fill_mode(area: &gtk::Box, form: &Rc<RefCell<StackForm>>, kind: &'static Kind) {
    clear(area);
    let mode = form.borrow().mode;
    let Some(field) = kind.modes.iter().find(|field| Some(field.key) == mode) else {
        return;
    };
    if field.input == Input::Toggle {
        area.append(&text(field.help, &["dialog-note"]));
    } else {
        area.append(&field_input(form, field, true));
    }
}

/// A key of the stack: its name, its input, and what it means. A `mode`
/// is named by the pills above it, and always has a value.
fn field_input(form: &Rc<RefCell<StackForm>>, field: &'static Field, mode: bool) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 8);
    if !mode {
        block.append(&heading(field.label));
    }
    let key = field.key;
    let set = {
        let form = form.clone();
        move |value: Option<Value>| form.borrow_mut().set(key, value)
    };
    match field.input {
        Input::Text => {
            let typed = form.borrow().text(key);
            block.append(&entry(&typed, "", move |typed| set(text_value(typed))));
        }
        Input::Choice(words) => {
            let current = form.borrow().text(key);
            let mut choices = Vec::new();
            if !field.required && !mode {
                choices.push(Choice {
                    label: "Not set".to_owned(),
                    value: None,
                    active: current.is_empty(),
                });
            }
            choices.extend(words.iter().map(|word| Choice {
                label: word_label(word),
                value: Some(*word),
                active: current == *word,
            }));
            block.append(&radio_pills(choices, move |word| {
                set(word.map(|word| Value::String(word.to_owned())));
            }));
        }
        Input::Choices(words) => {
            let picked = form.borrow().words(key);
            block.append(&many_pills(words, &picked, move |picked| {
                set(words_value(&picked));
            }));
        }
        Input::Words => {
            let words = form.borrow().words(key);
            block.append(&words_editor(words, "Add one", move |words| {
                set(words_value(&words));
            }));
        }
        Input::Toggle => {
            let current = form.borrow().toggle(key);
            let choices = [(None, "Either"), (Some(true), "Yes"), (Some(false), "No")]
                .into_iter()
                .map(|(value, label)| Choice {
                    label: label.to_owned(),
                    value,
                    active: current == value,
                })
                .collect();
            block.append(&radio_pills(choices, move |on| set(on.map(Value::Boolean))));
        }
    }
    block.append(&text(field.help, &["dialog-note"]));
    block
}

/// An entry holding `typed`, telling `changed` of each change.
fn entry(typed: &str, placeholder: &str, changed: impl Fn(&str) + 'static) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .text(typed)
        .placeholder_text(placeholder)
        .build();
    entry.connect_changed(move |entry| changed(&entry.text()));
    entry
}

/// The label over an input that is not a section of its own.
fn field_label(label: &str) -> gtk::Label {
    let label = text(label, &["field-label"]);
    label.set_margin_top(8);
    label
}

fn button(label: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.add_css_class("control");
    button.add_css_class("wake");
    button
}

fn primary(label: &str) -> gtk::Button {
    let button = button(label);
    button.add_css_class("primary");
    button
}

/// Pills of which one is on; choosing one calls `choose` with it.
fn radio_pills<T: Clone + 'static>(
    choices: Vec<Choice<T>>,
    choose: impl Fn(T) + 'static,
) -> gtk::FlowBox {
    let row = flow();
    let choose = Rc::new(choose);
    let mut first: Option<gtk::ToggleButton> = None;
    for choice in choices {
        let button = pill(&choice.label, choice.active);
        match &first {
            Some(first) => button.set_group(Some(first)),
            None => first = Some(button.clone()),
        }
        let choose = choose.clone();
        let value = choice.value;
        button.connect_toggled(move |button| {
            if button.is_active() {
                choose(value.clone());
            }
        });
        row.insert(&flow_child(&button), -1);
    }
    row
}

/// Pills that are on or off each; `changed` is told which words are on,
/// in the order of `words`.
fn many_pills(
    words: &'static [&'static str],
    picked: &[String],
    changed: impl Fn(Vec<String>) + 'static,
) -> gtk::FlowBox {
    let row = flow();
    let on: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(
        words
            .iter()
            .map(|word| picked.iter().any(|picked| picked == word))
            .collect(),
    ));
    let changed = Rc::new(changed);
    for (at, word) in words.iter().enumerate() {
        let button = pill(&word_label(word), on.borrow()[at]);
        let on = on.clone();
        let changed = changed.clone();
        button.connect_toggled(move |button| {
            on.borrow_mut()[at] = button.is_active();
            let picked = words
                .iter()
                .zip(on.borrow().iter())
                .filter(|(_, on)| **on)
                .map(|(word, _)| (*word).to_owned())
                .collect();
            changed(picked);
        });
        row.insert(&flow_child(&button), -1);
    }
    row
}

/// Words, each with a button that takes it out, and an entry that adds
/// one; `changed` is told every word, the one being typed too, so a
/// word typed and not yet added is not lost.
fn words_editor(
    words: Vec<String>,
    placeholder: &str,
    changed: impl Fn(Vec<String>) + 'static,
) -> gtk::Box {
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let chips = flow();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let typed = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    let add = button("Add");
    line.append(&typed);
    line.append(&add);
    editor.append(&chips);
    editor.append(&line);

    struct Words {
        words: RefCell<Vec<String>>,
        chips: gtk::glib::WeakRef<gtk::FlowBox>,
        typed: gtk::glib::WeakRef<gtk::Entry>,
        changed: Box<dyn Fn(Vec<String>)>,
    }
    impl Words {
        fn typed(&self) -> String {
            self.typed
                .upgrade()
                .map(|typed| typed.text().trim().to_owned())
                .unwrap_or_default()
        }
        fn tell(&self) {
            let mut all = self.words.borrow().clone();
            let typed = self.typed();
            if !typed.is_empty() && !all.contains(&typed) {
                all.push(typed);
            }
            (self.changed)(all);
        }
        fn draw(self: &Rc<Self>) {
            let Some(chips) = self.chips.upgrade() else {
                return;
            };
            while let Some(child) = chips.first_child() {
                chips.remove(&child);
            }
            for (at, word) in self.words.borrow().iter().enumerate() {
                let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                chip.add_css_class("chip");
                chip.append(&gtk::Label::new(Some(word)));
                let drop = gtk::Button::builder()
                    .icon_name("window-close-symbolic")
                    .tooltip_text(format!("Take {word} out"))
                    .build();
                drop.add_css_class("flat");
                drop.add_css_class("chip-remove");
                let words = self.clone();
                drop.connect_clicked(move |_| {
                    words.words.borrow_mut().remove(at);
                    words.draw();
                    words.tell();
                });
                chip.append(&drop);
                chips.insert(&flow_child(&chip), -1);
            }
            chips.set_visible(!self.words.borrow().is_empty());
        }
        fn add(self: &Rc<Self>) {
            let typed = self.typed();
            if typed.is_empty() {
                return;
            }
            if !self.words.borrow().contains(&typed) {
                self.words.borrow_mut().push(typed);
            }
            if let Some(entry) = self.typed.upgrade() {
                entry.set_text("");
            }
            self.draw();
            self.tell();
        }
    }

    let state = Rc::new(Words {
        words: RefCell::new(words),
        chips: chips.downgrade(),
        typed: typed.downgrade(),
        changed: Box::new(changed),
    });
    state.draw();
    {
        let state = state.clone();
        typed.connect_changed(move |_| state.tell());
    }
    {
        let state = state.clone();
        typed.connect_activate(move |_| state.add());
    }
    add.connect_clicked(move |_| state.add());
    editor
}

fn flow() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(6)
        .row_spacing(6)
        .max_children_per_line(10)
        .build()
}

/// Asks whether to go ahead with `go`, which cannot be undone here.
fn confirm(open: &Rc<Open>, title: &str, body: &str, go: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.add_css_class("tasks-pending");
    dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "remove" {
            go();
        }
    });
    dialog.present(Some(&open.nav));
}
