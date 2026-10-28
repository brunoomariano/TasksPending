//! The app's side of the daemon's API: one method per route, over loopback
//! only. The daemon has no authentication, so the client refuses any other
//! address, and it sends the header the state-changing routes require.

use std::time::Duration;

use chrono::{DateTime, Utc};
use pending_core::api::DASHBOARD_HEADER;
use pending_core::{DashboardSnapshot, WeatherAnswer, WeatherReport};
use reqwest::{StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub use pending_core::api::{Health, VersionReport};

/// Where `tasks-pending serve` listens by default.
pub const DEFAULT_DAEMON_URL: &str = "http://127.0.0.1:61000";

/// Longest wait for one answer. The daemon answers from memory, so this is
/// generous; it only ends a request the daemon will never answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The wait reported for a refresh the daemon refused without saying for
/// how long: short, so the window tries again soon rather than never.
const COOLDOWN_FALLBACK_SECS: u64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The address is not `http://` on a loopback host.
    #[error("{0} is not a local http address")]
    NotLocal(String),
    /// No daemon answered (connection refused, reset, ...).
    #[error("{0}")]
    Transport(String),
    #[error("request timed out")]
    Timeout,
    /// The daemon answered with an error status.
    #[error("HTTP {0}")]
    Status(u16),
    /// A refresh came too soon after the previous one.
    #[error("wait {0}s")]
    Cooldown(u64),
    /// The body was not what this version of the app expects.
    #[error("unexpected answer: {0}")]
    Shape(String),
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_decode() {
            Self::Shape(error.to_string())
        } else {
            // Without the URL: it is the same for every request and would
            // only make the message longer.
            Self::Transport(error.without_url().to_string())
        }
    }
}

#[derive(Debug, Deserialize)]
struct Wait {
    retry_after_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
struct MarkRequest<'a> {
    id: &'a str,
    marked: bool,
}

#[derive(Debug, Serialize)]
struct SnoozeRequest<'a> {
    id: &'a str,
    snoozed: bool,
    until: Option<DateTime<Utc>>,
}

/// A connection to one daemon.
#[derive(Clone, Debug)]
pub struct Client {
    base: Url,
    http: reqwest::Client,
}

impl Client {
    /// A client for the daemon at `base`, e.g. [`DEFAULT_DAEMON_URL`].
    pub fn new(base: &str) -> Result<Self, Error> {
        Self::with_timeout(base, REQUEST_TIMEOUT)
    }

    /// [`Client::new`] giving up on an answer after `timeout` instead of
    /// [`REQUEST_TIMEOUT`].
    pub fn with_timeout(base: &str, timeout: Duration) -> Result<Self, Error> {
        let url = Url::parse(base.trim()).map_err(|_| Error::NotLocal(base.to_owned()))?;
        let local = url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if !local {
            return Err(Error::NotLocal(base.to_owned()));
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            // Loopback never goes through a proxy, whatever the environment
            // says.
            .no_proxy()
            // The daemon never redirects; following one would take the
            // request, and the dashboard header, off loopback.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self { base: url, http })
    }

    fn url(&self, path: &str) -> Url {
        // `base` is a valid http URL and `path` is one of ours.
        self.base.join(path).expect("api path")
    }

    /// `GET path`, read as `T`.
    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let response = self.http.get(self.url(path)).send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }
        Ok(response.json().await?)
    }

    /// Whether a daemon answers, and which version it runs.
    pub async fn health(&self) -> Result<Health, Error> {
        self.get_json("/healthz").await
    }

    /// The dashboard as the daemon has it now.
    pub async fn snapshot(&self) -> Result<DashboardSnapshot, Error> {
        self.get_json("/api/v1/snapshot").await
    }

    /// Asks the daemon to refresh every source now. The sources answer in
    /// the background; read the snapshot again a moment later.
    pub async fn refresh(&self) -> Result<(), Error> {
        let response = self
            .http
            .post(self.url("/api/v1/refresh"))
            .header(DASHBOARD_HEADER.0, DASHBOARD_HEADER.1)
            .send()
            .await?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = response
                .json::<Wait>()
                .await
                .ok()
                .and_then(|wait| wait.retry_after_secs)
                .unwrap_or(COOLDOWN_FALLBACK_SECS);
            return Err(Error::Cooldown(wait));
        }
        Self::done(status)
    }

    /// Marks or unmarks a card as in progress.
    pub async fn set_mark(&self, id: &str, marked: bool) -> Result<(), Error> {
        let response = self
            .http
            .post(self.url("/api/v1/marks"))
            .header(DASHBOARD_HEADER.0, DASHBOARD_HEADER.1)
            .json(&MarkRequest { id, marked })
            .send()
            .await?;
        Self::done(response.status())
    }

    /// Hides a card until `until`, or until its item changes when `None`.
    pub async fn snooze(&self, id: &str, until: Option<DateTime<Utc>>) -> Result<(), Error> {
        self.send_snooze(SnoozeRequest {
            id,
            snoozed: true,
            until,
        })
        .await
    }

    /// Brings a snoozed card back now.
    pub async fn wake(&self, id: &str) -> Result<(), Error> {
        self.send_snooze(SnoozeRequest {
            id,
            snoozed: false,
            until: None,
        })
        .await
    }

    async fn send_snooze(&self, request: SnoozeRequest<'_>) -> Result<(), Error> {
        let response = self
            .http
            .post(self.url("/api/v1/snooze"))
            .header(DASHBOARD_HEADER.0, DASHBOARD_HEADER.1)
            .json(&request)
            .send()
            .await?;
        Self::done(response.status())
    }

    /// Tells the daemon the user is looking at the dashboard now.
    pub async fn look(&self) -> Result<(), Error> {
        let response = self
            .http
            .post(self.url("/api/v1/look"))
            .header(DASHBOARD_HEADER.0, DASHBOARD_HEADER.1)
            .send()
            .await?;
        Self::done(response.status())
    }

    /// The daemon's version, and a newer published one when it knows of it.
    pub async fn version(&self) -> Result<VersionReport, Error> {
        self.get_json("/api/v1/version").await
    }

    /// The weather beside the clock, or `None` when there is none to show.
    pub async fn weather(&self) -> Result<Option<WeatherReport>, Error> {
        let answer: WeatherAnswer = self.get_json("/api/v1/weather").await?;
        Ok(answer.report().cloned())
    }

    fn done(status: StatusCode) -> Result<(), Error> {
        if status.is_success() {
            Ok(())
        } else {
            Err(Error::Status(status.as_u16()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::routing::get;
    use chrono::Duration as ChronoDuration;
    use tokio::net::TcpListener;

    use super::*;

    /// `router` on a port of its own, and a client for it.
    async fn serve(router: pending_api::Router) -> Client {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(pending_api::serve_router(listener, router));
        Client::new(&format!("http://{address}")).unwrap()
    }

    /// The sandbox API on a port of its own, and a client for it.
    async fn sandbox() -> Client {
        serve(pending_api::sandbox_app()).await
    }

    fn first_card_id(snapshot: &DashboardSnapshot) -> String {
        snapshot.boards[0].groups[0].columns[0].cards[0].id.clone()
    }

    /// Only loopback http addresses are accepted: the daemon has no
    /// authentication, and the client must never send its requests anywhere
    /// else.
    #[test]
    fn refuses_addresses_that_are_not_local() {
        assert!(Client::new("http://127.0.0.1:61000").is_ok());
        assert!(Client::new("http://localhost:61000/").is_ok());
        assert!(Client::new("http://[::1]:61000").is_ok());
        for address in [
            "https://127.0.0.1:61000",
            "http://example.com",
            "http://10.0.0.5:61000",
            "not a url",
        ] {
            assert!(
                matches!(Client::new(address), Err(Error::NotLocal(_))),
                "{address}"
            );
        }
    }

    /// Health and the snapshot read back as the core types.
    #[tokio::test]
    async fn reads_health_and_the_snapshot() {
        let client = sandbox().await;

        let health = client.health().await.unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.version, env!("CARGO_PKG_VERSION"));

        let snapshot = client.snapshot().await.unwrap();
        assert!(!snapshot.boards.is_empty());
        assert!(!snapshot.sources.is_empty());
    }

    /// Marking, snoozing and waking go through with the header the daemon
    /// requires, and the snapshot shows the result.
    #[tokio::test]
    async fn marks_snoozes_and_wakes_cards() {
        let client = sandbox().await;
        let id = first_card_id(&client.snapshot().await.unwrap());

        client.set_mark(&id, true).await.unwrap();
        assert_eq!(client.snapshot().await.unwrap().marked, vec![id.clone()]);
        client.set_mark(&id, false).await.unwrap();
        assert!(client.snapshot().await.unwrap().marked.is_empty());

        let until = Utc::now() + ChronoDuration::hours(1);
        client.snooze(&id, Some(until)).await.unwrap();
        let snapshot = client.snapshot().await.unwrap();
        assert_eq!(snapshot.snoozed.len(), 1);
        assert_eq!(snapshot.snoozed[0].card.id, id);
        client.wake(&id).await.unwrap();
        assert!(client.snapshot().await.unwrap().snoozed.is_empty());

        assert!(matches!(
            client.set_mark("no:such:card", true).await,
            Err(Error::Status(404))
        ));
        assert!(matches!(
            client
                .snooze(&id, Some(Utc::now() - ChronoDuration::hours(1)))
                .await,
            Err(Error::Status(400))
        ));
    }

    /// A refresh is accepted once and then refused for a while, with the
    /// wait the daemon asks for.
    #[tokio::test]
    async fn refreshes_with_the_daemon_cooldown() {
        let client = sandbox().await;

        client.refresh().await.unwrap();
        assert!(matches!(
            client.refresh().await,
            Err(Error::Cooldown(wait)) if (1..=10).contains(&wait)
        ));
        client.look().await.unwrap();
    }

    /// The sandbox's version report names the daemon version with nothing
    /// newer.
    #[tokio::test]
    async fn reads_the_version_report() {
        let client = sandbox().await;
        let version = client.version().await.unwrap();
        assert_eq!(version.current, env!("CARGO_PKG_VERSION"));
        assert_eq!(version.available, None);
    }

    /// The sandbox weather is the fixed sample.
    #[tokio::test]
    async fn reads_the_weather() {
        let client = sandbox().await;
        let report = client.weather().await.unwrap().unwrap();
        assert_eq!(report.place, "Recife");
        assert_eq!(report.temperature, "29°C");
        assert_eq!(
            report.condition,
            pending_core::WeatherCondition::PartlyCloudyDay
        );
    }

    /// With nothing listening, the error says so instead of hanging.
    #[tokio::test]
    async fn reports_a_missing_daemon() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = Client::new(&format!("http://{address}")).unwrap();
        assert!(matches!(client.health().await, Err(Error::Transport(_))));
    }

    /// A daemon that takes the connection but never answers is given up on
    /// after the timeout.
    #[tokio::test]
    async fn gives_up_on_a_daemon_that_never_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Keep every connection open without answering.
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let client =
            Client::with_timeout(&format!("http://{address}"), Duration::from_millis(200)).unwrap();
        assert!(matches!(client.health().await, Err(Error::Timeout)));
    }

    /// Scenario: something on the daemon's port answers with a redirect.
    /// When the window reads health or posts a change, it does not follow
    /// the redirect elsewhere: the request stays on the loopback address
    /// it was given and the redirect is reported as an error status.
    #[tokio::test]
    async fn does_not_follow_redirects_away_from_the_daemon() {
        let elsewhere_hits = Arc::new(AtomicUsize::new(0));
        let hits = elsewhere_hits.clone();
        let elsewhere = serve(
            pending_api::Router::new()
                .route(
                    "/healthz",
                    get(move || {
                        hits.fetch_add(1, Ordering::SeqCst);
                        async {
                            axum::Json(Health {
                                status: "ok".to_owned(),
                                version: "9.9.9".to_owned(),
                            })
                        }
                    }),
                )
                .route("/api/v1/look", axum::routing::post(|| async { "" })),
        )
        .await;
        let target = elsewhere.base.clone();
        let redirect = move |status: StatusCode, path: &'static str| {
            let location = target.join(path).unwrap().to_string();
            move || async move { (status, [(reqwest::header::LOCATION, location)]) }
        };
        let client = serve(
            pending_api::Router::new()
                .route("/healthz", get(redirect(StatusCode::FOUND, "/healthz")))
                .route(
                    "/api/v1/look",
                    axum::routing::post(redirect(StatusCode::TEMPORARY_REDIRECT, "/api/v1/look")),
                ),
        )
        .await;

        assert!(matches!(client.health().await, Err(Error::Status(302))));
        assert!(matches!(client.look().await, Err(Error::Status(307))));
        assert_eq!(elsewhere_hits.load(Ordering::SeqCst), 0);
    }

    /// An answer that is not what this version expects is reported as
    /// such, and a daemon without the version route (0.5.0) answers it with
    /// a status, not a hang or a panic.
    #[tokio::test]
    async fn reports_answers_it_does_not_understand() {
        let client = serve(
            pending_api::Router::new().route("/api/v1/snapshot", get(|| async { "not json" })),
        )
        .await;
        assert!(matches!(client.snapshot().await, Err(Error::Shape(_))));
        assert!(matches!(client.version().await, Err(Error::Status(404))));
    }
}
