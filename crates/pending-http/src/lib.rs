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
    // Parsed the way the HTTP client will parse it, so the host checked
    // here is the host the request goes to (`http://a\@localhost` is `a`).
    let encrypted = reqwest::Url::parse(url.trim()).is_ok_and(|url| match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    });
    if encrypted {
        Ok(())
    } else {
        Err(format!(
            "{variable} must start with https:// so the token is not sent in the clear \
             (http:// is accepted only for localhost); fix it, then restart"
        ))
    }
}

/// Time limits for a source that asks for its stacks in turns: each stack
/// has `per_stack` for all its pages, and all of them together have `total`,
/// which is kept below the aggregator's default source timeout (60 s).
/// Without the total, enough slow stacks would add up past that timeout and
/// fail the whole source, losing the stacks that did answer.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    per_stack: Duration,
    deadline: tokio::time::Instant,
    total: Duration,
}

/// Default [`Budget`] total: what a refresh may take for all its stacks.
pub const DEFAULT_REFRESH_BUDGET: Duration = Duration::from_secs(50);

impl Budget {
    /// Starts the clock of one refresh.
    pub fn start(per_stack: Duration, total: Duration) -> Self {
        Self {
            per_stack,
            deadline: tokio::time::Instant::now() + total,
            total,
        }
    }

    /// Runs one stack within its own limit and what is left of the refresh.
    /// A stack that runs out of time fails with a message to show as that
    /// stack's warning; whatever it had read is dropped, because the stack
    /// would be incomplete.
    pub async fn run<T, E: From<String>>(
        &self,
        stack: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let now = tokio::time::Instant::now();
        let own = now + self.per_stack;
        if now >= self.deadline {
            return Err(self.out_of_time().into());
        }
        match tokio::time::timeout_at(own.min(self.deadline), stack).await {
            Ok(result) => result,
            Err(_) if own <= self.deadline => {
                Err(format!("timed out after {:?}", self.per_stack).into())
            }
            Err(_) => Err(self.out_of_time().into()),
        }
    }

    fn out_of_time(&self) -> String {
        format!(
            "not finished: the refresh used up its {:?} for all stacks",
            self.total
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Budget, require_encrypted};

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
            "http://user@localhost:8080",
            "  https://gitlab.com  ",
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
            // A backslash ends the host, as it does for the HTTP client.
            "http://evil.example\\@localhost",
            "http://evil.example\\@localhost:8080/",
            "http://evil.example#@localhost",
            "http:localhost.evil.example",
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

    async fn after(secs: u64) -> Result<u64, String> {
        tokio::time::sleep(Duration::from_secs(secs)).await;
        Ok(secs)
    }

    /// Each stack has its own limit, and all of them share the refresh's:
    /// a stack cut by the shared limit, or never started because of it,
    /// says so instead of blaming its own speed.
    #[tokio::test(start_paused = true)]
    async fn stacks_share_the_time_of_a_refresh() {
        let budget = Budget::start(Duration::from_secs(25), Duration::from_secs(50));

        assert_eq!(budget.run(after(20)).await, Ok(20));
        assert_eq!(
            budget.run(after(26)).await,
            Err("timed out after 25s".to_owned())
        );
        // 45 s gone: 5 s are left, less than the stack's own 25 s.
        assert_eq!(budget.run(after(3)).await, Ok(3));
        let cut = budget.run(after(10)).await.expect_err("only 2 s left");
        assert!(cut.contains("used up its 50s"), "{cut}");
        let unstarted = budget.run(after(0)).await.expect_err("no time left");
        assert_eq!(unstarted, cut);
    }
}
