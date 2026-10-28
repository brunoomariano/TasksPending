//! A quiet, once-a-day look at the latest release, so the window can say
//! when a newer version exists. Nothing is downloaded; `tasks-pending
//! upgrade` does that when asked.

use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use pending_core::api::VersionReport;
use pending_core::release::{is_newer, release_base_url, version_from_release_url};
use tracing::{debug, info, warn};

/// Set to any value but a blank one, e.g. `1`, to skip the check (offline
/// machines, tests). Blank reads as unset, as for the releases location.
pub const DISABLE_VARIABLE: &str = "TASKS_PENDING_NO_VERSION_CHECK";
/// How long after the daemon starts the first look happens.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
/// How long after a look that found the latest release the next one is due.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long after a look that failed (offline at login, the page down) the
/// next one is due: soon, rather than a day later.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// Longest sleep between two glances at the wall clock. A suspended
/// machine does not count the time asleep in a monotonic timer, so the
/// loop wakes this often to see whether a look came due meanwhile.
const WAKE_EVERY: Duration = Duration::from_secs(10 * 60);
/// Longest wait for the release page's redirect.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// When the looks happen.
#[derive(Clone, Copy, Debug)]
struct Schedule {
    first_after: Duration,
    every: Duration,
    retry_after: Duration,
    wake_every: Duration,
}

const SCHEDULE: Schedule = Schedule {
    first_after: FIRST_CHECK_AFTER,
    every: CHECK_EVERY,
    retry_after: RETRY_AFTER,
    wake_every: WAKE_EVERY,
};

/// The wall clock the schedule reads: the system's, or a fake in tests.
type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

fn system_clock() -> Clock {
    Arc::new(SystemTime::now)
}

/// Why a look at the releases page found no version.
#[derive(Debug)]
pub enum LatestReleaseError {
    /// No HTTP client could be built.
    Client(String),
    /// The page did not answer (offline, refused, timed out, not https).
    Request { url: String, reason: String },
    /// The page answered without redirecting to a release.
    NoRedirect { url: String, status: u16 },
    /// The redirect points at something that is not a version.
    NotAVersion { url: String, location: String },
}

impl fmt::Display for LatestReleaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(reason) => write!(f, "no HTTP client: {reason}"),
            Self::Request { url, reason } => write!(f, "{url}: {reason}"),
            Self::NoRedirect { url, status } => {
                write!(f, "{url} answered {status} without a redirect")
            }
            Self::NotAVersion { url, location } => {
                write!(f, "{url} redirects to {location}, which names no version")
            }
        }
    }
}

impl std::error::Error for LatestReleaseError {}

/// An error and every cause under it: reqwest's own text only names the
/// kind of failure.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

/// The latest version seen, shared with the route; none when the check is
/// off, since nothing looks.
#[derive(Clone, Default)]
pub struct UpdateCheck {
    latest: Option<Arc<RwLock<Option<String>>>>,
}

impl UpdateCheck {
    /// A check that never looks: for the sandbox and tests.
    pub fn off() -> Self {
        Self::default()
    }

    /// Starts looking in the background, unless [`DISABLE_VARIABLE`] is
    /// set or the releases location is not usable, both read through `var`.
    /// Needs a Tokio runtime.
    pub fn start_in(var: &dyn Fn(&str) -> Option<String>) -> Self {
        if var(DISABLE_VARIABLE).is_some_and(|value| !value.trim().is_empty()) {
            debug!("version check disabled by {DISABLE_VARIABLE}");
            return Self::off();
        }
        let base = match release_base_url(var) {
            Ok(base) => base,
            Err(error) => {
                warn!(%error, "version check off");
                return Self::off();
            }
        };
        let client = match release_client() {
            Ok(client) => client,
            Err(error) => {
                warn!(%error, "version check off: no HTTP client");
                return Self::off();
            }
        };
        Self::looking(client, base, SCHEDULE)
    }

    /// Whether the check looks at the releases page at all.
    #[cfg(test)]
    pub(crate) fn is_looking(&self) -> bool {
        self.latest.is_some()
    }

    /// Looks at `<base>/latest` with `client` on `schedule`, in the
    /// background, and remembers the version it points at.
    fn looking(client: reqwest::Client, base: String, schedule: Schedule) -> Self {
        let look = move || {
            let client = client.clone();
            let base = base.clone();
            async move { find_latest_release(&client, &base).await }
        };
        Self::looking_with(look, schedule, system_clock())
    }

    /// Runs `look` on `schedule`, reading `clock` for when the next one is
    /// due, and remembers the version the last successful one found.
    fn looking_with<L, F>(look: L, schedule: Schedule, clock: Clock) -> Self
    where
        L: Fn() -> F + Send + 'static,
        F: Future<Output = Result<String, LatestReleaseError>> + Send,
    {
        let latest: Arc<RwLock<Option<String>>> = Arc::default();
        let check = Self {
            latest: Some(latest.clone()),
        };
        tokio::spawn(async move {
            let mut due = clock() + schedule.first_after;
            let mut failed_before = false;
            loop {
                // Wall time decides, in short sleeps: time spent suspended
                // counts, which a monotonic sleep until `due` would not.
                if let Ok(left) = due.duration_since(clock())
                    && !left.is_zero()
                {
                    tokio::time::sleep(left.min(schedule.wake_every)).await;
                    continue;
                }
                due = match look().await {
                    Ok(version) => {
                        *latest.write().unwrap_or_else(|e| e.into_inner()) = Some(version);
                        failed_before = false;
                        clock() + schedule.every
                    }
                    Err(error) => {
                        // Once at info, so a wrong releases location shows
                        // in the journal; the hourly retries stay quiet.
                        if failed_before {
                            debug!(%error, "version check failed again");
                        } else {
                            info!(%error, "version check failed; looking again in an hour");
                        }
                        failed_before = true;
                        clock() + schedule.retry_after
                    }
                };
            }
        });
        check
    }

    /// The report for the running version.
    pub fn report(&self, current: &str) -> VersionReport {
        let latest = self
            .latest
            .as_ref()
            .and_then(|latest| latest.read().unwrap_or_else(|e| e.into_inner()).clone());
        VersionReport {
            current: current.to_owned(),
            available: latest.filter(|latest| is_newer(latest, current)),
        }
    }
}

/// The client for the releases page: https only, no redirect followed,
/// since the redirect itself is the answer, and a short wait.
pub fn release_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("tasks-pending/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// The version `<base>/latest` redirects to, from the redirect alone (no
/// body, no API, no token), or why there is none. `base` is the releases
/// URL prefix.
pub async fn try_latest_release(base: &str) -> Result<String, LatestReleaseError> {
    let client =
        release_client().map_err(|error| LatestReleaseError::Client(with_causes(&error)))?;
    find_latest_release(&client, base).await
}

/// [`try_latest_release`] with the caller's client.
pub async fn find_latest_release(
    client: &reqwest::Client,
    base: &str,
) -> Result<String, LatestReleaseError> {
    let url = format!("{base}/latest");
    let response = client
        .head(&url)
        .send()
        .await
        .map_err(|error| LatestReleaseError::Request {
            url: url.clone(),
            reason: with_causes(&error.without_url()),
        })?;
    let Some(location) = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(LatestReleaseError::NoRedirect {
            url,
            status: response.status().as_u16(),
        });
    };
    let version = version_from_release_url(location);
    debug!(?version, location, "version check");
    version.ok_or_else(|| LatestReleaseError::NotAVersion {
        url,
        location: location.to_owned(),
    })
}

/// The version route, to merge into the API router.
pub fn routes<S>(check: UpdateCheck) -> Router<S> {
    Router::new()
        .route("/api/v1/version", get(version))
        .with_state(check)
}

async fn version(State(check): State<UpdateCheck>) -> Json<VersionReport> {
    Json(check.report(env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::IntoResponse;
    use pending_core::release::BASE_URL_VARIABLE;
    use tokio::net::TcpListener;

    use super::*;

    const HOUR: Duration = Duration::from_secs(60 * 60);
    const MINUTE: Duration = Duration::from_secs(60);

    /// The daemon's schedule, with the first look right away.
    const TEST_SCHEDULE: Schedule = Schedule {
        first_after: Duration::ZERO,
        ..SCHEDULE
    };

    /// Wall time for the schedule tests: it follows Tokio's paused clock,
    /// plus whatever a test adds to stand for the machine being suspended
    /// (wall time passes, the monotonic clock does not).
    #[derive(Clone)]
    struct Wall {
        start: tokio::time::Instant,
        suspended: Arc<Mutex<Duration>>,
    }

    impl Wall {
        fn new() -> Self {
            Self {
                start: tokio::time::Instant::now(),
                suspended: Arc::default(),
            }
        }

        fn clock(&self) -> Clock {
            let wall = self.clone();
            Arc::new(move || {
                SystemTime::UNIX_EPOCH
                    + Duration::from_secs(1_800_000_000)
                    + wall.start.elapsed()
                    + *wall.suspended.lock().unwrap()
            })
        }

        fn suspend(&self, how_long: Duration) {
            *self.suspended.lock().unwrap() += how_long;
        }
    }

    /// A look that fails its first `failures` times and then finds 0.7.0,
    /// counting the looks in `looks`.
    fn look_failing(
        failures: usize,
        looks: Arc<AtomicUsize>,
    ) -> impl Fn() -> std::future::Ready<Result<String, LatestReleaseError>> + Send + 'static {
        move || {
            let n = looks.fetch_add(1, Ordering::SeqCst);
            std::future::ready(if n < failures {
                Err(LatestReleaseError::NoRedirect {
                    url: "https://releases.test/latest".to_owned(),
                    status: 503,
                })
            } else {
                Ok("0.7.0".to_owned())
            })
        }
    }

    /// Scenario: the machine is offline at login, so the first look fails.
    /// The daemon looks again an hour later, not a day later, and then
    /// reports the newer version.
    #[tokio::test(start_paused = true)]
    async fn looks_again_an_hour_after_a_failed_look() {
        let looks = Arc::new(AtomicUsize::new(0));
        let wall = Wall::new();
        let check =
            UpdateCheck::looking_with(look_failing(1, looks.clone()), TEST_SCHEDULE, wall.clock());

        tokio::time::sleep(MINUTE).await;
        assert_eq!(looks.load(Ordering::SeqCst), 1);
        assert_eq!(check.report("0.6.0").available, None);

        tokio::time::sleep(HOUR - 2 * MINUTE).await;
        assert_eq!(looks.load(Ordering::SeqCst), 1, "not before the hour");

        tokio::time::sleep(12 * MINUTE).await;
        assert_eq!(looks.load(Ordering::SeqCst), 2);
        assert_eq!(check.report("0.6.0").available.as_deref(), Some("0.7.0"));
    }

    /// Scenario: after a successful look the laptop sleeps through the
    /// night. Once it wakes, the next look happens within minutes, because
    /// a day has passed on the wall clock even though the monotonic clock
    /// did not count the time asleep.
    #[tokio::test(start_paused = true)]
    async fn a_look_comes_due_across_a_suspend() {
        let looks = Arc::new(AtomicUsize::new(0));
        let wall = Wall::new();
        let _check =
            UpdateCheck::looking_with(look_failing(0, looks.clone()), TEST_SCHEDULE, wall.clock());

        tokio::time::sleep(MINUTE).await;
        assert_eq!(looks.load(Ordering::SeqCst), 1);

        wall.suspend(25 * HOUR);
        tokio::time::sleep(WAKE_EVERY + MINUTE).await;
        assert_eq!(looks.load(Ordering::SeqCst), 2);

        tokio::time::sleep(HOUR).await;
        assert_eq!(
            looks.load(Ordering::SeqCst),
            2,
            "the next one is a day away"
        );
    }

    /// Nothing is reported until a newer version was seen; the same or an
    /// older one is not news.
    #[test]
    fn reports_only_a_newer_version() {
        let latest: Arc<RwLock<Option<String>>> = Arc::default();
        let check = UpdateCheck {
            latest: Some(latest.clone()),
        };
        assert_eq!(check.report("0.6.0").available, None);
        *latest.write().unwrap() = Some("0.6.0".to_owned());
        assert_eq!(check.report("0.6.0").available, None);
        *latest.write().unwrap() = Some("0.5.0".to_owned());
        assert_eq!(check.report("0.6.0").available, None);
        *latest.write().unwrap() = Some("0.7.0".to_owned());
        assert_eq!(
            check.report("0.6.0"),
            VersionReport {
                current: "0.6.0".to_owned(),
                available: Some("0.7.0".to_owned())
            }
        );
    }

    fn redirect(to: &'static str) -> axum::response::Response {
        (
            StatusCode::FOUND,
            [(header::LOCATION, HeaderValue::from_static(to))],
        )
            .into_response()
    }

    /// A stand-in for the releases pages: `/latest` redirects to 0.7.0 the
    /// way GitHub does, `/plain/latest` answers without a redirect and
    /// `/odd/latest` redirects to the list of releases.
    async fn releases() -> String {
        let router = Router::new()
            .route(
                "/latest",
                get(|| async { redirect("https://github.com/o/r/releases/tag/v0.7.0") }),
            )
            .route("/plain/latest", get(|| async { "no redirect" }))
            .route(
                "/odd/latest",
                get(|| async { redirect("https://github.com/o/r/releases") }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        base
    }

    fn permissive() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    /// The version comes from the redirect alone; an answer without one,
    /// or one to something that is not a release, is no version, and the
    /// error says which.
    #[tokio::test]
    async fn reads_the_version_from_the_redirect() {
        let base = releases().await;
        let client = permissive();
        let find = |base: String| {
            let client = client.clone();
            async move { find_latest_release(&client, &base).await }
        };
        assert_eq!(find(base.clone()).await.unwrap(), "0.7.0");
        assert!(matches!(
            find(format!("{base}/plain")).await,
            Err(LatestReleaseError::NoRedirect { status: 200, .. })
        ));
        assert!(matches!(
            find(format!("{base}/missing")).await,
            Err(LatestReleaseError::NoRedirect { status: 404, .. })
        ));
        assert!(matches!(
            find(format!("{base}/odd")).await,
            Err(LatestReleaseError::NotAVersion { .. })
        ));
    }

    /// The daemon's own client only talks https: a plain-text releases
    /// location, even one that answers, gives no version.
    #[tokio::test]
    async fn only_asks_over_https() {
        let base = releases().await;
        assert!(matches!(
            try_latest_release(&base).await,
            Err(LatestReleaseError::Request { .. })
        ));
    }

    /// The background look reads the page on its schedule and the report
    /// shows what it found.
    #[tokio::test]
    async fn looks_in_the_background() {
        let base = releases().await;
        let check = UpdateCheck::looking(permissive(), base, TEST_SCHEDULE);
        assert!(check.is_looking());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while check.report("0.6.0").available.is_none() {
            assert!(tokio::time::Instant::now() < deadline, "no version seen");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(check.report("0.6.0").available.as_deref(), Some("0.7.0"));
        assert_eq!(check.report("0.7.0").available, None);
    }

    /// An environment holding `pairs`, with the releases location on
    /// loopback unless `pairs` sets one, so a look never leaves the machine.
    fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        let mut pairs = pairs.to_vec();
        pairs.push((BASE_URL_VARIABLE, "https://127.0.0.1:9/releases"));
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    /// Scenario: the env file says `TASKS_PENDING_NO_VERSION_CHECK=1`, or
    /// names a releases location that is not https. The daemon never looks.
    #[tokio::test]
    async fn stays_off_when_told_or_when_the_location_is_unsafe() {
        assert!(!UpdateCheck::start_in(&env(&[(DISABLE_VARIABLE, "1")])).is_looking());
        let plain = env(&[(BASE_URL_VARIABLE, "http://x.test")]);
        assert!(!UpdateCheck::start_in(&plain).is_looking());
        assert!(!UpdateCheck::off().is_looking());
    }

    /// Scenario: the env file keeps `TASKS_PENDING_NO_VERSION_CHECK=` with
    /// no value, a line to fill in later. Blank reads as unset, as it does
    /// for the releases location, so the daemon still looks.
    #[tokio::test]
    async fn a_blank_switch_leaves_the_check_on() {
        // The first look is half a minute away: the test ends before it.
        assert!(UpdateCheck::start_in(&env(&[])).is_looking());
        assert!(UpdateCheck::start_in(&env(&[(DISABLE_VARIABLE, "")])).is_looking());
        assert!(UpdateCheck::start_in(&env(&[(DISABLE_VARIABLE, " ")])).is_looking());
    }
}
