//! HTTP helpers shared by source adapters.

use std::time::Duration;

/// A `reqwest` error and its causes, without the request URL: source URLs can
/// hold secrets (the iCal address) and are noise in the dashboard.
/// `connect_timeout` and `request_timeout` name the limit that was hit.
pub fn describe_error(
    error: reqwest::Error,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> String {
    if error.is_timeout() {
        return if error.is_connect() {
            format!("connection timed out after {connect_timeout:?}")
        } else {
            format!("timed out after {request_timeout:?}")
        };
    }
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
