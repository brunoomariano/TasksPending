//! A quiet, once-a-day look at the latest release, so the window can say
//! when a newer version exists. Nothing is downloaded; `tasks-pending
//! upgrade` does that when asked.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use pending_core::api::VersionReport;
use pending_core::release::{REPOSITORY, is_newer, release_base_url, version_from_release_url};
use tracing::debug;

/// Set to anything to skip the check (e.g. offline machines, tests).
pub const DISABLE_VARIABLE: &str = "TASKS_PENDING_NO_VERSION_CHECK";
/// How long after the daemon starts the first look happens.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
/// How often it looks again.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// Longest wait for the release page's redirect.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The latest version seen, shared with the route.
#[derive(Clone, Default)]
pub struct UpdateCheck {
    latest: Arc<RwLock<Option<String>>>,
}

impl UpdateCheck {
    /// A check that never looks: for the sandbox and tests.
    pub fn off() -> Self {
        Self::default()
    }

    /// Starts looking in the background, unless [`DISABLE_VARIABLE`] is
    /// set. Needs a Tokio runtime.
    pub fn start() -> Self {
        let check = Self::default();
        if std::env::var_os(DISABLE_VARIABLE).is_some() {
            debug!("version check disabled by {DISABLE_VARIABLE}");
            return check;
        }
        let latest = check.latest.clone();
        tokio::spawn(async move {
            tokio::time::sleep(FIRST_CHECK_AFTER).await;
            loop {
                if let Some(version) = latest_release(&release_base_url(REPOSITORY)).await {
                    *latest.write().unwrap_or_else(|e| e.into_inner()) = Some(version);
                }
                tokio::time::sleep(CHECK_EVERY).await;
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

/// The version `<base>/latest` redirects to, from the redirect alone: no
/// body, no API, no token. `base` is the releases URL prefix.
pub async fn latest_release(base: &str) -> Option<String> {
    let url = format!("{base}/latest");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("tasks-pending/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
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
}
