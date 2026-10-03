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

/// Checks that credentials sent to `url` travel encrypted: it must be
/// `https://`, or `http://` to this machine (a local proxy or a test
/// server). Refusing redirects does not help here: the first request would
/// already carry the token in the clear. The error names `variable`, the
/// setting that holds the address.
pub fn require_encrypted(url: &str, variable: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let local = lower.strip_prefix("http://").is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // Credentials in the address (`user@host`) are not a host.
        let host_port = authority.rsplit('@').next().unwrap_or_default();
        let host = match host_port.strip_prefix('[') {
            Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
            None => host_port.split(':').next().unwrap_or_default(),
        };
        matches!(host, "localhost" | "127.0.0.1" | "::1")
    });
    if local {
        Ok(())
    } else {
        Err(format!(
            "{variable} must start with https:// so the token is not sent in the clear \
             (http:// is accepted only for localhost); fix it, then restart"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::require_encrypted;

    /// Tokens go only to https addresses, or to this machine over http;
    /// anything else is refused with a message naming the setting.
    #[test]
    fn credentials_need_https_except_on_this_machine() {
        for url in [
            "https://gitlab.com",
            "HTTPS://Jira.Example.com/jira",
            "http://localhost:8080",
            "http://127.0.0.1:61000/api",
            "http://[::1]:9000",
        ] {
            assert_eq!(require_encrypted(url, "X_URL"), Ok(()), "{url}");
        }
        for url in [
            "http://gitlab.com",
            "http://acme.atlassian.net/",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example:80",
            "http://evil.example/?http://localhost",
            "http://localhost@evil.example",
            "gitlab.com",
            "ftp://localhost",
            "",
        ] {
            let error = require_encrypted(url, "X_URL").expect_err(url);
            assert!(
                error.contains("X_URL") && error.contains("https://"),
                "{error}"
            );
        }
    }
}
