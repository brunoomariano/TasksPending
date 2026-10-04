//! The running dashboard: an [`Aggregator`] built from the config file, rebuilt
//! when the file changes (on a manual refresh, or when a watch notices it).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use pending_core::DashboardSnapshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::Aggregator;
use crate::cache::Cache;
use crate::config::{LoadError, Origin, load_plan, locate};
use crate::looks::Looks;
use crate::marks::{MarkError, Marks};
use crate::snoozes::{SnoozeError, Snoozes};

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
    marks: Marks,
    snoozes: Snoozes,
    looks: Looks,
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
        // Marks and snoozes live next to the cache; without a cache they
        // stay in memory.
        let beside_cache = |name: &str| {
            cache
                .as_ref()
                .and_then(|cache| cache.parent())
                .map(|dir| dir.join(name))
        };
        let marks = Marks::load(beside_cache("marks.json"));
        let snoozes = Snoozes::load(beside_cache("snoozes.json"));
        let looks = Looks::load(beside_cache("looks.json"));
        let live = Self {
            inner: Arc::new(Inner {
                cli,
                env,
                cache,
                marks,
                snoozes,
                looks,
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
        let mut snapshot = assemble(
            &aggregator,
            &self.inner.marks,
            &self.inner.snoozes,
            &self.inner.looks,
        );
        snapshot.config_error = error;
        snapshot
    }

    /// Records that the user is looking at the dashboard now.
    pub fn look(&self) {
        // What the sources show now; marks and snoozes do not change when
        // their cards were fetched.
        let aggregator = self.state().aggregator.clone();
        self.inner.looks.look(&aggregator.snapshot());
    }

    /// Hides a card until `until`, or until the item changes when `None`;
    /// only a card on the dashboard can be snoozed.
    pub fn snooze(&self, id: &str, until: Option<DateTime<Utc>>) -> Result<(), SnoozeError> {
        self.inner.snoozes.snooze(&self.snapshot(), id, until)
    }

    /// Brings a snoozed card back now.
    pub fn wake(&self, id: &str) -> Result<(), SnoozeError> {
        self.inner.snoozes.wake(id)
    }

    /// Marks or unmarks a card as in progress; only a card on the dashboard
    /// can be marked.
    pub fn set_mark(&self, id: &str, marked: bool) -> Result<(), MarkError> {
        self.inner.marks.set(&self.snapshot(), id, marked)
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

/// The sources' snapshot with the user's state applied to it. The cards
/// hidden by `exclude` come from the same batches as the snapshot, so a
/// hidden card is never taken for a finished one.
fn assemble(
    aggregator: &Aggregator,
    marks: &Marks,
    snoozes: &Snoozes,
    looks: &Looks,
) -> DashboardSnapshot {
    let (mut snapshot, hidden) = aggregator.snapshot_with_hidden();
    // Marks first: they are judged on every card the sources returned,
    // including the ones a snooze is about to hide.
    marks.apply_hiding(&mut snapshot, &hidden);
    snoozes.apply_hiding(&mut snapshot, &hidden);
    // Last: only cards still on the boards are flagged as changed.
    looks.apply(&mut snapshot);
    snapshot
}

/// The config file's current text, if it exists and is readable.
fn read_config(cli: &Option<PathBuf>, env: &Env) -> Option<String> {
    let location = locate(cli.clone(), &**env)?;
    std::fs::read_to_string(location.path).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::Utc;
    use pending_core::{
        BoxFuture, CardSeverity, PendingCard, PendingSource, SourceBatch, SourceConfig,
        SourceError, SourceItem,
    };

    use super::*;
    use crate::SourceSpec;
    use crate::exclude::Excludes;

    /// One stack with one card, "wip parser".
    struct OneCard;

    impl PendingSource for OneCard {
        fn columns(&self) -> Vec<String> {
            vec!["Review".to_owned()]
        }

        fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>> {
            let batch = SourceBatch {
                items: vec![SourceItem {
                    column: "Review".to_owned(),
                    card: PendingCard {
                        id: "parser".to_owned(),
                        title: "wip parser".to_owned(),
                        body: String::new(),
                        source: "test".to_owned(),
                        url: None,
                        severity: CardSeverity::Info,
                        due_at: None,
                        updated_at: Utc::now(),
                    },
                }],
                warnings: Vec::new(),
            };
            Box::pin(async move { Ok(batch) })
        }
    }

    fn aggregator(exclude: &str) -> Aggregator {
        let config: SourceConfig = toml::from_str(&format!(
            "name = \"test\"\nkind = \"plane\"\n[[stacks]]\nname = \"Review\"\nexclude = [{exclude}]"
        ))
        .expect("valid toml");
        Aggregator::start(
            vec![SourceSpec {
                name: "test".to_owned(),
                source: Arc::new(OneCard),
                board: "Work".to_owned(),
                interval: Duration::from_secs(60),
                timeout: None,
                icon: None,
                sorts: Default::default(),
                excludes: Excludes::from_stacks(&config.stacks).expect("valid patterns"),
            }],
            Duration::from_secs(30),
        )
    }

    /// A marked card that a new `exclude` pattern hides keeps its mark on
    /// the running dashboard, although a clean refresh after the mark no
    /// longer shows the card: its source still lists it.
    #[tokio::test(start_paused = true)]
    async fn a_mark_survives_its_card_being_excluded() {
        let marks = Marks::load(None);
        let snoozes = Snoozes::load(None);
        let looks = Looks::load(None);
        let shown = aggregator("");
        tokio::time::sleep(Duration::from_secs(1)).await;
        marks
            .set(&assemble(&shown, &marks, &snoozes, &looks), "parser", true)
            .expect("the card is on the dashboard");

        // Real time, so that the next refresh is after the mark.
        std::thread::sleep(Duration::from_millis(5));
        let excluding = aggregator("\"wip\"");
        tokio::time::sleep(Duration::from_secs(1)).await;
        let snapshot = assemble(&excluding, &marks, &snoozes, &looks);

        assert!(snapshot.boards[0].groups[0].columns[0].cards.is_empty());
        assert_eq!(snapshot.marked, ["parser"]);
        // Passing the hidden ids is what keeps it: without them the mark
        // of a card gone after a clean refresh is dropped.
        let mut bare = excluding.snapshot();
        marks.apply(&mut bare);
        assert!(bare.marked.is_empty());
    }

    /// Only cards on the boards are flagged as changed: a snoozed card
    /// that changed is in the snoozed list, not among the flags, and a
    /// look records the refresh its source was showing.
    #[tokio::test(start_paused = true)]
    async fn a_snoozed_card_is_not_flagged_as_changed() {
        let (marks, snoozes, looks) = (Marks::load(None), Snoozes::load(None), Looks::load(None));
        // The first look was an hour ago; the card was updated since.
        looks.look_at(Utc::now() - chrono::Duration::hours(1));
        let shown = aggregator("");
        tokio::time::sleep(Duration::from_secs(1)).await;

        let snapshot = assemble(&shown, &marks, &snoozes, &looks);
        assert_eq!(snapshot.changed, ["parser"]);

        snoozes
            .snooze(
                &snapshot,
                "parser",
                Some(Utc::now() + chrono::Duration::hours(1)),
            )
            .expect("the card is on the dashboard");
        let snapshot = assemble(&shown, &marks, &snoozes, &looks);

        assert_eq!(snapshot.snoozed.len(), 1);
        assert!(snapshot.changed.is_empty());
    }
}
