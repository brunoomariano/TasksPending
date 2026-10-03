//! Per-stack `exclude` patterns: the cards a stack leaves out.
//!
//! Patterns are compiled once, when the config loads, and applied to each
//! batch as it arrives (from a refresh or from the cache), so building a
//! snapshot never runs a regular expression.

use std::collections::{BTreeMap, HashSet};

use pending_core::{SourceBatch, StackConfig};
use regex::{Regex, RegexBuilder};
use thiserror::Error;

/// A stack's `exclude` list has a pattern that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("stack `{stack}`: {message}")]
pub struct ExcludeError {
    pub stack: String,
    pub message: String,
}

/// The compiled `exclude` patterns of one source, by stack name.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    stacks: BTreeMap<String, Vec<Regex>>,
}

impl Excludes {
    /// Compiles every stack's patterns as case-insensitive regular
    /// expressions. Disabled stacks are checked too, then left out.
    pub fn from_stacks(stacks: &[StackConfig]) -> Result<Self, ExcludeError> {
        let mut compiled = BTreeMap::new();
        for stack in stacks {
            let patterns = stack
                .exclude
                .iter()
                .map(|pattern| compile(pattern))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|message| ExcludeError {
                    stack: stack.name.clone(),
                    message,
                })?;
            if stack.enabled && !patterns.is_empty() {
                compiled.insert(stack.name.clone(), patterns);
            }
        }
        Ok(Self { stacks: compiled })
    }

    /// Removes from each stack the cards whose title or body matches one of
    /// that stack's patterns. Returns the ids of the cards this left in no
    /// stack at all: still pending at the source, just not shown.
    pub fn apply(&self, batch: &mut SourceBatch) -> HashSet<String> {
        let mut hidden = HashSet::new();
        if self.stacks.is_empty() {
            return hidden;
        }
        batch.items.retain(|item| {
            let excluded = self.stacks.get(&item.column).is_some_and(|patterns| {
                patterns.iter().any(|pattern| {
                    pattern.is_match(&item.card.title) || pattern.is_match(&item.card.body)
                })
            });
            if excluded {
                hidden.insert(item.card.id.clone());
            }
            !excluded
        });
        for item in &batch.items {
            hidden.remove(&item.card.id);
        }
        hidden
    }
}

fn compile(pattern: &str) -> Result<Regex, String> {
    // It would match every card and empty the stack; `enabled = false` is
    // the way to switch a stack off.
    if pattern.trim().is_empty() {
        return Err("an `exclude` pattern is empty".to_owned());
    }
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|error| {
            // The regex crate explains syntax errors over several lines,
            // ending with the reason; the config error is a single line.
            let error = error.to_string();
            let reason = error.lines().last().unwrap_or_default().trim();
            let reason = reason.strip_prefix("error: ").unwrap_or(reason);
            format!("invalid `exclude` pattern `{pattern}`: {reason}")
        })
}
