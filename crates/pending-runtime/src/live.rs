//! The running dashboard: an [`Aggregator`] built from the config file, rebuilt
//! when the file changes (on a manual refresh, or when a watch notices it).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use pending_core::DashboardSnapshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::Aggregator;
use crate::cache::Cache;
use crate::config::{LoadError, Origin, load_plan, locate};

/// Reads an environment variable; injectable for tests.
pub type Env = Arc<dyn Fn(&str) -> Option<OsString> + Send + Sync>;

/// The process environment.
pub fn process_env() -> Env {
    Arc::new(|key| std::env::var_os(key))
}

/// Cheap-to-clone handle to the running dashboard.
#[derive(Clone)]
pub struct Live {
    inner: Arc<Inner>,
}

struct Inner {
    cli: Option<PathBuf>,
    env: Env,
    cache: Option<PathBuf>,
    state: Mutex<State>,
    generation: AtomicU64,
}

struct State {
    aggregator: Aggregator,
    /// The config file text last loaded (or rejected); `None` when there is
    /// no file.
    text: Option<String>,
    /// Why the current file was rejected; the previous config keeps running.
    error: Option<String>,
}

impl Live {
    /// Loads the config and starts refreshing. Must be called inside a Tokio
    /// runtime. A config error at startup is returned, as before.
    pub fn start(
        cli: Option<PathBuf>,
        env: Env,
        cache: Option<PathBuf>,
    ) -> Result<(Self, Origin), LoadError> {
        let text = read_config(&cli, &env);
        let (plan, origin) = load_plan(cli.clone(), &*env)?;
        let aggregator =
            Aggregator::start_with_cache(plan.specs, plan.timeout, cache.clone().map(Cache::new));
        let live = Self {
            inner: Arc::new(Inner {
                cli,
                env,
                cache,
                state: Mutex::new(State {
                    aggregator,
                    text,
                    error: None,
                }),
                generation: AtomicU64::new(0),
            }),
        };
        Ok((live, origin))
    }

    pub fn snapshot(&self) -> DashboardSnapshot {
        let (aggregator, error) = {
            let state = self.state();
            (state.aggregator.clone(), state.error.clone())
        };
        let mut snapshot = aggregator.snapshot();
        snapshot.config_error = error;
        snapshot
    }

    /// Reloads the config if the file changed (which refreshes every source),
    /// otherwise refreshes every source with the current config.
    pub fn refresh_now(&self) {
        if !self.reload_if_changed() {
            self.state().aggregator.refresh_now();
        }
    }

    /// Rebuilds the sources when the config file's text changed. A file that
    /// fails to load leaves the running config in place and is reported in
    /// the snapshot. Returns whether the sources were rebuilt.
    pub fn reload_if_changed(&self) -> bool {
        let text = read_config(&self.inner.cli, &self.inner.env);
        if self.state().text == text {
            return false;
        }
        // Built outside the lock: loading may run `gh auth token`.
        match load_plan(self.inner.cli.clone(), &*self.inner.env) {
            Ok((plan, origin)) => {
                let aggregator = Aggregator::start_with_cache(
                    plan.specs,
                    plan.timeout,
                    self.inner.cache.clone().map(Cache::new),
                );
                let mut state = self.state();
                state.aggregator = aggregator;
                state.text = text;
                state.error = None;
                self.inner.generation.fetch_add(1, Ordering::SeqCst);
                info!(origin = ?origin, "config reloaded");
                true
            }
            Err(error) => {
                warn!(%error, "config not reloaded; keeping the previous one");
                let mut state = self.state();
                state.text = text;
                state.error = Some(error.to_string());
                false
            }
        }
    }

    /// How many times the sources were rebuilt from a changed config.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    /// Checks the config file every `every` and reloads it when it changed.
    /// The check stops when the returned handle is dropped.
    pub fn watch(&self, every: Duration) -> Watch {
        let live = self.clone();
        Watch(tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                live.reload_if_changed();
            }
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Stops the config watch when dropped.
pub struct Watch(JoinHandle<()>);

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The config file's current text, if it exists and is readable.
fn read_config(cli: &Option<PathBuf>, env: &Env) -> Option<String> {
    let location = locate(cli.clone(), &**env)?;
    std::fs::read_to_string(location.path).ok()
}
