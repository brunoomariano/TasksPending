//! The window's keys: one table that the key handler, the keys dialog and
//! the hints on buttons all read, so each key is named once. The mouse
//! keeps doing everything it did; a key is another way to the same thing.

use gtk4::gdk::{Key, ModifierType};

/// Something a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    ShowKeys,
    NewTodo,
    Refresh,
    Sources,
    Settings,
    ActivityTab,
    BoardTab,
    SendAway,
    Quit,
}

/// Where the keys dialog lists a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Window,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Window => "Window",
        }
    }
}

/// A key with the modifiers held with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub mods: ModifierType,
}

const NONE: ModifierType = ModifierType::empty();
const CTRL: ModifierType = ModifierType::CONTROL_MASK;
const SHIFT: ModifierType = ModifierType::SHIFT_MASK;
const ALT: ModifierType = ModifierType::ALT_MASK;
const SUPER: ModifierType = ModifierType::SUPER_MASK;

const fn chord(key: Key, mods: ModifierType) -> Chord {
    Chord { key, mods }
}

/// A command, what the keys dialog calls it, and its keys, the first being
/// the one hints name.
#[derive(Debug)]
pub struct Binding {
    pub command: Command,
    pub label: &'static str,
    pub section: Section,
    pub chords: &'static [Chord],
    /// Another handler takes the key (Esc on the way back up from the
    /// focused widget, Ctrl+Q as the app's accelerator): the dialog lists
    /// and runs it, and the key handler leaves it alone.
    pub elsewhere: bool,
}

const fn bind(
    command: Command,
    label: &'static str,
    section: Section,
    chords: &'static [Chord],
) -> Binding {
    Binding {
        command,
        label,
        section,
        chords,
        elsewhere: false,
    }
}

/// Every key, in the order the keys dialog lists them.
pub const BINDINGS: &[Binding] = &[
    bind(
        Command::ShowKeys,
        "Show the keys",
        Section::Window,
        &[chord(Key::question, NONE), chord(Key::k, CTRL)],
    ),
    bind(
        Command::NewTodo,
        "Write down a todo",
        Section::Window,
        &[chord(Key::n, NONE)],
    ),
    bind(
        Command::Refresh,
        "Refresh the sources",
        Section::Window,
        &[chord(Key::r, NONE), chord(Key::F5, NONE)],
    ),
    bind(
        Command::Sources,
        "Show the sources",
        Section::Window,
        &[chord(Key::s, NONE)],
    ),
    bind(
        Command::Settings,
        "Open Settings",
        Section::Window,
        &[chord(Key::comma, CTRL)],
    ),
    bind(
        Command::ActivityTab,
        "Go to the Activity",
        Section::Window,
        &[chord(Key::_1, NONE)],
    ),
    bind(
        Command::BoardTab,
        "Go to the Board",
        Section::Window,
        &[chord(Key::_2, NONE)],
    ),
    Binding {
        elsewhere: true,
        ..bind(
            Command::SendAway,
            "Send the drop-down away",
            Section::Window,
            &[chord(Key::Escape, NONE)],
        )
    },
    Binding {
        elsewhere: true,
        ..bind(
            Command::Quit,
            "Quit TasksPending",
            Section::Window,
            &[chord(Key::q, CTRL)],
        )
    },
];

/// The binding of `command`.
pub fn binding(command: Command) -> &'static Binding {
    BINDINGS
        .iter()
        .find(|binding| binding.command == command)
        .expect("every command has a binding")
}

/// What `key` with `mods` held does in the window, if the key handler
/// takes it.
pub fn lookup(key: Key, mods: ModifierType) -> Option<Command> {
    BINDINGS
        .iter()
        .filter(|binding| !binding.elsewhere)
        .find(|binding| binding.chords.iter().any(|c| c.matches(key, mods)))
        .map(|binding| binding.command)
}

/// Whether `key` gives a character, which then carries Shift itself: `?`
/// is Shift+/ and `G` Shift+g.
fn printable(key: Key) -> bool {
    key.to_unicode().is_some_and(|c| !c.is_control())
}

impl Chord {
    /// Whether `key` pressed with `mods` is this chord. Lock keys and
    /// mouse buttons held do not count, nor Shift on a key that gives a
    /// character; with Ctrl or Alt, a letter counts in either case.
    pub fn matches(&self, key: Key, mods: ModifierType) -> bool {
        let mut mods = mods & (CTRL | SHIFT | ALT | SUPER);
        let mut key = key;
        if printable(key) {
            mods.remove(SHIFT);
            if mods.intersects(CTRL | ALT) {
                key = key.to_lower();
            }
        }
        key == self.key && mods == self.mods
    }

    /// The chord as a person reads it: `Ctrl+K`, `?`, `F5`, `Shift+→`.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for (mask, name) in [
            (CTRL, "Ctrl+"),
            (ALT, "Alt+"),
            (SUPER, "Super+"),
            (SHIFT, "Shift+"),
        ] {
            if self.mods.contains(mask) {
                text.push_str(name);
            }
        }
        let held = !self.mods.is_empty();
        let name = match self.key {
            Key::Escape => "Esc".to_owned(),
            Key::Return => "Enter".to_owned(),
            Key::Delete => "Delete".to_owned(),
            Key::Up => "↑".to_owned(),
            Key::Down => "↓".to_owned(),
            Key::Left => "←".to_owned(),
            Key::Right => "→".to_owned(),
            key => match key.to_unicode().filter(|c| !c.is_control()) {
                Some(c) if held => c.to_uppercase().collect(),
                Some(c) => c.to_string(),
                None => key.name().map(|name| name.to_string()).unwrap_or_default(),
            },
        };
        text.push_str(&name);
        text
    }
}

/// `label` with the first key of `command` after it, for a tooltip:
/// `Refresh (r)`, or `Settings (showing Work; Ctrl+,)` when the label ends
/// in brackets already.
pub fn hinted(label: &str, command: Command) -> String {
    let Some(chord) = binding(command).chords.first() else {
        return label.to_owned();
    };
    match label.strip_suffix(')') {
        Some(open) if open.contains('(') => format!("{open}; {})", chord.text()),
        _ => format!("{label} ({})", chord.text()),
    }
}

/// The bindings the keys dialog shows for `query`, best first: all of
/// them, in order, for an empty one. A label that is the query comes
/// first, then one that starts with it, holds it, or holds each of its
/// words; then a binding whose key the query names (`ctrl k`, `f5`).
/// Ties keep the table's order.
pub fn search(query: &str) -> Vec<&'static Binding> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return BINDINGS.iter().collect();
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    let squeezed: String = words.concat();
    let rank = |binding: &Binding| {
        let label = binding.label.to_lowercase();
        if label == query {
            Some(0)
        } else if label.starts_with(&query) {
            Some(1)
        } else if label.contains(&query) {
            Some(2)
        } else if words.iter().all(|word| label.contains(word)) {
            Some(3)
        } else if binding.chords.iter().any(|chord| {
            let text = chord.text().to_lowercase();
            text == query || text.replace('+', "") == squeezed
        }) {
            Some(4)
        } else {
            None
        }
    };
    let mut found: Vec<(usize, &Binding)> = BINDINGS
        .iter()
        .filter_map(|binding| rank(binding).map(|rank| (rank, binding)))
        .collect();
    found.sort_by_key(|(rank, _)| *rank);
    found.into_iter().map(|(_, binding)| binding).collect()
}

/// What a key does in the keys dialog, whose search entry has the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKey {
    Close,
    Previous,
    Next,
    Run,
}

/// What `key` with `mods` does in the keys dialog; `None` leaves it to the
/// search entry. Backspace closes it only once the query is empty.
pub fn dialog_key(key: Key, mods: ModifierType, query_empty: bool) -> Option<DialogKey> {
    let mods = mods & (CTRL | SHIFT | ALT | SUPER);
    match key {
        Key::Escape => Some(DialogKey::Close),
        Key::BackSpace if query_empty && mods.is_empty() => Some(DialogKey::Close),
        Key::Up | Key::KP_Up => Some(DialogKey::Previous),
        Key::Down | Key::KP_Down => Some(DialogKey::Next),
        Key::Return | Key::KP_Enter | Key::ISO_Enter => Some(DialogKey::Run),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Each command has one binding, a label of its own, and keys that no
    /// other command takes; the key handler leaves Esc and Ctrl+Q alone.
    #[test]
    fn every_key_does_one_thing() {
        let commands: HashSet<_> = BINDINGS.iter().map(|b| b.command).collect();
        assert_eq!(commands.len(), BINDINGS.len());
        let labels: HashSet<_> = BINDINGS.iter().map(|b| b.label).collect();
        assert_eq!(labels.len(), BINDINGS.len());
        let mut chords = Vec::new();
        for binding in BINDINGS {
            assert!(
                !binding.chords.is_empty(),
                "{:?} has a key",
                binding.command
            );
            for chord in binding.chords {
                assert!(!chords.contains(chord), "{chord:?} is taken twice");
                chords.push(*chord);
            }
        }
        assert_eq!(lookup(Key::Escape, NONE), None);
        assert_eq!(lookup(Key::q, CTRL), None);
    }

    /// The keys find their commands; one that is bound to nothing, or held
    /// with a modifier it does not take, finds none.
    #[test]
    fn finds_the_command_for_a_key() {
        assert_eq!(lookup(Key::n, NONE), Some(Command::NewTodo));
        assert_eq!(lookup(Key::r, NONE), Some(Command::Refresh));
        assert_eq!(lookup(Key::F5, NONE), Some(Command::Refresh));
        assert_eq!(lookup(Key::s, NONE), Some(Command::Sources));
        assert_eq!(lookup(Key::comma, CTRL), Some(Command::Settings));
        assert_eq!(lookup(Key::_1, NONE), Some(Command::ActivityTab));
        assert_eq!(lookup(Key::_2, NONE), Some(Command::BoardTab));
        assert_eq!(lookup(Key::k, CTRL), Some(Command::ShowKeys));
        assert_eq!(lookup(Key::x, NONE), None);
        assert_eq!(lookup(Key::n, CTRL), None, "Ctrl+N is not n");
        assert_eq!(lookup(Key::n, ALT), None);
        assert_eq!(lookup(Key::comma, NONE), None, "a comma is not Ctrl+,");
        assert_eq!(lookup(Key::F5, SHIFT), None, "Shift counts on F5");
    }

    /// `?` comes with Shift held on most layouts, Caps Lock gives Ctrl+K
    /// as a capital K, and a lock key or a mouse button held changes
    /// nothing.
    #[test]
    fn reads_keys_as_typed() {
        assert_eq!(lookup(Key::question, SHIFT), Some(Command::ShowKeys));
        assert_eq!(lookup(Key::K, CTRL | SHIFT), Some(Command::ShowKeys));
        assert_eq!(lookup(Key::K, CTRL), Some(Command::ShowKeys));
        let held = ModifierType::LOCK_MASK | ModifierType::BUTTON1_MASK;
        assert_eq!(lookup(Key::r, held), Some(Command::Refresh));
        assert_eq!(lookup(Key::N, SHIFT), None, "N is not n");
    }

    /// Keys read as a person writes them.
    #[test]
    fn names_the_keys() {
        let named = |command| {
            binding(command)
                .chords
                .iter()
                .map(Chord::text)
                .collect::<Vec<_>>()
        };
        assert_eq!(named(Command::ShowKeys), ["?", "Ctrl+K"]);
        assert_eq!(named(Command::Refresh), ["r", "F5"]);
        assert_eq!(named(Command::Settings), ["Ctrl+,"]);
        assert_eq!(named(Command::SendAway), ["Esc"]);
        assert_eq!(named(Command::Quit), ["Ctrl+Q"]);
        assert_eq!(
            chord(Key::Right, SHIFT).text(),
            "Shift+→",
            "arrows read as arrows"
        );
        assert_eq!(hinted("Refresh", Command::Refresh), "Refresh (r)");
        assert_eq!(hinted("Settings", Command::Settings), "Settings (Ctrl+,)");
        assert_eq!(
            hinted("Settings (showing Work)", Command::Settings),
            "Settings (showing Work; Ctrl+,)",
            "one pair of brackets"
        );
    }

    fn labels(query: &str) -> Vec<&'static str> {
        search(query).into_iter().map(|b| b.label).collect()
    }

    /// The dialog's search goes by what a key does, best match first, and
    /// by the key itself after that.
    #[test]
    fn searches_by_what_a_key_does() {
        assert_eq!(labels("").len(), BINDINGS.len());
        assert_eq!(labels("  "), labels(""));
        assert_eq!(labels("todo"), ["Write down a todo"]);
        assert_eq!(labels("TODO"), ["Write down a todo"], "in any case");
        assert_eq!(
            labels("go to"),
            ["Go to the Activity", "Go to the Board"],
            "a start ranks before what holds it"
        );
        assert_eq!(
            labels("the board"),
            ["Go to the Board"],
            "or what holds it, in order"
        );
        assert_eq!(
            labels("board go"),
            ["Go to the Board"],
            "each word, anywhere"
        );
        assert_eq!(
            labels("show the"),
            ["Show the keys", "Show the sources"],
            "ties keep the table's order"
        );
        assert_eq!(labels("ctrl k"), ["Show the keys"], "by its key");
        assert_eq!(labels("ctrl+,"), ["Open Settings"]);
        assert_eq!(labels("f5"), ["Refresh the sources"]);
        assert_eq!(labels("nothing like it"), Vec::<&str>::new());

        assert_eq!(
            labels("o")[0],
            "Open Settings",
            "a later label that starts with it ranks first"
        );
        // A label that holds the query outranks a key it names.
        assert_eq!(labels("s")[0], "Show the keys");
        assert!(labels("s").contains(&"Show the sources"));
    }

    /// In the keys dialog, Esc closes it, as Backspace does once the query
    /// is empty; the arrows move through the list and Enter runs the
    /// command picked. Other keys go to the search.
    #[test]
    fn steers_the_keys_dialog() {
        assert_eq!(dialog_key(Key::Escape, NONE, false), Some(DialogKey::Close));
        assert_eq!(
            dialog_key(Key::BackSpace, NONE, true),
            Some(DialogKey::Close)
        );
        assert_eq!(dialog_key(Key::BackSpace, NONE, false), None);
        assert_eq!(dialog_key(Key::BackSpace, CTRL, true), None);
        assert_eq!(dialog_key(Key::Up, NONE, true), Some(DialogKey::Previous));
        assert_eq!(dialog_key(Key::Down, NONE, true), Some(DialogKey::Next));
        assert_eq!(dialog_key(Key::Return, NONE, false), Some(DialogKey::Run));
        assert_eq!(dialog_key(Key::KP_Enter, NONE, false), Some(DialogKey::Run));
        assert_eq!(dialog_key(Key::a, NONE, true), None);
    }
}
