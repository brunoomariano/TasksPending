use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_refresh_seconds")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub name: String,
    pub kind: SourceKind,
    /// Dashboard lane that receives this source's cards.
    #[serde(default = "default_lane")]
    pub lane: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub refresh_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Github,
    Sample,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            refresh_seconds: default_refresh_seconds(),
            sources: Vec::new(),
        }
    }
}

fn default_refresh_seconds() -> u64 {
    300
}

pub const DEFAULT_LANE: &str = "Inbox";

fn default_lane() -> String {
    DEFAULT_LANE.to_owned()
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

    /// Um erro de digitação no tipo da fonte precisa falhar ao carregar a
    /// configuração, não virar uma fonte que nunca produz cards.
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

    /// Cada fonte alimenta uma lane definida na configuração; sem lane
    /// explícita, os cards vão para a lane "Inbox".
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

        assert_eq!(sources.sources[0].lane, "Work");
        assert_eq!(sources.sources[1].lane, "Inbox");
    }
}
