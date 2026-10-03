use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::Icon;
use crate::snapshot::is_http_url;

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
    /// Image URL shown next to the source's name, e.g. from
    /// https://dashboardicons.com.
    #[serde(default)]
    pub icon: Option<String>,
    /// Variant of `icon` for dark themes.
    #[serde(default)]
    pub icon_dark: Option<String>,
    /// Stacks (filters) of this source, top to bottom; empty means the
    /// source's defaults. Filter keys depend on `kind` and are checked when
    /// the source is built. `columns` is the old name.
    #[serde(default, alias = "columns")]
    pub stacks: Vec<StackConfig>,
}

/// One stack of a source's cards: a name, the order of its cards, the cards
/// it leaves out, plus kind-specific filter keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackConfig {
    pub name: String,
    /// `false` switches the stack off: not queried, not shown.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Card order, for any source kind; not part of `filter`.
    #[serde(default)]
    pub sort: StackSort,
    /// Patterns (case-insensitive regular expressions) of cards to leave out
    /// of this stack, matched against title and body; for any source kind,
    /// not part of `filter`. Compiled and checked when the config loads.
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(flatten)]
    pub filter: toml::Table,
}

impl Eq for StackConfig {}

/// How a stack orders cards of the same severity and due time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackSort {
    /// Most recently updated first.
    #[default]
    Newest,
    /// Least recently updated first, to surface stale work.
    Oldest,
}

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

const GITHUB_ICON: &str = "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/github.svg";
const GITHUB_DARK_ICON: &str =
    "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/github-light.svg";
const GOOGLE_CALENDAR_ICON: &str =
    "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/google-calendar.svg";
const ICAL_ICON: &str = "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/ical.svg";
const PLANE_ICON: &str = "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/plane.svg";
const TODOIST_ICON: &str =
    "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/todoist.svg";

impl SourceKind {
    /// The provider's logo from Dashboard Icons. Every provider kind has
    /// one; add it here when adding a kind.
    pub fn default_icon(self) -> Option<Icon> {
        let (url, dark_url) = match self {
            Self::Github => (GITHUB_ICON, Some(GITHUB_DARK_ICON)),
            Self::Google => (GOOGLE_CALENDAR_ICON, None),
            Self::Ical => (ICAL_ICON, None),
            Self::Plane => (PLANE_ICON, None),
            Self::Todoist => (TODOIST_ICON, None),
            // Not a provider: the page falls back to its generic icon.
            Self::Sample => return None,
        };

        Some(Icon {
            url: url.to_owned(),
            dark_url: dark_url.map(str::to_owned),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConfigError {
    #[error("source name `{0}` is used more than once")]
    DuplicateSourceName(String),
    #[error("`{0}` must be greater than zero")]
    Zero(String),
    #[error("source `{source_name}` has more than one stack named `{column}`")]
    DuplicateColumnName { source_name: String, column: String },
    #[error("source `{0}` has a stack without a name")]
    EmptyColumnName(String),
    #[error("source `{source_name}`: {message}")]
    Icon {
        source_name: String,
        message: String,
    },
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
            for column in &source.stacks {
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
            let icon_error = |message: &str| {
                Err(ConfigError::Icon {
                    source_name: source.name.clone(),
                    message: message.to_owned(),
                })
            };
            if source.icon_dark.is_some() && source.icon.is_none() {
                return icon_error("`icon_dark` needs `icon`");
            }
            for (key, url) in [("icon", &source.icon), ("icon_dark", &source.icon_dark)] {
                if url.as_deref().is_some_and(|url| !is_http_url(url)) {
                    return icon_error(&format!("`{key}` must be an http(s) URL"));
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

impl SourceConfig {
    /// Card order of each configured stack, by stack name.
    pub fn stack_sorts(&self) -> BTreeMap<String, StackSort> {
        self.stacks
            .iter()
            .filter(|stack| stack.enabled)
            .map(|stack| (stack.name.clone(), stack.sort))
            .collect()
    }

    /// Whether the source declares stacks and every one is switched off.
    pub fn all_stacks_disabled(&self) -> bool {
        !self.stacks.is_empty() && self.stacks.iter().all(|stack| !stack.enabled)
    }

    /// A configured icon, or the built-in provider logo when available.
    pub fn icon(&self) -> Option<Icon> {
        match &self.icon {
            Some(url) => Some(Icon {
                url: url.clone(),
                dark_url: self.icon_dark.clone(),
            }),
            None => self.kind.default_icon(),
        }
    }
}

fn default_refresh_seconds() -> u64 {
    300
}

/// Room for most sources; slow ones (Plane) set their own.
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

    /// A source's filters are `stacks`, shown top to bottom in file order;
    /// `columns`, the old name, still works.
    #[test]
    fn stacks_keep_file_order_and_accept_the_old_name() {
        let config = parse(
            r#"
            [[sources]]
            name = "github"
            kind = "github"

              [[sources.stacks]]
              name = "Notifications"

              [[sources.stacks]]
              name = "Review requested"

            [[sources]]
            name = "old"
            kind = "sample"

              [[sources.columns]]
              name = "Mine"
            "#,
        );
        config.validate().expect("valid stacks");
        let names: Vec<&str> = config.sources[0]
            .stacks
            .iter()
            .map(|stack| stack.name.as_str())
            .collect();
        assert_eq!(names, ["Notifications", "Review requested"]);
        assert_eq!(config.sources[1].stacks[0].name, "Mine");

        let error = parse(
            "[[sources]]\nname = \"s\"\nkind = \"sample\"\n[[sources.stacks]]\nname = \"A\"\n[[sources.stacks]]\nname = \"A\"",
        )
        .validate()
        .expect_err("duplicate stack");
        assert!(error.to_string().contains("stack named `A`"), "{error}");
    }

    /// Any stack may say `sort = "oldest"` (default `"newest"`); the key is
    /// read here, not by the source's filter, and other values are rejected.
    #[test]
    fn stacks_sort_newest_first_unless_asked_for_oldest() {
        let config = parse(
            r#"
            [[sources]]
            name = "plane"
            kind = "plane"

              [[sources.stacks]]
              name = "Stale"
              sort = "oldest"
              state = ["In Review"]

              [[sources.stacks]]
              name = "Recent"
              sort = "newest"

              [[sources.stacks]]
              name = "Default"
            "#,
        );
        let stacks = &config.sources[0].stacks;
        let sorts: Vec<StackSort> = stacks.iter().map(|stack| stack.sort).collect();
        assert_eq!(
            sorts,
            [StackSort::Oldest, StackSort::Newest, StackSort::Newest]
        );
        assert!(
            !stacks[0].filter.contains_key("sort"),
            "{:?}",
            stacks[0].filter
        );
        assert!(stacks[0].filter.contains_key("state"));

        let error = toml::from_str::<AppConfig>(
            "[[sources]]\nname = \"s\"\nkind = \"plane\"\n[[sources.stacks]]\nname = \"A\"\nsort = \"stale\"",
        )
        .expect_err("unknown sort");
        assert!(error.to_string().contains("stale"), "{error}");
    }

    /// Any stack may list `exclude` patterns (none by default); the key is
    /// read here, not by the source's filter, and it must be a list.
    #[test]
    fn stacks_take_exclude_patterns_out_of_the_filter() {
        let config = parse(
            r#"
            [[sources]]
            name = "plane"
            kind = "plane"

              [[sources.stacks]]
              name = "Quiet"
              exclude = ["dependabot\\[bot\\]", "^chore\\(deps\\)"]
              state = ["In Review"]

              [[sources.stacks]]
              name = "Default"
            "#,
        );
        let stacks = &config.sources[0].stacks;
        assert_eq!(stacks[0].exclude, [r"dependabot\[bot\]", r"^chore\(deps\)"]);
        assert!(stacks[1].exclude.is_empty());
        assert!(
            !stacks[0].filter.contains_key("exclude"),
            "{:?}",
            stacks[0].filter
        );
        assert!(stacks[0].filter.contains_key("state"));

        toml::from_str::<AppConfig>(
            "[[sources]]\nname = \"s\"\nkind = \"plane\"\n[[sources.stacks]]\nname = \"A\"\nexclude = \"bot\"",
        )
        .expect_err("exclude must be a list");
    }

    /// A source may name an icon (and a variant for dark themes) by http(s)
    /// URL; anything else is rejected, since the page loads it as an image.
    #[test]
    fn source_icons_are_http_urls() {
        let config = parse(
            r#"
            [[sources]]
            name = "github"
            kind = "github"
            icon = "https://cdn.example/github.svg"
            icon_dark = "https://cdn.example/github-light.svg"
            "#,
        );
        config.validate().expect("valid icons");
        assert_eq!(
            config.sources[0].icon(),
            Some(Icon {
                url: "https://cdn.example/github.svg".to_owned(),
                dark_url: Some("https://cdn.example/github-light.svg".to_owned()),
            })
        );

        for text in [
            "[[sources]]\nname = \"s\"\nkind = \"sample\"\nicon = \"javascript:alert(1)\"",
            "[[sources]]\nname = \"s\"\nkind = \"sample\"\nicon = \"https://x/a.svg\"\nicon_dark = \"file:///a.svg\"",
            "[[sources]]\nname = \"s\"\nkind = \"sample\"\nicon_dark = \"https://x/a.svg\"",
        ] {
            let error = parse(text).validate().expect_err(text);
            assert!(error.to_string().contains("icon"), "{error}");
        }
    }

    /// Where the default logos come from (the Dashboard Icons collection).
    const DASHBOARD_ICONS: &str = "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/";

    /// The kind after `kind` in declaration order. The match has no wildcard,
    /// so a new kind does not compile until it is placed in the walk, and
    /// the test below then covers it.
    fn kind_after(kind: SourceKind) -> Option<SourceKind> {
        match kind {
            SourceKind::Github => Some(SourceKind::Google),
            SourceKind::Google => Some(SourceKind::Ical),
            SourceKind::Ical => Some(SourceKind::Plane),
            SourceKind::Plane => Some(SourceKind::Sample),
            SourceKind::Sample => Some(SourceKind::Todoist),
            SourceKind::Todoist => None,
        }
    }

    /// Every provider has a logo from Dashboard Icons by default, so a group
    /// is recognisable without configuring `icon`; only the built-in sample
    /// source, which is no provider, has none.
    #[test]
    fn every_provider_has_a_default_icon() {
        let mut next = Some(SourceKind::Github);
        let mut seen = 0;
        while let Some(kind) = next {
            next = kind_after(kind);
            seen += 1;
            if kind == SourceKind::Sample {
                assert_eq!(kind.default_icon(), None);
                continue;
            }
            let icon = kind
                .default_icon()
                .unwrap_or_else(|| panic!("{kind:?} has a default icon"));
            assert!(
                icon.url.starts_with(DASHBOARD_ICONS),
                "{kind:?}: {}",
                icon.url
            );
        }
        assert_eq!(seen, 6, "the walk visits every kind");
        assert_eq!(
            SourceKind::Ical.default_icon().map(|icon| icon.url),
            Some(ICAL_ICON.to_owned())
        );
    }

    #[test]
    fn known_sources_use_provider_icons_by_default() {
        let config = parse(
            r#"
            [[sources]]
            name = "github"
            kind = "github"

            [[sources]]
            name = "agenda"
            kind = "google"

            [[sources]]
            name = "plane"
            kind = "plane"

            [[sources]]
            name = "todoist"
            kind = "todoist"
            "#,
        );

        assert_eq!(
            config.sources[0].icon(),
            Some(Icon {
                url: GITHUB_ICON.to_owned(),
                dark_url: Some(GITHUB_DARK_ICON.to_owned()),
            })
        );
        assert_eq!(
            config.sources[1]
                .icon()
                .as_ref()
                .map(|icon| icon.url.as_str()),
            Some(GOOGLE_CALENDAR_ICON)
        );
        assert_eq!(
            config.sources[2]
                .icon()
                .as_ref()
                .map(|icon| icon.url.as_str()),
            Some(PLANE_ICON)
        );
        assert_eq!(
            config.sources[3]
                .icon()
                .as_ref()
                .map(|icon| icon.url.as_str()),
            Some(TODOIST_ICON)
        );
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
