//! The daemon's HTTP contract, as far as the window and the daemon must
//! agree on it: the header the state-changing routes require, the refresh
//! cooldown, and the small answers that are not part of the snapshot.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The daemon's default address, written once for the two constants below.
macro_rules! default_listen_addr {
    () => {
        "127.0.0.1:61000"
    };
}

/// Where `tasks-pending serve` listens unless told otherwise: loopback
/// only, on a port outside the usual development range.
pub const DEFAULT_LISTEN_ADDR: &str = default_listen_addr!();

/// [`DEFAULT_LISTEN_ADDR`] as the address the window reads.
pub const DEFAULT_DAEMON_URL: &str = concat!("http://", default_listen_addr!());

/// Header the window sends with state-changing requests. A web page open
/// in a browser cannot add it without a CORS preflight, which is refused.
pub const DASHBOARD_HEADER: (&str, &str) = ("x-requested-with", "tasks-pending");

/// Minimum wait between refreshes requested over HTTP; each one queries
/// every source, and provider APIs rate-limit. The daemon refuses a sooner
/// refresh with 429, and the window holds its Refresh button for as long.
pub const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);

/// What `POST /api/v1/refresh` answers with 429: how long until a refresh
/// is accepted again.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RefreshWait {
    /// Whole seconds, rounded up, so waiting exactly that long is enough.
    pub retry_after_secs: u64,
}

/// What `GET /healthz` answers.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Health {
    pub status: String,
    /// The daemon's version, e.g. `0.6.0`.
    pub version: String,
}

/// What `GET /api/v1/version` answers.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VersionReport {
    /// The daemon's version.
    pub current: String,
    /// A newer published version, when the daemon has seen one.
    #[serde(default)]
    pub available: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default address is a loopback socket address (the daemon refuses
    /// any other), and the window's default URL points at it.
    #[test]
    fn the_default_address_is_loopback_and_the_url_names_it() {
        let address: std::net::SocketAddr = DEFAULT_LISTEN_ADDR.parse().unwrap();
        assert!(address.ip().is_loopback());
        assert_eq!(address.port(), 61000);
        assert_eq!(DEFAULT_DAEMON_URL, "http://127.0.0.1:61000");
    }

    /// A daemon that does not know of a newer version answers without the
    /// field, as a window must be able to read it.
    #[test]
    fn the_version_report_reads_without_an_available_version() {
        let report: VersionReport = serde_json::from_str(r#"{"current": "0.6.0"}"#).unwrap();
        assert_eq!(
            report,
            VersionReport {
                current: "0.6.0".to_owned(),
                available: None
            }
        );
    }
}
