//! What Settings offers for each kind of source is what that kind's stacks
//! accept: every key loads with every value Settings can fill in, and no
//! key is missing.

use std::collections::BTreeSet;
use std::path::Path;

use pending_core::schema::{Field, Input, KINDS, Kind};
use pending_runtime::config::{LoadError, check};
use toml::{Table, Value};

/// What Settings would write for `field`: one per word it can choose.
fn values(field: &Field) -> Vec<Value> {
    let word = |word: &str| Value::String(word.to_owned());
    match field.input {
        Input::Text => vec![word("acme/api")],
        Input::Choice(words) => words.iter().map(|w| word(w)).collect(),
        Input::Choices(words) => vec![Value::Array(words.iter().map(|w| word(w)).collect())],
        Input::Words => vec![Value::Array(vec![word("acme/api")])],
        Input::Toggle => vec![Value::Boolean(true)],
    }
}

/// A stack of `kind` with `keys` and whatever it cannot go without.
fn stack(kind: &Kind, keys: &[(&str, Value)]) -> Table {
    let mut stack = Table::new();
    stack.insert("name".to_owned(), Value::String("Test".to_owned()));
    let mut fill = |field: &Field| {
        let first = values(field).remove(0);
        stack.insert(field.key.to_owned(), first);
    };
    let has_mode = keys
        .iter()
        .any(|(key, _)| kind.modes.iter().any(|m| m.key == *key));
    if let Some(mode) = kind.modes.first().filter(|_| !has_mode) {
        fill(mode);
    }
    kind.fields
        .iter()
        .filter(|f| f.required)
        .for_each(&mut fill);
    for (key, value) in keys {
        stack.insert((*key).to_owned(), value.clone());
    }
    stack
}

fn checked(kind: &Kind, stack: Table) -> Result<(), LoadError> {
    let mut source = Table::new();
    source.insert("name".to_owned(), Value::String("src".to_owned()));
    source.insert("kind".to_owned(), Value::String(kind.id.to_owned()));
    source.insert("stacks".to_owned(), Value::Array(vec![Value::Table(stack)]));
    let mut config = Table::new();
    config.insert(
        "sources".to_owned(),
        Value::Array(vec![Value::Table(source)]),
    );
    let text = toml::to_string(&config).expect("a config");
    check(&text, Path::new("/somewhere/config.toml")).map(drop)
}

#[test]
fn every_key_settings_offers_loads() {
    for kind in KINDS.iter().filter(|kind| kind.id != "sample") {
        for field in kind.modes.iter().chain(kind.fields) {
            for value in values(field) {
                let stack = stack(kind, &[(field.key, value.clone())]);
                if let Err(error) = checked(kind, stack) {
                    panic!("{} `{}` = {value}: {error}", kind.id, field.key);
                }
            }
        }
    }
}

#[test]
fn a_mode_is_one_of_several() {
    for kind in KINDS.iter().filter(|kind| kind.modes.len() > 1) {
        let mut both = stack(kind, &[]);
        let second = &kind.modes[1];
        both.insert(second.key.to_owned(), values(second).remove(0));
        assert!(checked(kind, both).is_err(), "{}: two modes load", kind.id);
    }
}

#[test]
fn a_required_key_is_required() {
    for kind in KINDS {
        for field in kind.fields.iter().filter(|f| f.required) {
            let mut without = stack(kind, &[]);
            without.remove(field.key);
            assert!(
                checked(kind, without).is_err(),
                "{} loads without `{}`",
                kind.id,
                field.key
            );
        }
    }
}

/// The keys a kind's stacks take, as the error for an unknown one lists them.
fn accepted(kind: &Kind) -> BTreeSet<String> {
    let stack = stack(kind, &[("unheard_of", Value::Boolean(true))]);
    let message = match checked(kind, stack) {
        Err(LoadError::Column { message, .. }) => message,
        other => panic!("{}: {other:?}", kind.id),
    };
    let (_, expected) = message
        .split_once("expected")
        .unwrap_or_else(|| panic!("{}: {message}", kind.id));
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// Each variable Settings writes is one its kind reads.
#[test]
fn every_variable_is_read() {
    let code = |id: &str| match id {
        "github" => include_str!("../../pending-github/src/lib.rs"),
        "gitlab" => include_str!("../../pending-gitlab/src/lib.rs"),
        "ical" => include_str!("../../pending-ical/src/lib.rs"),
        "jira" => include_str!("../../pending-jira/src/lib.rs"),
        "linear" => include_str!("../../pending-linear/src/lib.rs"),
        "plane" => include_str!("../../pending-plane/src/lib.rs"),
        "todoist" => include_str!("../../pending-todoist/src/lib.rs"),
        _ => "",
    };
    for kind in KINDS {
        for name in kind.vars.iter().flat_map(|var| var.names) {
            assert!(
                code(kind.id).contains(&format!("\"{name}\"")),
                "{} does not read {name}",
                kind.id
            );
        }
    }
}

#[test]
fn settings_offers_every_key() {
    for kind in KINDS.iter().filter(|kind| kind.id != "sample") {
        let offered: BTreeSet<String> = kind
            .modes
            .iter()
            .chain(kind.fields)
            .map(|field| field.key.to_owned())
            .collect();
        assert_eq!(offered, accepted(kind), "{}", kind.id);
    }
}
