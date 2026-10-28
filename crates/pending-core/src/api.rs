//! The daemon's HTTP contract, as far as the window and the daemon must
//! agree on it: the header the state-changing routes require, the refresh
//! cooldown, and the small answers that are not part of the snapshot.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Header the window sends with state-changing requests. A web page open
/// in a browser cannot add it without a CORS preflight, which is refused.
pub const DASHBOARD_HEADER: (&str, &str) = ("x-requested-with", "tasks-pending");

/// Minimum wait between refreshes requested over HTTP; each one queries
/// every source, and provider APIs rate-limit. The daemon refuses a sooner
/// refresh with 429, and the window holds its Refresh button for as long.
pub const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);

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
