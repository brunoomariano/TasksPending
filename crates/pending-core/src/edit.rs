//! Changing the config file the way Settings does: adding, removing,
//! moving and rewriting sources and stacks, and the keys of the file
//! itself, while the comments, the layout and the order the file was
//! given stay as they were. What a change means is checked elsewhere,
//! with the same code that loads the file; this only writes it.

use thiserror::Error;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

/// Keys and their values, in the order they are to be written.
pub type Fields = Vec<(String, toml::Value)>;

/// Where the sources are.
const SOURCES: &str = "sources";
/// A source's stacks, under their name or the old one.
const STACKS: [&str; 2] = ["stacks", "columns"];
/// A source's board, under its name or the old one.
const BOARD: [&str; 2] = ["board", "lane"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("{0}")]
    Parse(String),
    #[error("there is no source {0}")]
    NoSource(usize),
    #[error("source {0} has no stack {1}")]
    NoStack(usize, usize),
    #[error("`{0}` is not a list of tables")]
    Shape(&'static str),
}

/// The config file as written, ready to be changed.
#[derive(Clone, Debug)]
pub struct ConfigDoc {
    doc: DocumentMut,
}

impl ConfigDoc {
    /// The file `text`; an empty text is an empty file.
    pub fn parse(text: &str) -> Result<Self, EditError> {
        text.parse::<DocumentMut>()
            .map(|doc| Self { doc })
            .map_err(|error| EditError::Parse(error.to_string()))
    }

    /// The file as it is now.
    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Sets the file's own `key` (`refresh_seconds`, `timeout_seconds`), or
    /// removes it, so its default applies.
    pub fn set_global(&mut self, key: &str, value: Option<toml::Value>) {
        set(self.doc.as_table_mut(), key, value);
    }

    /// How many sources the file lists.
    pub fn sources(&self) -> usize {
        match self.doc.get(SOURCES) {
            Some(Item::ArrayOfTables(sources)) => sources.len(),
            Some(Item::Value(Value::Array(sources))) => sources.len(),
            _ => 0,
        }
    }

    /// Adds a source with `fields` after the others; says where it is.
    pub fn add_source(&mut self, fields: &Fields) -> Result<usize, EditError> {
        let sources = self.sources_mut()?;
        let mut table = table_of(fields, "");
        table.decor_mut().set_prefix("\n");
        sources.push(table);
        let index = sources.len() - 1;
        self.renumber();
        Ok(index)
    }

    pub fn remove_source(&mut self, source: usize) -> Result<(), EditError> {
        let sources = self.sources_mut()?;
        if source >= sources.len() {
            return Err(EditError::NoSource(source));
        }
        sources.remove(source);
        self.renumber();
        Ok(())
    }

    /// Moves the source at `from` to `to`, with its stacks and the comments
    /// above it.
    pub fn move_source(&mut self, from: usize, to: usize) -> Result<(), EditError> {
        let sources = self.sources_mut()?;
        for index in [from, to] {
            if index >= sources.len() {
                return Err(EditError::NoSource(index));
            }
        }
        let table = sources.remove(from);
        sources.insert(to, table);
        self.renumber();
        Ok(())
    }

    /// Swaps the sources at `a` and `b`, with their stacks and comments;
    /// the sources between them stay where they are.
    pub fn swap_sources(&mut self, a: usize, b: usize) -> Result<(), EditError> {
        let (low, high) = (a.min(b), a.max(b));
        self.move_source(high, low)?;
        self.move_source(low + 1, high)
    }

    /// Sets a source's `key`, or removes it so its default applies. `board`
    /// is written under the old name, `lane`, where the file uses it.
    pub fn set_source(
        &mut self,
        source: usize,
        key: &str,
        value: Option<toml::Value>,
    ) -> Result<(), EditError> {
        let table = self.source_mut(source)?;
        let key = if BOARD.contains(&key) {
            BOARD
                .into_iter()
                .find(|name| table.contains_key(name))
                .unwrap_or(key)
        } else {
            key
        };
        set(table, key, value);
        Ok(())
    }

    /// How many stacks the source lists.
    pub fn stacks(&self, source: usize) -> Result<usize, EditError> {
        let table = self.source(source)?;
        Ok(STACKS
            .into_iter()
            .find_map(|key| match table.get(key) {
                Some(Item::ArrayOfTables(stacks)) => Some(stacks.len()),
                Some(Item::Value(Value::Array(stacks))) => Some(stacks.len()),
                _ => None,
            })
            .unwrap_or(0))
    }

    /// Adds a stack with `fields` after the source's others, indented as
    /// they are (as the file's other stacks are, for a source with none);
    /// says where it is.
    pub fn add_stack(&mut self, source: usize, fields: &Fields) -> Result<usize, EditError> {
        let model = self
            .first_stack(source)
            .or_else(|| (0..self.sources()).find_map(|other| self.first_stack(other)));
        let (header, keys) = model.as_ref().map(indentation).unwrap_or_default();
        let stacks = self.stacks_mut(source)?;
        let mut table = table_of(fields, &keys);
        table.decor_mut().set_prefix(format!("\n{header}"));
        stacks.push(table);
        let index = stacks.len() - 1;
        self.renumber();
        Ok(index)
    }

    pub fn remove_stack(&mut self, source: usize, stack: usize) -> Result<(), EditError> {
        let stacks = self.stacks_mut(source)?;
        if stack >= stacks.len() {
            return Err(EditError::NoStack(source, stack));
        }
        stacks.remove(stack);
        self.renumber();
        Ok(())
    }

    /// Moves the source's stack at `from` to `to`.
    pub fn move_stack(&mut self, source: usize, from: usize, to: usize) -> Result<(), EditError> {
        let stacks = self.stacks_mut(source)?;
        for stack in [from, to] {
            if stack >= stacks.len() {
                return Err(EditError::NoStack(source, stack));
            }
        }
        let table = stacks.remove(from);
        stacks.insert(to, table);
        self.renumber();
        Ok(())
    }

    /// Sets a stack's `key`, or removes it.
    pub fn set_stack(
        &mut self,
        source: usize,
        stack: usize,
        key: &str,
        value: Option<toml::Value>,
    ) -> Result<(), EditError> {
        set(self.stack_mut(source, stack)?, key, value);
        Ok(())
    }

    /// Rewrites a stack to hold `fields` and nothing else. The keys it keeps
    /// stay where they were, with their comments; new ones go at the end.
    pub fn replace_stack(
        &mut self,
        source: usize,
        stack: usize,
        fields: &Fields,
    ) -> Result<(), EditError> {
        let table = self.stack_mut(source, stack)?;
        let gone: Vec<String> = table
            .iter()
            .map(|(key, _)| key.to_owned())
            .filter(|key| !fields.iter().any(|(name, _)| name == key))
            .collect();
        for key in gone {
            table.remove(&key);
        }
        for (key, value) in fields {
            set(table, key, Some(value.clone()));
        }
        Ok(())
    }

    /// The source's first stack, written as a table.
    fn first_stack(&self, source: usize) -> Option<Table> {
        let table = self.source(source).ok()?;
        STACKS.into_iter().find_map(|key| match table.get(key) {
            Some(Item::ArrayOfTables(stacks)) => stacks.get(0).cloned(),
            _ => None,
        })
    }

    fn source(&self, source: usize) -> Result<&Table, EditError> {
        match self.doc.get(SOURCES) {
            Some(Item::ArrayOfTables(sources)) => sources.get(source),
            _ => None,
        }
        .ok_or(EditError::NoSource(source))
    }

    fn source_mut(&mut self, source: usize) -> Result<&mut Table, EditError> {
        self.sources_mut()?
            .get_mut(source)
            .ok_or(EditError::NoSource(source))
    }

    /// The sources as a list of tables, made one when the file wrote them
    /// as a list of inline tables or had none.
    fn sources_mut(&mut self) -> Result<&mut ArrayOfTables, EditError> {
        tables_in(self.doc.as_table_mut(), SOURCES, SOURCES)
    }

    fn stacks_mut(&mut self, source: usize) -> Result<&mut ArrayOfTables, EditError> {
        let table = self.source_mut(source)?;
        let key = STACKS
            .into_iter()
            .find(|key| table.contains_key(key))
            .unwrap_or(STACKS[0]);
        tables_in(table, key, "stacks")
    }

    fn stack_mut(&mut self, source: usize, stack: usize) -> Result<&mut Table, EditError> {
        self.stacks_mut(source)?
            .get_mut(stack)
            .ok_or(EditError::NoStack(source, stack))
    }

    /// Puts every table where its place in the lists says. The file is
    /// written in the order of these positions, and a table added without
    /// one would be written last: a stack there would belong to the last
    /// source instead of its own.
    fn renumber(&mut self) {
        let Some(Item::ArrayOfTables(sources)) = self.doc.get_mut(SOURCES) else {
            return;
        };
        let mut next = 1;
        for source in sources.iter_mut() {
            source.set_position(Some(next));
            next += 1;
            for key in STACKS {
                if let Some(Item::ArrayOfTables(stacks)) = source.get_mut(key) {
                    for stack in stacks.iter_mut() {
                        stack.set_position(Some(next));
                        next += 1;
                    }
                }
            }
        }
    }
}

/// The list of tables at `key` in `table`, made one when it is a list of
/// inline tables or missing; `name` names it in the error otherwise.
fn tables_in<'t>(
    table: &'t mut Table,
    key: &str,
    name: &'static str,
) -> Result<&'t mut ArrayOfTables, EditError> {
    let item = table
        .entry(key)
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()));
    if let Item::Value(Value::Array(_)) = item {
        let taken = std::mem::take(item);
        let tables = taken
            .into_array_of_tables()
            .map_err(|_| EditError::Shape(name))?;
        *item = Item::ArrayOfTables(tables);
    }
    item.as_array_of_tables_mut().ok_or(EditError::Shape(name))
}

/// Sets `key` in `table` to `value`, keeping the comment and spacing around
/// the value it replaces, or removes the key. A new key is indented like
/// the table's first one.
fn set(table: &mut Table, key: &str, value: Option<toml::Value>) {
    let Some(value) = value else {
        table.remove(key);
        return;
    };
    let mut new = to_edit(&value);
    match table.get_mut(key) {
        Some(item) => {
            if let Some(old) = item.as_value() {
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            let (_, indent) = indentation(table);
            table.insert(key, Item::Value(new));
            if let Some(mut key) = table.key_mut(key) {
                key.leaf_decor_mut().set_prefix(indent);
            }
        }
    }
}

/// A table holding `fields`, each key indented with `indent`.
fn table_of(fields: &Fields, indent: &str) -> Table {
    let mut table = Table::new();
    for (key, value) in fields {
        table.insert(key, Item::Value(to_edit(value)));
        if let Some(mut key) = table.key_mut(key) {
            key.leaf_decor_mut().set_prefix(indent);
        }
    }
    table
}

/// How `table` is indented: before its header, and before its first key.
fn indentation(table: &Table) -> (String, String) {
    let header = whitespace(table.decor().prefix())
        .map(|prefix| prefix.rsplit('\n').next().unwrap_or_default().to_owned())
        .unwrap_or_default();
    let keys = table
        .iter()
        .next()
        .and_then(|(key, _)| table.key(key))
        .and_then(|key| whitespace(key.leaf_decor().prefix()))
        .unwrap_or_default();
    (header, keys)
}

/// The raw text of a decor, when it is only spacing (not a comment).
fn whitespace(raw: Option<&toml_edit::RawString>) -> Option<String> {
    raw.and_then(toml_edit::RawString::as_str)
        .filter(|text| text.chars().all(char::is_whitespace))
        .map(str::to_owned)
}

fn to_edit(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::from(text.as_str()),
        toml::Value::Integer(number) => Value::from(*number),
        toml::Value::Float(number) => Value::from(*number),
        toml::Value::Boolean(yes) => Value::from(*yes),
        toml::Value::Datetime(when) => when
            .to_string()
            .parse::<Value>()
            .unwrap_or_else(|_| Value::from(when.to_string())),
        toml::Value::Array(values) => Value::Array(values.iter().map(to_edit).collect()),
        toml::Value::Table(table) => Value::InlineTable(
            table
                .iter()
                .map(|(key, value)| (key.as_str(), to_edit(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    const FILE: &str = r#"# My sources.
refresh_seconds = 300 # five minutes
timeout_seconds = 60

# Work first.
[[sources]]
name = "github" # the work account
kind = "github"
board = "Work"

  [[sources.stacks]]
  name = "Review requested"
  query = "is:open is:pr review-requested:@me" # mine to review
  severity = "warning"

  [[sources.stacks]]
  name = "Drafts"
  query = "is:open is:pr author:@me draft:true"

# Then home.
[[sources]]
name = "todoist"
kind = "todoist"
board = "Home"

  [[sources.stacks]]
  name = "Today"
  filter = "today | overdue"

# The end.
"#;

    fn string(text: &str) -> toml::Value {
        toml::Value::String(text.to_owned())
    }

    fn fields(pairs: &[(&str, toml::Value)]) -> Fields {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    /// What the file means, as TasksPending loads it.
    fn loaded(doc: &ConfigDoc) -> AppConfig {
        toml::from_str(&doc.text()).unwrap_or_else(|error| panic!("{error}\n{}", doc.text()))
    }

    fn names(config: &AppConfig) -> Vec<(String, Vec<String>)> {
        config
            .sources
            .iter()
            .map(|source| {
                let stacks = source.stacks.iter().map(|stack| stack.name.clone());
                (source.name.clone(), stacks.collect())
            })
            .collect()
    }

    fn owned(names: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        names
            .iter()
            .map(|(source, stacks)| {
                let stacks = stacks.iter().map(|stack| (*stack).to_owned());
                ((*source).to_owned(), stacks.collect())
            })
            .collect()
    }

    /// The lines of `after` that are not in `before`, and the reverse.
    fn diff(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
        let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        let (before, after) = (lines(before), lines(after));
        (
            after
                .iter()
                .filter(|l| !before.contains(l))
                .cloned()
                .collect(),
            before
                .iter()
                .filter(|l| !after.contains(l))
                .cloned()
                .collect(),
        )
    }

    /// Read and written back, the file is the same, byte for byte.
    #[test]
    fn leaves_the_file_as_it_was() {
        assert_eq!(ConfigDoc::parse(FILE).unwrap().text(), FILE);
        assert_eq!(ConfigDoc::parse("").unwrap().text(), "");
    }

    /// A value changes in its line, keeping the comment after it; a key set
    /// to nothing goes, and its default applies.
    #[test]
    fn changes_the_files_own_keys() {
        let mut doc = ConfigDoc::parse(FILE).unwrap();
        doc.set_global("refresh_seconds", Some(toml::Value::Integer(60)));
        doc.set_global("timeout_seconds", None);
        let (added, removed) = diff(FILE, &doc.text());
        assert_eq!(added, ["refresh_seconds = 60 # five minutes"]);
        assert_eq!(
            removed,
            [
                "refresh_seconds = 300 # five minutes",
                "timeout_seconds = 60"
            ]
        );
        let config = loaded(&doc);
        assert_eq!(
            (config.refresh_seconds, config.timeout_seconds),
            (60, AppConfig::default().timeout_seconds)
        );
    }

    /// A source added goes after the others; a stack added goes under its
    /// own source, indented like its other stacks, even when another source
    /// follows.
    #[test]
    fn adds_sources_and_stacks_where_they_belong() {
        let mut doc = ConfigDoc::parse(FILE).unwrap();
        let stack = doc
            .add_stack(
                0,
                &fields(&[("name", string("Mine")), ("query", string("is:pr"))]),
            )
            .unwrap();
        assert_eq!(stack, 2);
        let source = doc
            .add_source(&fields(&[
                ("name", string("plane")),
                ("kind", string("plane")),
                ("board", string("Work")),
            ]))
            .unwrap();
        assert_eq!(source, 2);
        doc.add_stack(2, &fields(&[("name", string("Assigned to me"))]))
            .unwrap();
        assert_eq!(
            names(&loaded(&doc)),
            owned(&[
                ("github", &["Review requested", "Drafts", "Mine"]),
                ("todoist", &["Today"]),
                ("plane", &["Assigned to me"]),
            ])
        );
        let text = doc.text();
        assert!(
            text.contains("\n  [[sources.stacks]]\n  name = \"Mine\"\n  query = \"is:pr\"\n"),
            "{text}"
        );
        assert!(
            text.contains("\n  [[sources.stacks]]\n  name = \"Assigned to me\"\n"),
            "a source's first stack is indented like the file's others: {text}"
        );
        assert_eq!(diff(FILE, &text).1, Vec::<String>::new(), "nothing lost");
        assert!(text.ends_with("# The end.\n"), "{text}");
        assert_eq!(
            (doc.sources(), doc.stacks(0), doc.stacks(2)),
            (3, Ok(3), Ok(1))
        );
    }

    /// Sources and stacks move with what is written above them, and go
    /// when removed; the other sources keep their stacks.
    #[test]
    fn moves_and_removes_sources_and_stacks() {
        let mut doc = ConfigDoc::parse(FILE).unwrap();
        doc.move_source(1, 0).unwrap();
        doc.move_stack(1, 1, 0).unwrap();
        assert_eq!(
            names(&loaded(&doc)),
            owned(&[
                ("todoist", &["Today"]),
                ("github", &["Drafts", "Review requested"]),
            ])
        );
        let text = doc.text();
        let home = text.find("# Then home.").unwrap();
        let work = text.find("# Work first.").unwrap();
        assert!(home < work, "{text}");
        assert!(text.contains("# mine to review"), "{text}");

        doc.remove_stack(1, 0).unwrap();
        doc.remove_source(0).unwrap();
        assert_eq!(
            names(&loaded(&doc)),
            owned(&[("github", &["Review requested"])])
        );
        assert!(!doc.text().contains("# Then home."));
        assert_eq!(doc.remove_source(1), Err(EditError::NoSource(1)));
        assert_eq!(doc.move_source(0, 1), Err(EditError::NoSource(1)));
        assert_eq!(doc.remove_stack(0, 1), Err(EditError::NoStack(0, 1)));
    }

    /// Two sources swap places around the ones between them, which stay
    /// where they are.
    #[test]
    fn swaps_sources_around_the_others() {
        let text: String = ["a", "b", "c", "d"]
            .iter()
            .map(|name| format!("[[sources]]\nname = \"{name}\"\nkind = \"sample\"\n\n"))
            .collect();
        let order = |doc: &ConfigDoc| -> Vec<String> {
            names(&loaded(doc))
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        };
        let mut doc = ConfigDoc::parse(&text).unwrap();
        doc.swap_sources(0, 2).unwrap();
        assert_eq!(order(&doc), ["c", "b", "a", "d"]);
        doc.swap_sources(3, 1).unwrap();
        assert_eq!(order(&doc), ["c", "d", "a", "b"]);
        doc.swap_sources(2, 3).unwrap();
        assert_eq!(order(&doc), ["c", "d", "b", "a"]);
        assert_eq!(doc.swap_sources(0, 4), Err(EditError::NoSource(4)));
    }

    /// A source's and a stack's keys change in place, comments kept; a
    /// stack rewritten keeps only the keys given, the kept ones where they
    /// were and the new ones last.
    #[test]
    fn rewrites_sources_and_stacks() {
        let mut doc = ConfigDoc::parse(FILE).unwrap();
        doc.set_source(0, "name", Some(string("work"))).unwrap();
        doc.set_source(0, "enabled", Some(toml::Value::Boolean(false)))
            .unwrap();
        doc.set_stack(0, 1, "sort", Some(string("oldest"))).unwrap();
        doc.replace_stack(
            0,
            0,
            &fields(&[
                ("name", string("To review")),
                ("query", string("is:open review-requested:@me")),
                ("exclude", toml::Value::Array(vec![string("bot")])),
            ]),
        )
        .unwrap();
        let (added, removed) = diff(FILE, &doc.text());
        assert_eq!(
            added,
            [
                "name = \"work\" # the work account",
                "enabled = false",
                "  name = \"To review\"",
                "  query = \"is:open review-requested:@me\" # mine to review",
                "  exclude = [\"bot\"]",
                "  sort = \"oldest\"",
            ]
        );
        assert_eq!(
            removed,
            [
                "name = \"github\" # the work account",
                "  name = \"Review requested\"",
                "  query = \"is:open is:pr review-requested:@me\" # mine to review",
                "  severity = \"warning\"",
            ]
        );
        let config = loaded(&doc);
        assert!(!config.sources[0].enabled);
        assert_eq!(config.sources[0].stacks[0].exclude, ["bot"]);
        assert_eq!(
            doc.set_stack(0, 5, "name", None),
            Err(EditError::NoStack(0, 5))
        );
        assert_eq!(doc.set_source(4, "name", None), Err(EditError::NoSource(4)));
    }

    /// Files that use the old names, `lane` and `columns`, keep them.
    #[test]
    fn keeps_the_old_names() {
        let old = "[[sources]]\nname = \"a\"\nkind = \"todoist\"\nlane = \"Old\"\n\n\
                   [[sources.columns]]\nname = \"One\"\nfilter = \"today\"\n";
        let mut doc = ConfigDoc::parse(old).unwrap();
        doc.set_source(0, "board", Some(string("New"))).unwrap();
        doc.add_stack(
            0,
            &fields(&[("name", string("Two")), ("filter", string("7 days"))]),
        )
        .unwrap();
        let text = doc.text();
        assert!(text.contains("lane = \"New\""), "{text}");
        assert!(!text.contains("board"), "{text}");
        assert!(!text.contains("[[sources.stacks]]"), "{text}");
        let config = loaded(&doc);
        assert_eq!(config.sources[0].board, "New");
        assert_eq!(names(&config), owned(&[("a", &["One", "Two"])]));
    }

    /// An empty file takes its first source; sources written as a list of
    /// inline tables become tables; something else is said to be wrong.
    #[test]
    fn starts_files_and_fixes_their_shape() {
        let mut doc = ConfigDoc::parse("").unwrap();
        doc.add_source(&fields(&[
            ("name", string("s")),
            ("kind", string("sample")),
        ]))
        .unwrap();
        assert_eq!(names(&loaded(&doc)), owned(&[("s", &[])]));

        let inline = "sources = [{ name = \"a\", kind = \"todoist\" }]\n";
        let mut doc = ConfigDoc::parse(inline).unwrap();
        doc.add_stack(
            0,
            &fields(&[("name", string("T")), ("filter", string("today"))]),
        )
        .unwrap();
        assert_eq!(names(&loaded(&doc)), owned(&[("a", &["T"])]));

        let mut doc = ConfigDoc::parse("sources = 3\n").unwrap();
        assert_eq!(doc.sources(), 0);
        assert_eq!(
            doc.add_source(&fields(&[("name", string("s"))])),
            Err(EditError::Shape("sources"))
        );
        assert!(matches!(
            ConfigDoc::parse("[[sources]\n"),
            Err(EditError::Parse(_))
        ));
    }
}
