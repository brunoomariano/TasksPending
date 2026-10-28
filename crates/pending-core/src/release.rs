//! Releases: where they are published and how versions compare, shared by
//! the daemon's update check and `tasks-pending upgrade`.

/// The GitHub repository the releases come from, `owner/name`.
pub const REPOSITORY: &str = "brunoomariano/TasksPending";

/// The release asset for this platform, by version: the Linux x86_64
/// bundle, the only one published.
pub fn bundle_asset(version: &str) -> String {
    format!("tasks-pending-{version}-x86_64-unknown-linux-gnu.tar.gz")
}

/// The releases URL prefix: `TASKS_PENDING_RELEASE_BASE_URL`, or GitHub.
pub fn release_base_url(repository: &str) -> String {
    std::env::var("TASKS_PENDING_RELEASE_BASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .map(|url| url.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| format!("https://github.com/{repository}/releases"))
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
    fn names_the_bundle_and_the_releases() {
        assert_eq!(
            bundle_asset("0.6.0"),
            "tasks-pending-0.6.0-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert!(release_base_url("o/r").ends_with("/releases"));
    }
}
