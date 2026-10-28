//! Releases: where they are published and how versions compare, shared by
//! the daemon's update check, `tasks-pending upgrade` and the installer.

/// The GitHub repository the releases come from, `owner/name`.
pub const REPOSITORY: &str = "brunoomariano/TasksPending";
/// Names another releases URL prefix (the installer reads it too).
pub const BASE_URL_VARIABLE: &str = "TASKS_PENDING_RELEASE_BASE_URL";
/// Names another GitHub repository, `owner/name`.
pub const REPOSITORY_VARIABLE: &str = "TASKS_PENDING_REPOSITORY";

/// The release asset for this platform, by version: the Linux x86_64
/// bundle, the only one published.
pub fn bundle_asset(version: &str) -> String {
    format!("tasks-pending-{version}-x86_64-unknown-linux-gnu.tar.gz")
}

/// Why the environment's releases location cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseUrlError {
    /// The archive and its checksum come from the same place, so it must
    /// not be one a network could rewrite on the way.
    #[error("{variable} must start with https:// (it is {value})")]
    NotHttps {
        variable: &'static str,
        value: String,
    },
    #[error("{variable} must be owner/name (it is {value})")]
    NotARepository {
        variable: &'static str,
        value: String,
    },
}

/// The process environment, for [`release_base_url`].
pub fn process_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The releases URL prefix: [`BASE_URL_VARIABLE`] when set, else GitHub's
/// for [`REPOSITORY_VARIABLE`] or [`REPOSITORY`]. Only `https://` is
/// accepted. `var` reads the environment (see [`process_var`]).
pub fn release_base_url(var: &dyn Fn(&str) -> Option<String>) -> Result<String, ReleaseUrlError> {
    let set = |name: &str| {
        var(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    if let Some(url) = set(BASE_URL_VARIABLE) {
        if !url.starts_with("https://") {
            return Err(ReleaseUrlError::NotHttps {
                variable: BASE_URL_VARIABLE,
                value: url,
            });
        }
        return Ok(url.trim_end_matches('/').to_owned());
    }
    let repository = set(REPOSITORY_VARIABLE).unwrap_or_else(|| REPOSITORY.to_owned());
    if !is_repository(&repository) {
        return Err(ReleaseUrlError::NotARepository {
            variable: REPOSITORY_VARIABLE,
            value: repository,
        });
    }
    Ok(format!("https://github.com/{repository}/releases"))
}

/// `owner/name`, each a GitHub-style slug, so the value stays a path
/// segment of its own in the URL.
fn is_repository(text: &str) -> bool {
    let slug = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let mut parts = text.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(owner), Some(name), None) if slug(owner) && slug(name)
    )
}

/// The version a release page URL points at: `.../releases/tag/v0.6.1`
/// gives `0.6.1`. That is where `<releases>/latest` redirects.
pub fn version_from_release_url(url: &str) -> Option<String> {
    let tag = url.trim().trim_end_matches('/').rsplit('/').next()?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(version).map(|_| version.to_owned())
}

/// `major.minor.patch` as numbers; `None` for anything else.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Whether `candidate` is a newer release than `current`. Anything that is
/// not a version is not newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_from_the_release_page() {
        assert_eq!(
            version_from_release_url("https://github.com/o/r/releases/tag/v0.6.1"),
            Some("0.6.1".to_owned())
        );
        assert_eq!(
            version_from_release_url("https://example.test/releases/tag/1.2.3/"),
            Some("1.2.3".to_owned())
        );
        assert_eq!(
            version_from_release_url("https://github.com/o/r/releases"),
            None
        );
        assert_eq!(
            version_from_release_url("https://github.com/o/r/releases/tag/nightly"),
            None
        );
    }

    #[test]
    fn compares_versions_as_numbers() {
        assert!(is_newer("0.6.1", "0.6.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.6.0", "0.6.0"));
        assert!(!is_newer("0.5.9", "0.6.0"));
        assert!(!is_newer("0.6", "0.5.0"));
        assert!(!is_newer("latest", "0.5.0"));
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn names_the_bundle() {
        assert_eq!(
            bundle_asset("0.6.0"),
            "tasks-pending-0.6.0-x86_64-unknown-linux-gnu.tar.gz"
        );
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    /// GitHub by default; another repository or another location from
    /// the environment, trimmed; blanks are as good as unset.
    #[test]
    fn the_releases_come_from_github_unless_told_otherwise() {
        assert_eq!(
            release_base_url(&env(&[])).as_deref(),
            Ok("https://github.com/brunoomariano/TasksPending/releases")
        );
        assert_eq!(
            release_base_url(&env(&[(REPOSITORY_VARIABLE, " me/fork ")])).as_deref(),
            Ok("https://github.com/me/fork/releases")
        );
        assert_eq!(
            release_base_url(&env(&[
                (REPOSITORY_VARIABLE, "ignored/when-the-url-is-set"),
                (BASE_URL_VARIABLE, "https://example.test/releases/ ")
            ]))
            .as_deref(),
            Ok("https://example.test/releases")
        );
        assert_eq!(
            release_base_url(&env(&[
                (BASE_URL_VARIABLE, "  "),
                (REPOSITORY_VARIABLE, "")
            ]))
            .as_deref(),
            Ok("https://github.com/brunoomariano/TasksPending/releases")
        );
    }

    /// A plain-text location or a repository that is not `owner/name`
    /// (and could change the path) is refused.
    #[test]
    fn refuses_plain_text_and_odd_repositories() {
        assert_eq!(
            release_base_url(&env(&[(BASE_URL_VARIABLE, "http://example.test/releases")])),
            Err(ReleaseUrlError::NotHttps {
                variable: BASE_URL_VARIABLE,
                value: "http://example.test/releases".to_owned()
            })
        );
        for odd in ["single", "a/b/c", "../x", "a/..", "o/r?x=1", "o/r r", "/r"] {
            assert_eq!(
                release_base_url(&env(&[(REPOSITORY_VARIABLE, odd)])),
                Err(ReleaseUrlError::NotARepository {
                    variable: REPOSITORY_VARIABLE,
                    value: odd.to_owned()
                }),
                "{odd}"
            );
        }
    }
}
