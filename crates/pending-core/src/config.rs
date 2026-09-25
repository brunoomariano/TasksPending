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
    pub kind: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub refresh_seconds: Option<u64>,
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
        assert_eq!(source.refresh_seconds, None);
    }
}
