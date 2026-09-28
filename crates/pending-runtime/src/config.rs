//! Locating and loading the config file, and turning it into scheduled sources.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pending_core::{AppConfig, ConfigError, DEFAULT_LANE, SampleSource, SourceKind};
use thiserror::Error;

use crate::SourceSpec;

/// Environment variable that points at an explicit config file.
pub const CONFIG_ENV: &str = "TASKS_PENDING_CONFIG";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigLocation {
    pub path: PathBuf,
    /// Asked for by the user (flag or env). A missing explicit file is an error;
    /// a missing default file means "no config yet".
    pub explicit: bool,
}

/// Where the running plan came from, for startup logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    File(PathBuf),
    /// No config file at the default location; running the sample source.
    SampleDefault {
        searched: Option<PathBuf>,
    },
}

/// Sources to schedule plus the refresh timeout.
#[derive(Debug)]
pub struct Plan {
    pub specs: Vec<SourceSpec>,
    pub timeout: Duration,
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("config file {:?} does not exist", .0)]
    Missing(PathBuf),
    #[error("reading {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parsing {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid config {}: {source}", path.display())]
    Invalid { path: PathBuf, source: ConfigError },
    #[error("invalid config {}: source `{name}` has kind `{kind}`, which is not supported yet", path.display())]
    UnsupportedKind {
        path: PathBuf,
        name: String,
        kind: String,
    },
}

/// Resolves the config path: `cli`, then `TASKS_PENDING_CONFIG`, then
/// `$XDG_CONFIG_HOME/tasks-pending/config.toml`, then
/// `$HOME/.config/tasks-pending/config.toml`. Empty variables are ignored, and
/// so are relative `XDG_CONFIG_HOME`/`HOME` values, per the XDG spec.
pub fn locate(
    cli: Option<PathBuf>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<ConfigLocation> {
    let non_empty = |key: &str| {
        env(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let absolute = |key: &str| non_empty(key).filter(|path| path.is_absolute());

    if let Some(path) = cli.or_else(|| non_empty(CONFIG_ENV)) {
        return Some(ConfigLocation {
            path,
            explicit: true,
        });
    }

    let base = absolute("XDG_CONFIG_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".config")))?;
    Some(ConfigLocation {
        path: base.join("tasks-pending").join("config.toml"),
        explicit: false,
    })
}

/// Locates and loads the config, falling back to the sample source only when
/// no file exists at the default location.
pub fn load_plan(
    cli: Option<PathBuf>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<(Plan, Origin), LoadError> {
    let location = locate(cli, env);
    let Some(location) = location else {
        return Ok((sample_plan(), Origin::SampleDefault { searched: None }));
    };

    let text = match std::fs::read_to_string(&location.path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if location.explicit {
                return Err(LoadError::Missing(location.path));
            }
            return Ok((
                sample_plan(),
                Origin::SampleDefault {
                    searched: Some(location.path),
                },
            ));
        }
        Err(source) => {
            return Err(LoadError::Read {
                path: location.path,
                source,
            });
        }
    };

    let path = location.path;
    let config: AppConfig = match toml::from_str(&text) {
        Ok(config) => config,
        Err(source) => return Err(LoadError::Parse { path, source }),
    };
    if let Err(source) = config.validate() {
        return Err(LoadError::Invalid { path, source });
    }
    let plan = plan(&config, &path)?;
    Ok((plan, Origin::File(path)))
}

fn plan(config: &AppConfig, path: &Path) -> Result<Plan, LoadError> {
    let specs = config
        .sources
        .iter()
        .filter(|source| source.enabled)
        .map(|source| {
            let implementation: Arc<dyn pending_core::PendingSource> = match source.kind {
                SourceKind::Sample => Arc::new(SampleSource),
                SourceKind::Github => {
                    return Err(LoadError::UnsupportedKind {
                        path: path.to_owned(),
                        name: source.name.clone(),
                        kind: "github".to_owned(),
                    });
                }
            };
            Ok(SourceSpec {
                name: source.name.clone(),
                source: implementation,
                lane: source.lane.clone(),
                interval: Duration::from_secs(config.refresh_seconds_for(source)),
            })
        })
        .collect::<Result<_, _>>()?;

    Ok(Plan {
        specs,
        timeout: Duration::from_secs(config.timeout_seconds),
    })
}

fn sample_plan() -> Plan {
    let defaults = AppConfig::default();
    Plan {
        specs: vec![SourceSpec {
            name: "sample".to_owned(),
            source: Arc::new(SampleSource),
            lane: DEFAULT_LANE.to_owned(),
            interval: Duration::from_secs(defaults.refresh_seconds),
        }],
        timeout: Duration::from_secs(defaults.timeout_seconds),
    }
}
