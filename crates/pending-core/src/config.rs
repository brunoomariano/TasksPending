use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// Default wait between refreshes of a source.
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    /// A refresh slower than this counts as a failure.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub name: String,
    pub kind: SourceKind,
    /// Area (tab) that shows this source's columns. `lane` is the old name.
    #[serde(default = "default_board", alias = "lane")]
    pub board: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub refresh_seconds: Option<u64>,
    /// Overrides the global `timeout_seconds` for this source.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Columns (filters) of this source; empty means the source's defaults.
    /// Filter keys depend on `kind` and are checked when the source is built.
    #[serde(default)]
    pub columns: Vec<ColumnConfig>,
}

/// One kanban column of a source: a name plus kind-specific filter keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnConfig {
    pub name: String,
    #[serde(flatten)]
    pub filter: toml::Table,
}

impl Eq for ColumnConfig {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Github,
    Google,
    Ical,
    Plane,
    Sample,
    Todoist,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("source name `{0}` is used more than once")]
    DuplicateSourceName(String),
    #[error("`{0}` must be greater than zero")]
    Zero(String),
    #[error("source `{source_name}` has more than one column named `{column}`")]
    DuplicateColumnName { source_name: String, column: String },
    #[error("source `{0}` has a column without a name")]
    EmptyColumnName(String),
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            refresh_seconds: default_refresh_seconds(),
            timeout_seconds: default_timeout_seconds(),
            sources: Vec::new(),
        }
    }
}

impl AppConfig {
    /// Checks invariants serde cannot express.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.refresh_seconds == 0 {
            return Err(ConfigError::Zero("refresh_seconds".to_owned()));
        }
        if self.timeout_seconds == 0 {
            return Err(ConfigError::Zero("timeout_seconds".to_owned()));
        }

        let mut names = HashSet::new();
        for source in &self.sources {
            if !names.insert(source.name.as_str()) {
                return Err(ConfigError::DuplicateSourceName(source.name.clone()));
            }
            let mut columns = HashSet::new();
            for column in &source.columns {
                if column.name.trim().is_empty() {
                    return Err(ConfigError::EmptyColumnName(source.name.clone()));
                }
                if !columns.insert(column.name.trim()) {
                    return Err(ConfigError::DuplicateColumnName {
                        source_name: source.name.clone(),
                        column: column.name.clone(),
                    });
                }
            }
            for (key, value) in [
                ("refresh_seconds", source.refresh_seconds),
                ("timeout_seconds", source.timeout_seconds),
            ] {
                if value == Some(0) {
                    return Err(ConfigError::Zero(format!("sources.{}.{key}", source.name)));
                }
            }
        }
        Ok(())
    }

    pub fn refresh_seconds_for(&self, source: &SourceConfig) -> u64 {
        source.refresh_seconds.unwrap_or(self.refresh_seconds)
    }
}

fn default_refresh_seconds() -> u64 {
    300
}

/// Room for the slowest source: Plane reads the user, the project list and
/// then each project (up to 20 s each) before a refresh completes.
fn default_timeout_seconds() -> u64 {
    60
}

pub const DEFAULT_BOARD: &str = "Inbox";

fn default_board() -> String {
    DEFAULT_BOARD.to_owned()
}

fn default_enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_five_minute_refresh() {
        let config = AppConfig::default();
        assert_eq!(config.refresh_seconds, 300);
        assert!(config.sources.is_empty());
    }

    #[test]
    fn source_is_enabled_by_default() {
        let source: SourceConfig = toml::from_str(
            r#"
            name = "github"
            kind = "github"
            "#,
        )
        .expect("valid source config");

        assert!(source.enabled);
        assert_eq!(source.kind, SourceKind::Github);
        assert_eq!(source.refresh_seconds, None);
    }

    /// A typo in the source kind must fail loading the config, not become a
    /// source that never produces cards.
    #[test]
    fn unknown_source_kind_is_rejected() {
        let error = toml::from_str::<SourceConfig>(
            r#"
            name = "typo"
            kind = "gihtub"
            "#,
        )
        .expect_err("unknown kind");

        assert!(error.to_string().contains("gihtub"), "{error}");
    }

    /// Each source feeds a lane defined in the config; without an explicit
    /// lane, cards go to the "Inbox" lane.
    #[test]
    fn source_lane_comes_from_config_with_inbox_default() {
        let sources: AppConfig = toml::from_str(
            r#"
            [[sources]]
            name = "github"
            kind = "github"
            lane = "Work"

            [[sources]]
            name = "sample"
            kind = "sample"
            "#,
        )
        .expect("valid config");

        assert_eq!(sources.sources[0].board, "Work");
        assert_eq!(sources.sources[1].board, "Inbox");
    }

    fn parse(text: &str) -> AppConfig {
        toml::from_str(text).expect("valid toml")
    }

    /// Source health is keyed by name; two equal names would make the
    /// dashboard ambiguous, so the config is rejected naming the duplicate.
    #[test]
    fn duplicate_source_names_are_rejected() {
        let config = parse(
            r#"
            [[sources]]
            name = "github"
            kind = "github"

            [[sources]]
            name = "github"
            kind = "sample"
            "#,
        );

        let error = config.validate().expect_err("duplicate names");
        assert!(error.to_string().contains("github"), "{error}");
    }

    /// A zero interval or timeout would make the aggregator poll the source
    /// nonstop or never allow a response; the config is rejected.
    #[test]
    fn zero_intervals_and_timeouts_are_rejected() {
        for text in [
            "refresh_seconds = 0",
            "timeout_seconds = 0",
            "[[sources]]\nname = \"s\"\nkind = \"sample\"\nrefresh_seconds = 0",
            "[[sources]]\nname = \"s\"\nkind = \"sample\"\ntimeout_seconds = 0",
        ] {
            let error = parse(text).validate().expect_err(text);
            assert!(
                error.to_string().contains("must be greater than zero"),
                "{error}"
            );
        }
    }

    /// Each source uses its own interval when set, otherwise the global one;
    /// the refresh timeout defaults to 60 seconds.
    #[test]
    fn source_interval_falls_back_to_the_global_refresh() {
        let config = parse(
            r#"
            refresh_seconds = 120

            [[sources]]
            name = "fast"
            kind = "sample"
            refresh_seconds = 30
            timeout_seconds = 180

            [[sources]]
            name = "default"
            kind = "sample"
            "#,
        );

        config.validate().expect("valid config");
        assert_eq!(config.refresh_seconds_for(&config.sources[0]), 30);
        assert_eq!(config.refresh_seconds_for(&config.sources[1]), 120);
        assert_eq!(config.timeout_seconds, 60);
        assert_eq!(config.sources[0].timeout_seconds, Some(180));
        assert_eq!(config.sources[1].timeout_seconds, None);
    }

    /// A typo in a global key is rejected instead of ignored.
    #[test]
    fn unknown_top_level_keys_are_rejected() {
        let error = toml::from_str::<AppConfig>("refresh_secs = 10").expect_err("typo");
        assert!(error.to_string().contains("refresh_secs"), "{error}");
    }

    /// A typo inside a source (e.g. `enable` instead of `enabled`) is rejected
    /// naming the key, instead of enabling the source with the default value.
    #[test]
    fn unknown_source_keys_are_rejected() {
        let error = toml::from_str::<AppConfig>(
            r#"
            [[sources]]
            name = "github"
            kind = "github"
            enable = false
            "#,
        )
        .expect_err("typo inside a source");
        assert!(error.to_string().contains("enable"), "{error}");
    }
}
