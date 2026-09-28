//! Locating and loading the config file, and turning it into scheduled sources.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pending_core::{
    AppConfig, ConfigError, DEFAULT_BOARD, PendingSource, SampleSource, SourceConfig, SourceKind,
};
use pending_github::{DEFAULT_API_URL, GithubColumn, GithubSource, gh_cli_token, resolve_token};
use pending_google::{GoaTokens, GoogleColumn, GoogleSource};
use pending_ical::{IcalColumn, IcalSource};
use pending_plane::{PlaneColumn, PlaneSettings, PlaneSource};
use pending_todoist::{TodoistColumn, TodoistSource};
use serde::de::DeserializeOwned;
use thiserror::Error;
use tracing::info;

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
    #[error("invalid config {}: source `{source_name}`, column `{column}`: {message}", path.display())]
    Column {
        path: PathBuf,
        source_name: String,
        column: String,
        message: String,
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

/// Where the source cache lives: `$XDG_STATE_HOME/tasks-pending/cache.json`,
/// else `$HOME/.local/state/tasks-pending/cache.json`. Relative values are
/// ignored, per the XDG spec.
pub fn cache_path(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |key: &str| {
        env(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let base = absolute("XDG_STATE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".local").join("state")))?;
    Some(base.join("tasks-pending").join("cache.json"))
}

/// Locates and loads the config, falling back to the sample source only when
/// no file exists at the default location.
pub fn load_plan(
    cli: Option<PathBuf>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<(Plan, Origin), LoadError> {
    load_plan_with(cli, env, &gh_cli_token)
}

/// [`load_plan`] with the `gh auth token` lookup injected, for tests.
pub fn load_plan_with(
    cli: Option<PathBuf>,
    env: &dyn Fn(&str) -> Option<OsString>,
    gh: &dyn Fn() -> Option<String>,
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
    let plan = plan(&config, &path, env, gh)?;
    Ok((plan, Origin::File(path)))
}

fn plan(
    config: &AppConfig,
    path: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
    gh: &dyn Fn() -> Option<String>,
) -> Result<Plan, LoadError> {
    let env_string = |key: &str| env(key).and_then(|value| value.into_string().ok());
    // Resolved at most once: `gh auth token` may be slow, and every GitHub
    // source uses the same account.
    let mut github_token: Option<Option<String>> = None;
    let mut specs = Vec::new();

    for source in config.sources.iter().filter(|source| source.enabled) {
        let implementation: Arc<dyn PendingSource> =
            match source.kind {
                SourceKind::Sample => {
                    if let Some(column) = source.columns.first() {
                        return Err(column_error(
                            path,
                            source,
                            &column.name,
                            "the sample source has fixed columns",
                        ));
                    }
                    Arc::new(SampleSource)
                }
                SourceKind::Google => Arc::new(
                    GoogleSource::new(Arc::new(GoaTokens::from_env(&env_string))).with_columns(
                        parse_columns(path, source, |c: &mut GoogleColumn, name| {
                            c.name = name;
                            Ok(())
                        })?,
                    ),
                ),
                SourceKind::Ical => Arc::new(IcalSource::from_env(&env_string).with_columns(
                    parse_columns(path, source, |c: &mut IcalColumn, name| {
                        c.name = name;
                        Ok(())
                    })?,
                )),
                SourceKind::Plane => Arc::new(
                    PlaneSource::new(PlaneSettings::from_env(&env_string)).with_columns(
                        parse_columns(path, source, |c: &mut PlaneColumn, name| {
                            c.name = name;
                            Ok(())
                        })?,
                    ),
                ),
                SourceKind::Todoist => Arc::new(TodoistSource::from_env(&env_string).with_columns(
                    parse_columns(path, source, |c: &mut TodoistColumn, name| {
                        c.name = name;
                        Ok(())
                    })?,
                )),
                SourceKind::Github => {
                    let token = github_token
                        .get_or_insert_with(|| {
                            resolve_token(&env_string, &|| {
                                info!("no GitHub token in the environment; asking `gh auth token`");
                                gh()
                            })
                        })
                        .clone();
                    Arc::new(
                        GithubSource::new(DEFAULT_API_URL, token).with_columns(parse_columns(
                            path,
                            source,
                            |c: &mut GithubColumn, name| {
                                c.name = name;
                                c.validate()
                            },
                        )?),
                    )
                }
            };
        specs.push(SourceSpec {
            name: source.name.clone(),
            source: implementation,
            board: source.board.clone(),
            interval: Duration::from_secs(config.refresh_seconds_for(source)),
        });
    }

    Ok(Plan {
        specs,
        timeout: Duration::from_secs(config.timeout_seconds),
    })
}

/// A source's `[[sources.columns]]` read as that kind's column type; unknown
/// or missing filter keys name the source and the column.
fn parse_columns<T: DeserializeOwned>(
    path: &Path,
    source: &SourceConfig,
    finish: impl Fn(&mut T, String) -> Result<(), String>,
) -> Result<Vec<T>, LoadError> {
    source
        .columns
        .iter()
        .map(|column| {
            let mut parsed: T = toml::Value::Table(column.filter.clone())
                .try_into()
                .map_err(|error: toml::de::Error| {
                    column_error(path, source, &column.name, error.message())
                })?;
            finish(&mut parsed, column.name.clone())
                .map_err(|message| column_error(path, source, &column.name, &message))?;
            Ok(parsed)
        })
        .collect()
}

fn column_error(path: &Path, source: &SourceConfig, column: &str, message: &str) -> LoadError {
    LoadError::Column {
        path: path.to_owned(),
        source_name: source.name.clone(),
        column: column.to_owned(),
        message: message.to_owned(),
    }
}

fn sample_plan() -> Plan {
    let defaults = AppConfig::default();
    Plan {
        specs: vec![SourceSpec {
            name: "sample".to_owned(),
            source: Arc::new(SampleSource),
            board: DEFAULT_BOARD.to_owned(),
            interval: Duration::from_secs(defaults.refresh_seconds),
        }],
        timeout: Duration::from_secs(defaults.timeout_seconds),
    }
}
