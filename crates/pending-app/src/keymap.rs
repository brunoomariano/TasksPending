//! The window's keys: one table that the key handler, the keys dialog and
//! the hints on buttons and menus all read, so each key is named once. The
//! mouse keeps doing everything it did; a key is another way to the same
//! thing.

use gtk4::gdk::{Key, ModifierType};

use crate::nav::Held;

/// Something a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    ShowKeys,
    NewTodo,
    Refresh,
    Settings,
    ActivityTab,
    BoardTab,
    SwitchTab,
    Snoozed,
    OpenStacks,
    SendAway,
    Quit,
    Up,
    Down,
    Left,
    Right,
    First,
    Last,
    Pick,
    NextStage,
    PreviousStage,
    OpenLink,
    EditTodo,
    Menu,
    Alert,
    Snooze,
    Delete,
}

/// Where the keys dialog lists a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Window,
    Moving,
    Cards,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Window => "Window",
            Section::Moving => "Moving between cards",
            Section::Cards => "The card in focus",
        }
    }
}

/// Where the keyboard focus is, for the keys that need a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Not on a card.
    Elsewhere,
    /// On something inside a card: only its open menu, now that a card's
    /// link and menu button take no focus.
    InCard,
    /// On the card itself.
    OnCard,
}

/// What a key needs to do anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    Nothing,
    /// The focus on a card or inside one.
    Card,
    /// The focus on the card itself, not inside it, in its open menu,
    /// which takes that key for itself.
    CardItself,
}

impl Needs {
    fn met(self, focus: Focus) -> bool {
        match self {
            Needs::Nothing => true,
            Needs::Card => focus != Focus::Elsewhere,
            Needs::CardItself => focus == Focus::OnCard,
        }
    }
}

/// A key with the modifiers held with it, and what it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub mods: ModifierType,
    pub needs: Needs,
}

const NONE: ModifierType = ModifierType::empty();
const CTRL: ModifierType = ModifierType::CONTROL_MASK;
const SHIFT: ModifierType = ModifierType::SHIFT_MASK;
const ALT: ModifierType = ModifierType::ALT_MASK;
const SUPER: ModifierType = ModifierType::SUPER_MASK;

const fn chord(key: Key, mods: ModifierType) -> Chord {
    Chord {
        key,
        mods,
        needs: Needs::Nothing,
    }
}

const fn on_card(key: Key, mods: ModifierType) -> Chord {
    Chord {
        key,
        mods,
        needs: Needs::Card,
    }
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
    bind(
        Command::SwitchTab,
        "Switch between the Activity and the Board",
        Section::Window,
        &[chord(Key::Tab, NONE), chord(Key::Tab, SHIFT)],
    ),
    // The keyboard reaches the cards only, so what the mouse opens to show
    // more of them has keys: these two, and Down past a stack's last card.
    bind(
        Command::Snoozed,
        "Show the snoozed cards",
        Section::Window,
        &[chord(Key::s, NONE)],
    ),
    bind(
        Command::OpenStacks,
        "Open the collapsed stacks",
        Section::Window,
        &[chord(Key::c, NONE)],
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
    // The keys that move reach the cards from anywhere in the window: the
    // keyboard focus goes only to cards.
    bind(
        Command::Up,
        "Move up",
        Section::Moving,
        &[chord(Key::Up, NONE), chord(Key::k, NONE)],
    ),
    bind(
        Command::Down,
        "Move down (shows the rest of a stack)",
        Section::Moving,
        &[chord(Key::Down, NONE), chord(Key::j, NONE)],
    ),
    bind(
        Command::Left,
        "Move to the column on the left",
        Section::Moving,
        &[chord(Key::Left, NONE), chord(Key::h, NONE)],
    ),
    bind(
        Command::Right,
        "Move to the column on the right",
        Section::Moving,
        &[chord(Key::Right, NONE), chord(Key::l, NONE)],
    ),
    bind(
        Command::First,
        "Go to the first card of the column",
        Section::Moving,
        &[chord(Key::g, NONE)],
    ),
    bind(
        Command::Last,
        "Go to the last card of the column",
        Section::Moving,
        &[chord(Key::G, NONE)],
    ),
    bind(
        Command::Pick,
        "Send to or remove from the Activity",
        Section::Cards,
        &[Chord {
            needs: Needs::CardItself,
            ..chord(Key::Return, NONE)
        }],
    ),
    bind(
        Command::NextStage,
        "Move to the next stage",
        Section::Cards,
        &[on_card(Key::Right, SHIFT), on_card(Key::L, NONE)],
    ),
    bind(
        Command::PreviousStage,
        "Move back a stage",
        Section::Cards,
        &[on_card(Key::Left, SHIFT), on_card(Key::H, NONE)],
    ),
    bind(
        Command::OpenLink,
        "Open the link",
        Section::Cards,
        &[on_card(Key::o, NONE)],
    ),
    bind(
        Command::EditTodo,
        "Edit the todo",
        Section::Cards,
        &[on_card(Key::e, NONE)],
    ),
    bind(
        Command::Menu,
        "Open the card's menu",
        Section::Cards,
        &[on_card(Key::period, CTRL)],
    ),
    bind(
        Command::Alert,
        "Set an alert",
        Section::Cards,
        &[on_card(Key::a, NONE)],
    ),
    bind(
        Command::Snooze,
        "Snooze",
        Section::Cards,
        &[on_card(Key::z, NONE)],
    ),
    bind(
        Command::Delete,
        "Delete the todo (press twice)",
        Section::Cards,
        &[on_card(Key::Delete, NONE)],
    ),
];

impl Binding {
    /// Whether holding the key repeats the command: only the moves do, so
    /// a key that acts on a card or opens something acts once per press.
    pub fn repeats(&self) -> bool {
        self.section == Section::Moving
    }
}

/// The key a card menu's entry names for `command`, if any. GTK makes it
/// a shortcut of the entry while the menu is open, so only the keys that
/// are safe there have one: not Enter, which picks the entry in focus,
/// nor Delete, which would delete on one press.
pub fn menu_hint(command: Command) -> Option<&'static Chord> {
    match command {
        Command::NextStage | Command::PreviousStage | Command::EditTodo => {
            binding(command).chords.first()
        }
        _ => None,
    }
}

/// The binding of `command`.
pub fn binding(command: Command) -> &'static Binding {
    BINDINGS
        .iter()
        .find(|binding| binding.command == command)
        .expect("every command has a binding")
}

/// What `key` with `mods` held does with the focus at `focus`, if the key
/// handler takes it.
pub fn lookup(key: Key, mods: ModifierType, focus: Focus) -> Option<Command> {
    BINDINGS
        .iter()
        .filter(|binding| !binding.elsewhere)
        .find(|binding| {
            binding
                .chords
                .iter()
                .any(|c| c.needs.met(focus) && c.matches(key, mods))
        })
        .map(|binding| binding.command)
}

/// Whether `key` gives a character, which then carries Shift itself: `?`
/// is Shift+/ and `G` Shift+g.
fn printable(key: Key) -> bool {
    key.to_unicode().is_some_and(|c| !c.is_control())
}

/// The modifiers a chord is written with, in that order.
const MODIFIERS: [(ModifierType, &str, &str); 4] = [
    (CTRL, "Ctrl+", "<Control>"),
    (ALT, "Alt+", "<Alt>"),
    (SUPER, "Super+", "<Super>"),
    (SHIFT, "Shift+", "<Shift>"),
];

impl Chord {
    /// Whether `key` pressed with `mods` is this chord. Mouse buttons held
    /// do not count, nor Shift on a key that gives a character; a letter
    /// reads as if Caps Lock were off, and with Ctrl or Alt counts in
    /// either case.
    pub fn matches(&self, key: Key, mods: ModifierType) -> bool {
        // GDK reports Shift+Tab as its own key.
        let mut key = if key == Key::ISO_Left_Tab {
            Key::Tab
        } else {
            key
        };
        if mods.contains(ModifierType::LOCK_MASK) && key.to_lower() != key.to_upper() {
            // Caps Lock gives L for l, and l for Shift+l.
            key = if key.is_upper() {
                key.to_lower()
            } else {
                key.to_upper()
            };
        }
        let mut mods = mods & (CTRL | SHIFT | ALT | SUPER);
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
        for (mask, name, _) in MODIFIERS {
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

    /// The chord in GTK's accelerator syntax, for a menu entry's hint:
    /// `<Control>k`, `question`.
    pub fn accel(&self) -> String {
        let mut accel = String::new();
        for (mask, _, name) in MODIFIERS {
            if self.mods.contains(mask) {
                accel.push_str(name);
            }
        }
        accel.push_str(
            &self
                .key
                .name()
                .map(|name| name.to_string())
                .unwrap_or_default(),
        );
        accel
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

/// What the window does with a key that went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Do the command.
    Run(Command),
    /// Keep the key from the focused widget, and do nothing.
    Swallow,
    /// Let the focused widget have it.
    Pass,
}

/// What the window does with `key`, hardware `code`, going down, `held`
/// noting every key down. `focus` is where the keyboard focus is, or
/// `None` while the window's keys stand aside (a dialog or a menu open,
/// or typing). A key held down repeats, and only the moves go on: a key
/// whose press ran something goes nowhere until it comes up, not even
/// into the dialog it opened; one held through a dialog does not act
/// once the dialog closes; and Esc, whose press closed what it was for,
/// does not then send the window away.
pub fn verdict(
    held: &mut Held,
    code: u32,
    key: Key,
    mods: ModifierType,
    focus: Option<Focus>,
) -> Verdict {
    let fresh = held.press(code);
    if !fresh && (held.spent(code) || key == Key::Escape) {
        return Verdict::Swallow;
    }
    let Some(focus) = focus else {
        return Verdict::Pass;
    };
    match lookup(key, mods, focus) {
        Some(command) if binding(command).repeats() => Verdict::Run(command),
        Some(command) if fresh => {
            held.spend(code);
            Verdict::Run(command)
        }
        Some(_) => Verdict::Swallow,
        None => Verdict::Pass,
    }
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
    use gtk4::glib::translate::IntoGlib;

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
                let pressed = (chord.key, chord.mods);
                assert!(!chords.contains(&pressed), "{chord:?} is taken twice");
                chords.push(pressed);
            }
        }
        for focus in [Focus::Elsewhere, Focus::InCard, Focus::OnCard] {
            assert_eq!(lookup(Key::Escape, NONE, focus), None);
            assert_eq!(lookup(Key::q, CTRL, focus), None);
        }
    }

    fn window(key: Key, mods: ModifierType) -> Option<Command> {
        lookup(key, mods, Focus::Elsewhere)
    }

    /// The keys find their commands; one that is bound to nothing, or held
    /// with a modifier it does not take, finds none.
    #[test]
    fn finds_the_command_for_a_key() {
        assert_eq!(window(Key::n, NONE), Some(Command::NewTodo));
        assert_eq!(window(Key::r, NONE), Some(Command::Refresh));
        assert_eq!(window(Key::F5, NONE), Some(Command::Refresh));
        assert_eq!(window(Key::s, NONE), Some(Command::Snoozed));
        assert_eq!(window(Key::c, NONE), Some(Command::OpenStacks));
        assert_eq!(window(Key::comma, CTRL), Some(Command::Settings));
        assert_eq!(window(Key::Tab, NONE), Some(Command::SwitchTab));
        assert_eq!(window(Key::Tab, SHIFT), Some(Command::SwitchTab));
        assert_eq!(
            window(Key::ISO_Left_Tab, SHIFT),
            Some(Command::SwitchTab),
            "Shift+Tab, as GDK reports it"
        );
        assert_eq!(window(Key::Tab, CTRL), None);
        assert_eq!(window(Key::_1, NONE), Some(Command::ActivityTab));
        assert_eq!(window(Key::_2, NONE), Some(Command::BoardTab));
        assert_eq!(window(Key::k, CTRL), Some(Command::ShowKeys));
        assert_eq!(window(Key::x, NONE), None);
        assert_eq!(window(Key::n, CTRL), None, "Ctrl+N is not n");
        assert_eq!(window(Key::n, ALT), None);
        assert_eq!(window(Key::comma, NONE), None, "a comma is not Ctrl+,");
        assert_eq!(window(Key::F5, SHIFT), None, "Shift counts on F5");
    }

    /// `?` comes with Shift held on most layouts, Ctrl+K can come as a
    /// capital K, and a mouse button held changes nothing.
    #[test]
    fn reads_keys_as_typed() {
        assert_eq!(window(Key::question, SHIFT), Some(Command::ShowKeys));
        assert_eq!(window(Key::K, CTRL | SHIFT), Some(Command::ShowKeys));
        assert_eq!(window(Key::K, CTRL), Some(Command::ShowKeys));
        assert_eq!(
            window(Key::r, ModifierType::BUTTON1_MASK),
            Some(Command::Refresh),
            "a mouse button held changes nothing"
        );
        assert_eq!(window(Key::N, SHIFT), None, "N is not n");
    }

    /// Caps Lock turns the letters GDK reports to capitals, and Shift
    /// with it back to small ones: the keys read the letter typed as if
    /// it were off, so l still moves the focus and does not move the card.
    #[test]
    fn reads_letters_as_if_caps_lock_were_off() {
        let lock = ModifierType::LOCK_MASK;
        assert_eq!(window(Key::R, lock), Some(Command::Refresh));
        assert_eq!(window(Key::L, lock), Some(Command::Right));
        assert_eq!(window(Key::G, lock), Some(Command::First));
        assert_eq!(window(Key::g, lock | SHIFT), Some(Command::Last));
        assert_eq!(
            lookup(Key::l, lock | SHIFT, Focus::OnCard),
            Some(Command::NextStage)
        );
        assert_eq!(
            lookup(Key::L, lock, Focus::OnCard),
            Some(Command::Right),
            "not the next stage"
        );
        assert_eq!(window(Key::K, CTRL | lock), Some(Command::ShowKeys));
        assert_eq!(window(Key::question, SHIFT | lock), Some(Command::ShowKeys));
        assert_eq!(window(Key::_1, lock), Some(Command::ActivityTab));
    }

    /// Holding a key repeats it. Only the keys that move the focus take
    /// the repeats: one that acts on a card or opens something acts once
    /// per press, so holding Delete or Enter does not go down a column.
    #[test]
    fn only_the_moves_repeat() {
        use Command::{Down, First, Last, Left, Right, Up};
        let moves = [Up, Down, Left, Right, First, Last];
        for binding in BINDINGS {
            let repeats = moves.contains(&binding.command);
            assert_eq!(binding.repeats(), repeats, "{:?}", binding.command);
        }
    }

    /// A key held down: a move goes on; a key that ran something goes
    /// nowhere until it comes up, not into the dialog it opened either;
    /// one held through a dialog does not act once it closes; Esc that
    /// closed a dialog does not send the window away; and in a text field
    /// a key types, held or not.
    #[test]
    fn a_held_key_acts_once() {
        let held = &mut Held::default();
        let (on_card, elsewhere) = (Some(Focus::OnCard), Some(Focus::Elsewhere));
        assert_eq!(down(held, Key::j, on_card), Verdict::Run(Command::Down));
        assert_eq!(down(held, Key::j, on_card), Verdict::Run(Command::Down));

        assert_eq!(down(held, Key::e, on_card), Verdict::Run(Command::EditTodo));
        assert_eq!(down(held, Key::e, None), Verdict::Swallow, "not typed");

        assert_eq!(down(held, Key::Return, None), Verdict::Pass, "the dialog's");
        assert_eq!(down(held, Key::Return, on_card), Verdict::Swallow);

        assert_eq!(down(held, Key::Escape, None), Verdict::Pass, "the dialog's");
        assert_eq!(down(held, Key::Escape, elsewhere), Verdict::Swallow);

        for key in [Key::e, Key::Return, Key::Escape] {
            held.release(code(key));
        }
        assert_eq!(down(held, Key::e, None), Verdict::Pass, "typed");
        assert_eq!(down(held, Key::e, None), Verdict::Pass, "and again");
        assert_eq!(
            down(held, Key::Return, on_card),
            Verdict::Run(Command::Pick)
        );
        assert_eq!(
            down(held, Key::Escape, elsewhere),
            Verdict::Pass,
            "Esc's own"
        );

        // Enter running a key from the keys dialog, which spends it.
        held.release(code(Key::Return));
        assert_eq!(down(held, Key::Return, None), Verdict::Pass);
        held.spend(code(Key::Return));
        assert_eq!(down(held, Key::Return, on_card), Verdict::Swallow);
    }

    /// `key` going down, with a hardware code of its own.
    fn down(held: &mut Held, key: Key, focus: Option<Focus>) -> Verdict {
        verdict(held, code(key), key, NONE, focus)
    }

    fn code(key: Key) -> u32 {
        key.into_glib()
    }

    /// The card menu names the keys that are safe to press while it is
    /// open, where GTK makes them shortcuts of the entries: the stage moves
    /// and Edit. Not Enter, which picks the entry in focus, nor Delete,
    /// which there would delete on one press.
    #[test]
    fn the_menu_names_only_the_safe_keys() {
        let hint = |command| menu_hint(command).map(Chord::accel);
        assert_eq!(hint(Command::NextStage).as_deref(), Some("<Shift>Right"));
        assert_eq!(hint(Command::PreviousStage).as_deref(), Some("<Shift>Left"));
        assert_eq!(hint(Command::EditTodo).as_deref(), Some("e"));
        assert_eq!(hint(Command::Pick), None);
        assert_eq!(hint(Command::Delete), None);
        assert_eq!(hint(Command::Refresh), None);
    }

    /// The keys that act on a card need the focus on one, and Enter on the
    /// card itself: inside it, in its open menu, Enter is the menu's. The
    /// keys that move, the arrows as the letters, reach the cards from
    /// anywhere: the focus goes only to cards.
    #[test]
    fn card_keys_need_a_card() {
        use Focus::{Elsewhere, InCard, OnCard};
        assert_eq!(lookup(Key::Down, NONE, Elsewhere), Some(Command::Down));
        assert_eq!(lookup(Key::Up, NONE, Elsewhere), Some(Command::Up));
        assert_eq!(lookup(Key::Left, NONE, Elsewhere), Some(Command::Left));
        assert_eq!(lookup(Key::Right, NONE, Elsewhere), Some(Command::Right));
        assert_eq!(lookup(Key::Down, NONE, InCard), Some(Command::Down));
        assert_eq!(lookup(Key::j, NONE, Elsewhere), Some(Command::Down));
        assert_eq!(lookup(Key::k, NONE, OnCard), Some(Command::Up));
        assert_eq!(lookup(Key::h, NONE, InCard), Some(Command::Left));
        assert_eq!(lookup(Key::l, NONE, InCard), Some(Command::Right));
        assert_eq!(lookup(Key::g, NONE, Elsewhere), Some(Command::First));
        assert_eq!(lookup(Key::G, SHIFT, Elsewhere), Some(Command::Last));
        assert_eq!(lookup(Key::Return, NONE, OnCard), Some(Command::Pick));
        assert_eq!(lookup(Key::Return, NONE, InCard), None);
        assert_eq!(lookup(Key::Return, NONE, Elsewhere), None);
        assert_eq!(lookup(Key::Right, SHIFT, OnCard), Some(Command::NextStage));
        assert_eq!(lookup(Key::Right, SHIFT, Elsewhere), None);
        assert_eq!(lookup(Key::L, SHIFT, InCard), Some(Command::NextStage));
        assert_eq!(
            lookup(Key::Left, SHIFT, OnCard),
            Some(Command::PreviousStage)
        );
        assert_eq!(lookup(Key::H, SHIFT, OnCard), Some(Command::PreviousStage));
        assert_eq!(lookup(Key::o, NONE, Elsewhere), None);
        assert_eq!(lookup(Key::o, NONE, OnCard), Some(Command::OpenLink));
        assert_eq!(lookup(Key::e, NONE, InCard), Some(Command::EditTodo));
        assert_eq!(lookup(Key::period, CTRL, InCard), Some(Command::Menu));
        assert_eq!(lookup(Key::a, NONE, OnCard), Some(Command::Alert));
        assert_eq!(lookup(Key::z, NONE, OnCard), Some(Command::Snooze));
        assert_eq!(lookup(Key::Delete, NONE, OnCard), Some(Command::Delete));
        assert_eq!(lookup(Key::Delete, NONE, Elsewhere), None);
        assert_eq!(
            lookup(Key::n, NONE, OnCard),
            Some(Command::NewTodo),
            "the window's keys work on a card too"
        );
    }

    /// Keys read as a person writes them, and as GTK names them.
    #[test]
    fn names_the_keys() {
        let named = |command| {
            binding(command)
                .chords
                .iter()
                .map(|c| (c.text(), c.accel()))
                .collect::<Vec<_>>()
        };
        let pair = |text: &str, accel: &str| (text.to_owned(), accel.to_owned());
        assert_eq!(
            named(Command::ShowKeys),
            [pair("?", "question"), pair("Ctrl+K", "<Control>k")]
        );
        assert_eq!(named(Command::Refresh), [pair("r", "r"), pair("F5", "F5")]);
        assert_eq!(named(Command::Settings), [pair("Ctrl+,", "<Control>comma")]);
        assert_eq!(named(Command::SendAway), [pair("Esc", "Escape")]);
        assert_eq!(named(Command::Quit), [pair("Ctrl+Q", "<Control>q")]);
        assert_eq!(
            named(Command::NextStage),
            [pair("Shift+→", "<Shift>Right"), pair("L", "L")]
        );
        assert_eq!(named(Command::Down), [pair("↓", "Down"), pair("j", "j")]);
        assert_eq!(named(Command::Pick), [pair("Enter", "Return")]);
        assert_eq!(
            named(Command::SwitchTab),
            [pair("Tab", "Tab"), pair("Shift+Tab", "<Shift>Tab")]
        );
        assert_eq!(named(Command::Menu), [pair("Ctrl+.", "<Control>period")]);
        assert_eq!(named(Command::Snoozed), [pair("s", "s")]);
        assert_eq!(named(Command::OpenStacks), [pair("c", "c")]);
        assert_eq!(named(Command::Delete), [pair("Delete", "Delete")]);
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
        assert_eq!(
            labels("todo"),
            [
                "Write down a todo",
                "Edit the todo",
                "Delete the todo (press twice)"
            ]
        );
        assert_eq!(labels("WRITE"), ["Write down a todo"], "in any case");
        assert_eq!(
            labels("go to the"),
            [
                "Go to the Activity",
                "Go to the Board",
                "Go to the first card of the column",
                "Go to the last card of the column",
            ],
            "a start ranks before what holds it"
        );
        assert_eq!(
            labels("the board"),
            [
                "Go to the Board",
                "Switch between the Activity and the Board"
            ],
            "or what holds it, ties in the table's order"
        );
        assert_eq!(
            labels("board go"),
            ["Go to the Board"],
            "each word, anywhere"
        );
        assert_eq!(labels("stage next"), ["Move to the next stage"]);
        assert_eq!(
            labels("show the"),
            [
                "Show the keys",
                "Show the snoozed cards",
                "Move down (shows the rest of a stack)"
            ]
        );
        assert_eq!(labels("sources"), ["Refresh the sources"]);
        assert_eq!(labels("snoozed"), ["Show the snoozed cards"]);
        assert_eq!(labels("collapsed"), ["Open the collapsed stacks"]);
        assert_eq!(labels("rest"), ["Move down (shows the rest of a stack)"]);
        assert_eq!(labels("ctrl k"), ["Show the keys"], "by its key");
        assert_eq!(labels("tab"), ["Switch between the Activity and the Board"]);
        assert_eq!(labels("ctrl+,"), ["Open Settings"]);
        assert_eq!(labels("f5"), ["Refresh the sources"]);
        assert_eq!(labels("nothing like it"), Vec::<&str>::new());

        assert_eq!(
            labels("o")[0],
            "Open Settings",
            "a later label that starts with it ranks first"
        );
        // A label that holds the query outranks a key it names.
        assert_eq!(labels("k")[0], "Show the keys");
        assert_eq!(labels("k").last(), Some(&"Move up"));
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
