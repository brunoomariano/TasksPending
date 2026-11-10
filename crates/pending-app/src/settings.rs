//! What Settings changes in the config file and the env file, apart from
//! how it looks: where the files are, what they hold, and each change. A
//! change is written only while the file still holds what Settings showed,
//! and only when the dashboard would load the result.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use pending_core::config::{AppConfig, SourceConfig, StackConfig, StackSort};
use pending_core::edit::{ConfigDoc, EditError, Fields};
use pending_core::schema::{Input, Kind, Var};
use pending_runtime::config::{LoadError, WriteError, check, write_file};
use pending_runtime::envfile::EnvFile;
use toml::Value;

/// Where Settings reads and writes one of its files: on disk, or, for the
/// sandbox, in memory.
#[derive(Clone, Debug)]
pub enum Place {
    File(PathBuf),
    Memory(Rc<RefCell<Option<String>>>),
}

impl Place {
    /// A file in memory, holding `text` (`None`: there is no file yet).
    pub fn memory(text: Option<&str>) -> Self {
        Self::Memory(Rc::new(RefCell::new(text.map(str::to_owned))))
    }

    /// Where it is, as Settings names it; `None` in memory.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => Some(path),
            Self::Memory(_) => None,
        }
    }

    /// What it holds; `None` when there is no such file yet.
    fn read(&self) -> Result<Option<String>, String> {
        match self {
            Self::File(path) => match std::fs::read_to_string(path) {
                Ok(text) => Ok(Some(text)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!("{} could not be read ({error})", path.display())),
            },
            Self::Memory(text) => Ok(text.borrow().clone()),
        }
    }

    /// Puts `text` in place of `was`, unless it holds something else now.
    /// `private` files are readable by their owner only.
    fn write(&self, text: &str, was: Option<&str>, private: bool) -> Result<(), Refusal> {
        match self {
            Self::File(path) => write_file(path, text, was, private).map_err(|error| match error {
                WriteError::Changed(_) => Refusal::Changed,
                error @ WriteError::Io { .. } => Refusal::Failed(error.to_string()),
            }),
            Self::Memory(held) => {
                let mut held = held.borrow_mut();
                if held.as_deref() != was {
                    return Err(Refusal::Changed);
                }
                *held = Some(text.to_owned());
                Ok(())
            }
        }
    }
}

/// Why a change was not written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The file changed outside Settings since it was read; Settings now
    /// shows what it holds.
    Changed,
    /// The dashboard would not load the result: what is wrong with it.
    Invalid(String),
    /// The file could not be read or written.
    Failed(String),
}

impl Refusal {
    /// What Settings says about it.
    pub fn message(&self) -> String {
        match self {
            Self::Changed => "The file changed outside Settings, so nothing was written; \
                              this is what it holds now."
                .to_owned(),
            Self::Invalid(why) | Self::Failed(why) => format!("Not saved: {why}."),
        }
    }
}

/// What the loader says is wrong, without the file's path, which Settings
/// shows apart.
fn invalid(error: &LoadError) -> String {
    match error {
        LoadError::Parse { source, .. } => source.message().to_owned(),
        LoadError::Invalid { source, .. } => source.to_string(),
        LoadError::Column {
            source_name,
            column,
            message,
            ..
        } => format!("source `{source_name}`, stack `{column}`: {message}"),
        other => other.to_string(),
    }
}

/// The config file as Settings shows it: the text it read, so that a
/// change is made to what Settings showed and to nothing else.
#[derive(Debug)]
pub struct ConfigFile {
    place: Place,
    text: Option<String>,
    unread: Option<String>,
}

impl ConfigFile {
    pub fn open(place: Place) -> Self {
        let mut file = Self {
            place,
            text: None,
            unread: None,
        };
        file.reload();
        file
    }

    pub fn place(&self) -> &Place {
        &self.place
    }

    /// Reads the file again.
    pub fn reload(&mut self) {
        match self.place.read() {
            Ok(text) => {
                self.text = text;
                self.unread = None;
            }
            Err(why) => {
                self.text = None;
                self.unread = Some(why);
            }
        }
    }

    /// What the file says (an empty config when there is none yet), or why
    /// Settings cannot show it.
    pub fn config(&self) -> Result<AppConfig, String> {
        if let Some(why) = &self.unread {
            return Err(why.clone());
        }
        toml::from_str(self.text.as_deref().unwrap_or(""))
            .map_err(|error: toml::de::Error| error.message().to_owned())
    }

    /// Makes `change` to the file: to the text Settings showed, then checked
    /// as the dashboard loads it, then written in one step. Says what
    /// `change` said, or why nothing was written.
    pub fn change<T>(
        &mut self,
        change: impl FnOnce(&mut ConfigDoc) -> Result<T, EditError>,
    ) -> Result<T, Refusal> {
        let now = self.place.read().map_err(Refusal::Failed)?;
        if now != self.text {
            self.reload();
            return Err(Refusal::Changed);
        }
        let mut doc = ConfigDoc::parse(now.as_deref().unwrap_or(""))
            .map_err(|error| Refusal::Invalid(error.to_string()))?;
        let made = change(&mut doc).map_err(|error| Refusal::Failed(error.to_string()))?;
        let text = doc.text();
        let path = self.place.path().unwrap_or(Path::new("config.toml"));
        check(&text, path).map_err(|error| Refusal::Invalid(invalid(&error)))?;
        if let Err(refusal) = self.place.write(&text, now.as_deref(), false) {
            self.reload();
            return Err(refusal);
        }
        self.text = Some(text);
        Ok(made)
    }
}

/// The env file as Settings shows it: which variables it sets, and the
/// value of those that are not secrets. A secret's value is never read
/// back out of it.
#[derive(Debug)]
pub struct EnvEdit {
    place: Place,
    text: Option<String>,
    unread: Option<String>,
}

impl EnvEdit {
    pub fn open(place: Place) -> Self {
        let mut file = Self {
            place,
            text: None,
            unread: None,
        };
        file.reload();
        file
    }

    pub fn place(&self) -> &Place {
        &self.place
    }

    /// Why the file could not be read, when it could not.
    pub fn unread(&self) -> Option<&str> {
        self.unread.as_deref()
    }

    pub fn reload(&mut self) {
        match self.place.read() {
            Ok(text) => {
                self.text = text;
                self.unread = None;
            }
            Err(why) => {
                self.text = None;
                self.unread = Some(why);
            }
        }
    }

    fn file(&self) -> EnvFile {
        EnvFile::parse(self.text.as_deref().unwrap_or(""))
    }

    /// The name `var` is set under, the first a source reads when several
    /// are; `None` when it is not set, or set to nothing.
    pub fn set_as(&self, var: &Var) -> Option<&'static str> {
        let file = self.file();
        var.names
            .iter()
            .copied()
            .find(|name| file.get(name).is_some_and(|value| !value.is_empty()))
    }

    /// The value of `var`, unless it is a secret.
    pub fn value(&self, var: &Var) -> Option<String> {
        if var.secret {
            return None;
        }
        let name = self.set_as(var)?;
        self.file().get(name).map(str::to_owned)
    }

    /// Sets `var` to `value` under its first name, or removes it when
    /// there is no value; either way its other names go, so the value
    /// written is the one read. A secret typed blank is refused: only
    /// Remove takes one out.
    pub fn set(&mut self, var: &Var, value: Option<&str>) -> Result<(), Refusal> {
        let value = value.map(str::trim);
        if var.secret && value.is_some_and(str::is_empty) {
            return Err(Refusal::Invalid(
                "a secret cannot be blank; Remove takes it out".to_owned(),
            ));
        }
        let value = value.filter(|value| !value.is_empty());
        if value.is_some_and(|value| value.contains(['\n', '\r'])) {
            return Err(Refusal::Invalid("a value is one line".to_owned()));
        }
        let now = self.place.read().map_err(Refusal::Failed)?;
        if now != self.text {
            self.reload();
            return Err(Refusal::Changed);
        }
        let mut file = self.file();
        let (first, others) = var.names.split_first().expect("a variable has a name");
        match value {
            Some(value) => file.set(first, value),
            None => file.remove(first),
        }
        for name in others {
            file.remove(name);
        }
        let text = file.text();
        if let Err(refusal) = self.place.write(&text, now.as_deref(), true) {
            self.reload();
            return Err(refusal);
        }
        self.text = Some(text);
        Ok(())
    }
}

/// A board in Settings: its sources by their place in the file, and
/// whether any is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardSources {
    pub board: String,
    pub on: bool,
    pub sources: Vec<usize>,
}

/// The sources by board, the boards in the order their first source comes.
pub fn by_board(config: &AppConfig) -> Vec<BoardSources> {
    let mut boards: Vec<BoardSources> = Vec::new();
    for (index, source) in config.sources.iter().enumerate() {
        match boards.iter_mut().find(|board| board.board == source.board) {
            Some(board) => {
                board.on |= source.enabled;
                board.sources.push(index);
            }
            None => boards.push(BoardSources {
                board: source.board.clone(),
                on: source.enabled,
                sources: vec![index],
            }),
        }
    }
    boards
}

/// Where the source at `index` goes to move up (or down) among its
/// board's: the place of the one before (after) it; `None` when it is
/// already first (last).
pub fn neighbour(config: &AppConfig, index: usize, up: bool) -> Option<usize> {
    let board = &config.sources.get(index)?.board;
    let same = |at: &usize| config.sources[*at].board == *board;
    if up {
        (0..index).rev().find(same)
    } else {
        (index + 1..config.sources.len()).find(same)
    }
}

/// `base`, or `base 2`, `base 3`… when a source already has that name.
pub fn free_name(config: &AppConfig, base: &str) -> String {
    let taken = |name: &str| config.sources.iter().any(|source| source.name == name);
    std::iter::once(base.to_owned())
        .chain((2..).map(|n| format!("{base} {n}")))
        .find(|name| !taken(name))
        .expect("some name is free")
}

/// A new source of `kind`: named after it, on its board, with its default
/// stacks.
pub fn new_source(config: &AppConfig, kind: &Kind) -> Fields {
    vec![
        (
            "name".to_owned(),
            Value::String(free_name(config, kind.label)),
        ),
        ("kind".to_owned(), Value::String(kind.id.to_owned())),
        ("board".to_owned(), Value::String(kind.board.to_owned())),
    ]
}

/// How often the sources may be refreshed, in seconds.
pub const REFRESH_CHOICES: [u64; 5] = [60, 120, 300, 900, 1800];
/// How long a refresh may take, in seconds.
pub const TIMEOUT_CHOICES: [u64; 4] = [30, 60, 120, 300];

/// `choices`, with `current` among them when it is not one of them.
pub fn durations(choices: &[u64], current: u64) -> Vec<u64> {
    let mut all = choices.to_vec();
    if !all.contains(&current) {
        all.push(current);
        all.sort_unstable();
    }
    all
}

/// `secs` as Settings writes it: `90 s`, `5 min`, `2 h`.
pub fn duration_label(secs: u64) -> String {
    match secs {
        secs if secs % 3600 == 0 => format!("{} h", secs / 3600),
        secs if secs % 60 == 0 => format!("{} min", secs / 60),
        secs => format!("{secs} s"),
    }
}

/// What a source's page holds until it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceForm {
    pub name: String,
    pub board: String,
    /// Its own interval and timeout; `None` follows the file's.
    pub refresh_seconds: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub icon: String,
    pub icon_dark: String,
}

/// A key to set (or, `None`, to remove) and its value.
pub type Change = (&'static str, Option<Value>);

impl SourceForm {
    pub fn of(source: &SourceConfig) -> Self {
        Self {
            name: source.name.clone(),
            board: source.board.clone(),
            refresh_seconds: source.refresh_seconds,
            timeout_seconds: source.timeout_seconds,
            icon: source.icon.clone().unwrap_or_default(),
            icon_dark: source.icon_dark.clone().unwrap_or_default(),
        }
    }

    /// What to change for `source` to be as the page has it, the keys
    /// left as they were untouched; or what is missing.
    pub fn changes(&self, source: &SourceConfig) -> Result<Vec<Change>, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("a source needs a name".to_owned());
        }
        let board = self.board.trim();
        if board.is_empty() {
            return Err("a source needs a board".to_owned());
        }
        let seconds = |secs: u64| Value::Integer(i64::try_from(secs).unwrap_or(i64::MAX));
        let mut changes = Vec::new();
        if name != source.name {
            changes.push(("name", text_value(name)));
        }
        if board != source.board {
            changes.push(("board", text_value(board)));
        }
        if self.refresh_seconds != source.refresh_seconds {
            changes.push(("refresh_seconds", self.refresh_seconds.map(seconds)));
        }
        if self.timeout_seconds != source.timeout_seconds {
            changes.push(("timeout_seconds", self.timeout_seconds.map(seconds)));
        }
        for (key, typed, was) in [
            ("icon", &self.icon, &source.icon),
            ("icon_dark", &self.icon_dark, &source.icon_dark),
        ] {
            let typed = text_value(typed);
            if typed.as_ref().and_then(Value::as_str) != was.as_deref() {
                changes.push((key, typed));
            }
        }
        Ok(changes)
    }
}

/// A word of a choice as Settings shows it: `review_requested` is
/// `review requested`.
pub fn word_label(word: &str) -> String {
    word.replace('_', " ")
}

/// A stack in a line: what it shows (its mode, else its first key set),
/// or that it filters nothing of its own.
pub fn stack_summary(kind: &Kind, stack: &StackConfig) -> String {
    let shown = |value: &Value| match value {
        Value::String(text) => word_label(text),
        Value::Array(words) => words
            .iter()
            .filter_map(Value::as_str)
            .map(word_label)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Boolean(true) => "yes".to_owned(),
        Value::Boolean(false) => "no".to_owned(),
        other => other.to_string(),
    };
    let summary = kind
        .modes
        .iter()
        .chain(kind.fields)
        .find_map(|field| {
            let value = stack.filter.get(field.key)?;
            Some(match (field.input, value) {
                (Input::Toggle, Value::Boolean(true)) if kind.modes.contains(field) => {
                    field.label.to_owned()
                }
                _ => format!("{}: {}", field.label, shown(value)),
            })
        })
        .unwrap_or_else(|| "No filter of its own".to_owned());
    if stack.enabled {
        summary
    } else {
        format!("Off · {summary}")
    }
}

/// What a stack's page holds until it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct StackForm {
    pub name: String,
    pub enabled: bool,
    pub sort: StackSort,
    pub exclude: Vec<String>,
    /// Which of its kind's modes it shows, by key.
    pub mode: Option<&'static str>,
    /// The value of each key that is set, the modes' included.
    pub values: BTreeMap<&'static str, Value>,
}

impl StackForm {
    /// A new stack of `kind`, on its first mode.
    pub fn new(kind: &Kind) -> Self {
        Self {
            name: String::new(),
            enabled: true,
            sort: StackSort::default(),
            exclude: Vec::new(),
            mode: kind.modes.first().map(|mode| mode.key),
            values: BTreeMap::new(),
        }
    }

    /// `stack` as the file has it.
    pub fn of(kind: &Kind, stack: &StackConfig) -> Self {
        let values: BTreeMap<_, _> = kind
            .modes
            .iter()
            .chain(kind.fields)
            .filter_map(|field| Some((field.key, stack.filter.get(field.key)?.clone())))
            .collect();
        let mode = kind
            .modes
            .iter()
            .find(|mode| values.contains_key(mode.key))
            .or(kind.modes.first())
            .map(|mode| mode.key);
        Self {
            name: stack.name.clone(),
            enabled: stack.enabled,
            sort: stack.sort,
            exclude: stack.exclude.clone(),
            mode,
            values,
        }
    }

    /// Sets `key`, or (`None`) unsets it.
    pub fn set(&mut self, key: &'static str, value: Option<Value>) {
        match value {
            Some(value) => self.values.insert(key, value),
            None => self.values.remove(key),
        };
    }

    /// The text of `key`.
    pub fn text(&self, key: &str) -> String {
        match self.values.get(key) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        }
    }

    /// The words of `key`.
    pub fn words(&self, key: &str) -> Vec<String> {
        match self.values.get(key) {
            Some(Value::Array(words)) => words
                .iter()
                .filter_map(|word| word.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether `key` is on, off, or unset.
    pub fn toggle(&self, key: &str) -> Option<bool> {
        self.values.get(key).and_then(Value::as_bool)
    }

    /// The keys to write, in the order the page shows them; or what is
    /// missing.
    pub fn fields(&self, kind: &Kind) -> Result<Fields, String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("a stack needs a name".to_owned());
        }
        let mut fields: Fields = vec![("name".to_owned(), Value::String(name.to_owned()))];
        if !self.enabled {
            fields.push(("enabled".to_owned(), Value::Boolean(false)));
        }
        let mode = self
            .mode
            .and_then(|key| kind.modes.iter().find(|mode| mode.key == key));
        if let Some(mode) = mode {
            let value = match mode.input {
                Input::Toggle => Some(Value::Boolean(true)),
                _ => self.values.get(mode.key).cloned(),
            };
            let value = value.ok_or_else(|| format!("{} needs a value", mode.label))?;
            fields.push((mode.key.to_owned(), value));
        }
        for field in kind.fields {
            match self.values.get(field.key) {
                Some(value) => fields.push((field.key.to_owned(), value.clone())),
                None if field.required => return Err(format!("{} needs a value", field.label)),
                None => {}
            }
        }
        if self.sort != StackSort::default() {
            let sort = Value::try_from(self.sort).expect("a sort is a word");
            fields.push(("sort".to_owned(), sort));
        }
        if !self.exclude.is_empty() {
            fields.push(("exclude".to_owned(), words(&self.exclude)));
        }
        Ok(fields)
    }
}

/// `text` as a key's value: trimmed, and unset when there is nothing.
pub fn text_value(text: &str) -> Option<Value> {
    let text = text.trim();
    (!text.is_empty()).then(|| Value::String(text.to_owned()))
}

/// `words` as a key's value, each trimmed and the empty ones left out;
/// unset when none is left.
pub fn words_value(words: &[String]) -> Option<Value> {
    let words: Vec<String> = words
        .iter()
        .map(|word| word.trim().to_owned())
        .filter(|word| !word.is_empty())
        .collect();
    (!words.is_empty()).then(|| self::words(&words))
}

fn words(words: &[String]) -> Value {
    Value::Array(words.iter().cloned().map(Value::String).collect())
}

/// The sandbox's config, in memory: what its board shows, to change in
/// Settings without anything being written.
pub const SANDBOX_CONFIG: &str = include_str!("sandbox.toml");

/// The sandbox's env file, in memory: none of it is a real credential.
pub const SANDBOX_ENV: &str = "GITHUB_TOKEN=sandbox\n\
                               JIRA_BASE_URL=https://acme.atlassian.net\n\
                               JIRA_EMAIL=you@example.com\n";

#[cfg(test)]
mod tests {
    use super::*;
    use pending_core::config::SourceKind;
    use pending_core::schema;

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tasks-pending-settings-{}-{test}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const GITHUB: &str = "# mine\n[[sources]]\nname = \"gh\"\nkind = \"github\"\n";

    /// A change is made to the text Settings read, checked as the
    /// dashboard loads it, and written; one that would not load is not
    /// written, and neither is one to a file that changed since.
    #[test]
    fn writes_only_what_loads_onto_what_it_showed() {
        let dir = scratch("config");
        let path = dir.join("config.toml");
        std::fs::write(&path, GITHUB).unwrap();
        let mut file = ConfigFile::open(Place::File(path.clone()));
        assert_eq!(file.config().unwrap().sources[0].name, "gh");

        file.change(|doc| doc.set_source(0, "board", Some(Value::String("Work".into()))))
            .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("# mine\n") && written.contains("board = \"Work\""));
        assert_eq!(file.config().unwrap().sources[0].board, "Work");

        let refused = file.change(|doc| {
            doc.set_global("refresh_seconds", Some(Value::Integer(0)));
            Ok(())
        });
        assert_eq!(
            refused,
            Err(Refusal::Invalid(
                "`refresh_seconds` must be greater than zero".into()
            ))
        );
        let stack = vec![("name".to_owned(), Value::String("Mine".into()))];
        let refused = file.change(|doc| doc.add_stack(0, &stack)).unwrap_err();
        assert!(
            matches!(&refused, Refusal::Invalid(why) if why.starts_with("source `gh`, stack `Mine`")),
            "{refused:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);

        std::fs::write(&path, format!("refresh_seconds = 60\n{written}")).unwrap();
        let refused = file.change(|doc| doc.remove_source(0));
        assert_eq!(refused, Err(Refusal::Changed));
        assert!(std::fs::read_to_string(&path).unwrap().contains("gh"));
        // Settings now shows the file as it is, and a change goes through.
        assert_eq!(file.config().unwrap().refresh_seconds, 60);
        file.change(|doc| doc.remove_source(0)).unwrap();
        assert!(file.config().unwrap().sources.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With no config yet, Settings shows an empty one and the first
    /// change makes the file; one it cannot read is said, not shown.
    #[test]
    fn starts_the_config_and_says_what_it_cannot_read() {
        let place = Place::memory(None);
        let mut file = ConfigFile::open(place.clone());
        assert!(file.config().unwrap().sources.is_empty());
        let kind = schema::kind(SourceKind::Todoist);
        let at = file
            .change(|doc| doc.add_source(&new_source(&AppConfig::default(), kind)))
            .unwrap();
        assert_eq!(at, 0);
        let config = file.config().unwrap();
        assert_eq!(
            (
                config.sources[0].name.as_str(),
                config.sources[0].board.as_str()
            ),
            ("Todoist", "Personal")
        );
        let Place::Memory(held) = &place else {
            unreachable!()
        };
        assert!(
            held.borrow()
                .as_deref()
                .unwrap()
                .contains("kind = \"todoist\"")
        );

        let broken = ConfigFile::open(Place::memory(Some("[[sources]\n")));
        assert!(broken.config().is_err());
    }

    /// A credential is written under its first name, its other names gone,
    /// in a file only its owner reads; a secret's value never comes back
    /// out, the others' do.
    #[test]
    fn keeps_the_credentials_private() {
        use std::os::unix::fs::PermissionsExt;

        let plane = schema::kind(SourceKind::Plane);
        let key = &plane.vars[2];
        let address = &plane.vars[0];
        assert!(key.secret && !address.secret);

        let dir = scratch("env");
        let path = dir.join("tasks-pending").join("env");
        let mut env = EnvEdit::open(Place::File(path.clone()));
        assert_eq!(env.set_as(key), None);
        env.set(address, Some(" https://plane.example.com "))
            .unwrap();
        env.set(key, Some("s3cret")).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(env.set_as(key), Some("PLANE_API_KEY"));
        assert_eq!(env.value(key), None);
        assert_eq!(
            env.value(address).as_deref(),
            Some("https://plane.example.com")
        );

        std::fs::write(&path, "# Plane\nPLANE_TOKEN=old\n").unwrap();
        env.reload();
        assert_eq!(env.set_as(key), Some("PLANE_TOKEN"));
        env.set(key, Some("new")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Plane\nPLANE_API_KEY=new\n"
        );
        assert_eq!(
            env.set(key, Some("  ")),
            Err(Refusal::Invalid(
                "a secret cannot be blank; Remove takes it out".into()
            ))
        );
        assert_eq!(env.set_as(key), Some("PLANE_API_KEY"), "still set");
        env.set(address, Some(" ")).unwrap();
        assert_eq!(env.value(address), None, "an address typed blank goes");
        env.set(key, None).unwrap();
        assert_eq!(env.set_as(key), None);
        assert_eq!(
            env.set(key, Some("two\nlines")),
            Err(Refusal::Invalid("a value is one line".into()))
        );

        std::fs::write(&path, "PLANE_API_KEY=theirs\n").unwrap();
        assert_eq!(env.set(key, Some("mine")), Err(Refusal::Changed));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "PLANE_API_KEY=theirs\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn config(text: &str) -> AppConfig {
        toml::from_str(text).unwrap()
    }

    /// Sources are grouped by board in the order the boards first come; a
    /// board is on while any of its sources is, and a source moves among
    /// its board's.
    #[test]
    fn groups_and_moves_the_sources_by_board() {
        let config = config(
            "[[sources]]\nname = \"a\"\nkind = \"sample\"\nboard = \"Work\"\n\
             [[sources]]\nname = \"b\"\nkind = \"sample\"\nboard = \"Home\"\nenabled = false\n\
             [[sources]]\nname = \"c\"\nkind = \"sample\"\nboard = \"Work\"\nenabled = false\n\
             [[sources]]\nname = \"d\"\nkind = \"sample\"\nboard = \"Work\"\nenabled = false\n",
        );
        assert_eq!(
            by_board(&config),
            [
                BoardSources {
                    board: "Work".into(),
                    on: true,
                    sources: vec![0, 2, 3]
                },
                BoardSources {
                    board: "Home".into(),
                    on: false,
                    sources: vec![1]
                },
            ]
        );
        assert_eq!(neighbour(&config, 3, true), Some(2));
        assert_eq!(neighbour(&config, 2, true), Some(0));
        assert_eq!(neighbour(&config, 0, false), Some(2));
        assert_eq!(neighbour(&config, 0, true), None);
        assert_eq!(neighbour(&config, 3, false), None);
        assert_eq!(neighbour(&config, 1, false), None);
        assert_eq!(free_name(&config, "a"), "a 2");
        assert_eq!(free_name(&config, "e"), "e");
    }

    #[test]
    fn names_the_durations() {
        assert_eq!(durations(&REFRESH_CHOICES, 300), REFRESH_CHOICES);
        assert_eq!(durations(&TIMEOUT_CHOICES, 90), [30, 60, 90, 120, 300]);
        let labels = [45, 60, 90, 300, 3600, 5400_u64].map(duration_label);
        assert_eq!(labels, ["45 s", "1 min", "90 s", "5 min", "1 h", "90 min"]);
    }

    /// A stack reads back as the file has it and writes back the same; a
    /// mode is written alone, a toggle mode as on, and what is missing is
    /// said instead.
    #[test]
    fn fills_in_a_stack_and_writes_it_back() {
        let gitlab = schema::kind(SourceKind::Gitlab);
        let text = "[[sources]]\nname = \"gl\"\nkind = \"gitlab\"\n\
                    [[sources.stacks]]\nname = \"Mine\"\nenabled = false\nsort = \"oldest\"\n\
                    exclude = [\"wip\"]\nissues = \"assigned\"\nlabels = [\"bug\"]\n";
        let stack = &config(text).sources[0].stacks[0];
        let form = StackForm::of(gitlab, stack);
        assert_eq!(form.mode, Some("issues"));
        assert_eq!(form.words("labels"), ["bug"]);
        let fields = form.fields(gitlab).unwrap();
        let keys: Vec<_> = fields.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            ["name", "enabled", "issues", "labels", "sort", "exclude"]
        );
        let mut rewritten = toml::Table::new();
        rewritten.extend(fields);
        assert_eq!(
            config(&format!(
                "[[sources]]\nname = \"gl\"\nkind = \"gitlab\"\n[[sources.stacks]]\n{rewritten}"
            ))
            .sources[0]
                .stacks[0],
            *stack
        );

        let mut form = StackForm::new(gitlab);
        assert_eq!(form.fields(gitlab), Err("a stack needs a name".into()));
        form.name = " Review ".into();
        assert_eq!(
            form.fields(gitlab),
            Err("Merge requests needs a value".into())
        );
        form.set("issues", text_value("assigned"));
        form.mode = Some("todos");
        let fields = form.fields(gitlab).unwrap();
        assert_eq!(
            fields,
            [
                ("name".to_owned(), Value::String("Review".into())),
                ("todos".to_owned(), Value::Boolean(true)),
            ]
        );

        let jira = schema::kind(SourceKind::Jira);
        let mut form = StackForm::new(jira);
        form.name = "Sprint".into();
        assert_eq!(form.fields(jira), Err("JQL needs a value".into()));
        form.set("jql", text_value("   "));
        assert_eq!(form.fields(jira), Err("JQL needs a value".into()));
        assert_eq!(
            words_value(&[" a ".into(), String::new()]),
            Some(words(&["a".into()]))
        );
        assert_eq!(words_value(&[" ".into()]), None);
    }

    /// A source's page changes only the keys that differ, an empty icon
    /// removing it and a source's own interval going back to the file's;
    /// a stack reads in a line.
    #[test]
    fn changes_only_what_the_page_changed() {
        let text = "[[sources]]\nname = \"gl\"\nkind = \"gitlab\"\nrefresh_seconds = 60\n\
                    icon = \"https://example.com/a.svg\"\n\
                    [[sources.stacks]]\nname = \"Mine\"\nmerge_requests = \"review_requested\"\n\
                    [[sources.stacks]]\nname = \"Todo\"\nenabled = false\ntodos = true\n\
                    [[sources.stacks]]\nname = \"Any\"\ngroup = \"acme\"\n";
        let config = config(text);
        let source = &config.sources[0];
        let mut form = SourceForm::of(source);
        assert_eq!(form.changes(source), Ok(Vec::new()));
        form.name = " gitlab ".into();
        form.refresh_seconds = None;
        form.timeout_seconds = Some(90);
        form.icon = String::new();
        assert_eq!(
            form.changes(source),
            Ok(vec![
                ("name", Some(Value::String("gitlab".into()))),
                ("refresh_seconds", None),
                ("timeout_seconds", Some(Value::Integer(90))),
                ("icon", None),
            ])
        );
        form.board = " ".into();
        assert_eq!(form.changes(source), Err("a source needs a board".into()));

        let gitlab = schema::kind(SourceKind::Gitlab);
        let lines: Vec<_> = source
            .stacks
            .iter()
            .map(|stack| stack_summary(gitlab, stack))
            .collect();
        assert_eq!(
            lines,
            [
                "Merge requests: review requested",
                "Off · To-do items",
                "Group: acme"
            ]
        );
    }

    /// The sandbox's config is the one its board shows: it loads, and it
    /// has the board's sources, on their boards, with their stacks.
    #[test]
    fn the_sandbox_config_is_the_sandbox_board() {
        let config = check(SANDBOX_CONFIG, Path::new("config.toml")).expect("it loads");
        let snapshot = pending_core::sandbox::sandbox_snapshot();
        let shown: Vec<(String, String, Vec<String>)> = snapshot
            .boards
            .iter()
            .flat_map(|board| {
                board.groups.iter().map(|group| {
                    let stacks = group.columns.iter().map(|c| c.name.clone()).collect();
                    (board.name.clone(), group.source.clone(), stacks)
                })
            })
            .collect();
        let written: Vec<(String, String, Vec<String>)> = config
            .sources
            .iter()
            .map(|source| {
                let stacks = source.stacks.iter().map(|s| s.name.clone()).collect();
                (source.board.clone(), source.name.clone(), stacks)
            })
            .collect();
        assert_eq!(written, shown);
        let env = EnvFile::parse(SANDBOX_ENV);
        assert_eq!(env.keys().len(), 3);
    }
}
