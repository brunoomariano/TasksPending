//! A quiet, once-a-day look at the latest release, so the window can say
//! when a newer version exists. Nothing is downloaded; `tasks-pending
//! upgrade` does that when asked.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use pending_core::api::VersionReport;
use pending_core::release::{is_newer, process_var, release_base_url, version_from_release_url};
use tracing::{debug, warn};

/// Set to anything to skip the check (e.g. offline machines, tests).
pub const DISABLE_VARIABLE: &str = "TASKS_PENDING_NO_VERSION_CHECK";
/// How long after the daemon starts the first look happens.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
/// How often it looks again.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest wait for the release page's redirect.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// When the looks happen.
#[derive(Clone, Copy, Debug)]
struct Schedule {
    first_after: Duration,
    every: Duration,
}

/// The latest version seen, shared with the route.
#[derive(Clone, Default)]
pub struct UpdateCheck {
    latest: Arc<RwLock<Option<String>>>,
    looking: bool,
}

impl UpdateCheck {
    /// A check that never looks: for the sandbox and tests.
    pub fn off() -> Self {
        Self::default()
    }

    /// Starts looking in the background, unless [`DISABLE_VARIABLE`] is
    /// set or the releases location in the environment is not usable.
    /// Needs a Tokio runtime.
    pub fn start() -> Self {
        Self::start_in(&process_var)
    }

    /// [`start`](Self::start) reading the environment through `var`.
    pub fn start_in(var: &dyn Fn(&str) -> Option<String>) -> Self {
        if var(DISABLE_VARIABLE).is_some() {
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
        Self::looking(
            client,
            base,
            Schedule {
                first_after: FIRST_CHECK_AFTER,
                every: CHECK_EVERY,
            },
        )
    }

    /// Whether the check looks at the releases page at all.
    pub fn is_looking(&self) -> bool {
        self.looking
    }

    /// Looks at `<base>/latest` with `client` on `schedule`, in the
    /// background, and remembers the version it points at.
    fn looking(client: reqwest::Client, base: String, schedule: Schedule) -> Self {
        let check = Self {
            latest: Arc::default(),
            looking: true,
        };
        let latest = check.latest.clone();
        tokio::spawn(async move {
            tokio::time::sleep(schedule.first_after).await;
            loop {
                if let Some(version) = latest_release_with(&client, &base).await {
                    *latest.write().unwrap_or_else(|e| e.into_inner()) = Some(version);
                }
                tokio::time::sleep(schedule.every).await;
            }
        });
        check
    }

    /// The report for the running version.
    pub fn report(&self, current: &str) -> VersionReport {
        let latest = self
            .latest
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        VersionReport {
            current: current.to_owned(),
            available: latest.filter(|latest| is_newer(latest, current)),
        }
    }
}

/// The client for the releases page: https only (also for the redirect's
/// target), the redirect itself is the answer so none is followed, and a
/// short wait.
pub fn release_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("tasks-pending/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// The version `<base>/latest` redirects to, from the redirect alone: no
/// body, no API, no token. `base` is the releases URL prefix.
pub async fn latest_release(base: &str) -> Option<String> {
    let client = match release_client() {
        Ok(client) => client,
        Err(error) => {
            debug!(%error, "version check failed: no HTTP client");
            return None;
        }
    };
    latest_release_with(&client, base).await
}

/// [`latest_release`] with the caller's client.
pub async fn latest_release_with(client: &reqwest::Client, base: &str) -> Option<String> {
    let url = format!("{base}/latest");
    let response = match client.head(&url).send().await {
        Ok(response) => response,
        Err(error) => {
            debug!(%error, "version check failed");
            return None;
        }
    };
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    let version = version_from_release_url(location);
    debug!(?version, location, "version check");
    version
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
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::IntoResponse;
    use pending_core::release::BASE_URL_VARIABLE;
    use tokio::net::TcpListener;

    use super::*;

    /// Nothing is reported until a newer version was seen; the same or an
    /// older one is not news.
    #[test]
    fn reports_only_a_newer_version() {
        let check = UpdateCheck::off();
        assert_eq!(check.report("0.6.0").available, None);
        *check.latest.write().unwrap() = Some("0.6.0".to_owned());
        assert_eq!(check.report("0.6.0").available, None);
        *check.latest.write().unwrap() = Some("0.5.0".to_owned());
        assert_eq!(check.report("0.6.0").available, None);
        *check.latest.write().unwrap() = Some("0.7.0".to_owned());
        assert_eq!(
            check.report("0.6.0"),
            VersionReport {
                current: "0.6.0".to_owned(),
                available: Some("0.7.0".to_owned())
            }
        );
    }

    /// A stand-in for the releases pages: `/latest` redirects to 0.7.0 the
    /// way GitHub does, `/plain/latest` answers without a redirect.
    async fn releases() -> String {
        let router = Router::new()
            .route(
                "/latest",
                get(|| async {
                    (
                        StatusCode::FOUND,
                        [(
                            header::LOCATION,
                            HeaderValue::from_static("https://github.com/o/r/releases/tag/v0.7.0"),
                        )],
                    )
                        .into_response()
                }),
            )
            .route("/plain/latest", get(|| async { "no redirect" }));
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
    /// or none at all, is no version.
    #[tokio::test]
    async fn reads_the_version_from_the_redirect() {
        let base = releases().await;
        let client = permissive();
        assert_eq!(
            latest_release_with(&client, &base).await,
            Some("0.7.0".to_owned())
        );
        assert_eq!(
            latest_release_with(&client, &format!("{base}/plain")).await,
            None
        );
        assert_eq!(
            latest_release_with(&client, &format!("{base}/missing")).await,
            None
        );
    }

    /// The daemon's own client only talks https: a plain-text releases
    /// location, even one that answers, gives no version.
    #[tokio::test]
    async fn only_asks_over_https() {
        let base = releases().await;
        assert_eq!(latest_release(&base).await, None);
    }

    /// The background look reads the page on its schedule and the report
    /// shows what it found.
    #[tokio::test]
    async fn looks_in_the_background() {
        let base = releases().await;
        let check = UpdateCheck::looking(
            permissive(),
            base,
            Schedule {
                first_after: Duration::ZERO,
                every: Duration::from_secs(3600),
            },
        );
        assert!(check.is_looking());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while check.report("0.6.0").available.is_none() {
            assert!(tokio::time::Instant::now() < deadline, "no version seen");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(check.report("0.6.0").available.as_deref(), Some("0.7.0"));
        assert_eq!(check.report("0.7.0").available, None);
    }

    /// `TASKS_PENDING_NO_VERSION_CHECK` set to anything, or a releases
    /// location that is not https, means no look at all.
    #[tokio::test]
    async fn stays_off_when_told_or_when_the_location_is_unsafe() {
        let disabled = |name: &str| (name == DISABLE_VARIABLE).then(String::new);
        assert!(!UpdateCheck::start_in(&disabled).is_looking());
        let plain = |name: &str| (name == BASE_URL_VARIABLE).then(|| "http://x.test".to_owned());
        assert!(!UpdateCheck::start_in(&plain).is_looking());
        assert!(!UpdateCheck::off().is_looking());
        // The first look is half a minute away: the test ends before it.
        assert!(UpdateCheck::start_in(&|_| None).is_looking());
    }
}
