//! `tasks-pending upgrade`: fetches the latest release the way the
//! installer does (bundle and published checksum), swaps it in under the
//! release directory and restarts the daemon. Only for installs made by
//! the release installer; a package install upgrades with the package
//! manager.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use pending_core::release::{REPOSITORY, bundle_asset, is_newer, release_base_url};
use sha2::{Digest, Sha256};

/// Longest download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Default)]
pub struct Options {
    /// Only say whether a newer version exists.
    pub check: bool,
    /// Install this version instead of the latest.
    pub version: Option<String>,
}

/// How the running executable was installed.
#[derive(Debug, PartialEq, Eq)]
pub enum Install {
    /// Under the release directory: the installer or a previous upgrade.
    Bundle,
    /// By the package manager.
    Package,
    /// Somewhere else (a source build under a prefix, a checkout).
    Other(PathBuf),
}

fn env_dir(name: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map_or_else(default, PathBuf::from)
}

fn home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

/// `TASKS_PENDING_LIB_DIR`, or `~/.local/lib/tasks-pending`.
pub fn lib_dir() -> anyhow::Result<PathBuf> {
    let home = home()?;
    Ok(env_dir("TASKS_PENDING_LIB_DIR", || {
        home.join(".local/lib/tasks-pending")
    }))
}

/// `TASKS_PENDING_BIN_DIR`, or `~/.local/bin`.
pub fn bin_dir() -> anyhow::Result<PathBuf> {
    let home = home()?;
    Ok(env_dir("TASKS_PENDING_BIN_DIR", || home.join(".local/bin")))
}

/// Where `exe` was installed, given the release directory.
pub fn installed_from(exe: &Path, lib_dir: &Path) -> Install {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let lib_dir = lib_dir
        .canonicalize()
        .unwrap_or_else(|_| lib_dir.to_path_buf());
    if exe.starts_with(&lib_dir) {
        Install::Bundle
    } else if exe.starts_with("/usr") {
        Install::Package
    } else {
        Install::Other(exe)
    }
}

/// Whether `checksum_text` (the published `.sha256` file) names `asset`
/// with the hash of `bytes`.
pub fn checksum_matches(bytes: &[u8], checksum_text: &str, asset: &str) -> bool {
    let actual = format!("{:x}", Sha256::digest(bytes));
    checksum_text.lines().any(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(hash), Some(name)) => {
                hash.eq_ignore_ascii_case(&actual)
                    && (name == asset || name.strip_prefix('*') == Some(asset))
            }
            _ => false,
        }
    })
}

/// Checks a `tar -tzvf` listing: every member is a plain file or a
/// directory inside the archive. Returns the offending member otherwise.
pub fn unsafe_member(listing: &str) -> Option<String> {
    for line in listing.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let kind = line.chars().next()?;
        let name = line.split_whitespace().last().unwrap_or_default();
        if !matches!(kind, '-' | 'd') {
            return Some(name.to_owned());
        }
        if name.starts_with('/')
            || Path::new(name)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Some(name.to_owned());
        }
    }
    None
}

fn tar(args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("tar")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running tar")?;
    if !output.status.success() {
        anyhow::bail!(
            "tar {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn download(client: &reqwest::Client, url: &str) -> anyhow::Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("downloading {url}: HTTP {status}");
    }
    Ok(response.bytes().await?.to_vec())
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let lib_dir = lib_dir()?;
    let bin_dir = bin_dir()?;
    let base = release_base_url(REPOSITORY);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(concat!("tasks-pending/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let target = match &options.version {
        Some(version) => version.trim().trim_start_matches('v').to_owned(),
        None => runtime
            .block_on(pending_api::update::latest_release(&base))
            .with_context(|| format!("could not read the latest release from {base}/latest"))?,
    };
    if options.version.is_none() && !is_newer(&target, current) {
        println!("tasks-pending {current} is the latest release");
        return Ok(());
    }
    if options.check {
        println!(
            "tasks-pending {current} is installed; {target} is available (run `tasks-pending upgrade`)"
        );
        return Ok(());
    }

    let exe = std::env::current_exe().context("finding the running executable")?;
    match installed_from(&exe, &lib_dir) {
        Install::Bundle => {}
        Install::Package => anyhow::bail!(
            "this tasks-pending was installed by the package manager; upgrade it with pacman, or with the release installer"
        ),
        Install::Other(path) => anyhow::bail!(
            "{} is not under {} (the release directory); upgrade the way it was installed (make install / make deploy), or run the release installer",
            path.display(),
            lib_dir.display()
        ),
    }

    let asset = bundle_asset(&target);
    let url = format!("{base}/download/v{target}/{asset}");
    println!("downloading {url}");
    let (archive, checksum) = runtime.block_on(async {
        let archive = download(&client, &url).await?;
        let checksum = download(&client, &format!("{url}.sha256")).await?;
        anyhow::Ok((archive, String::from_utf8_lossy(&checksum).into_owned()))
    })?;
    if !checksum_matches(&archive, &checksum, &asset) {
        anyhow::bail!("checksum mismatch for {asset}; nothing was installed");
    }

    let parent = lib_dir
        .parent()
        .map(Path::to_path_buf)
        .context("the release directory has no parent")?;
    fs::create_dir_all(&parent)?;
    let work = parent.join(format!(".tasks-pending-upgrade-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = install_archive(&archive, &asset, &work, &lib_dir, &bin_dir, &target);
    let _ = fs::remove_dir_all(&work);
    result
}

fn install_archive(
    archive: &[u8],
    asset: &str,
    work: &Path,
    lib_dir: &Path,
    bin_dir: &Path,
    target: &str,
) -> anyhow::Result<()> {
    let archive_path = work.join(asset);
    fs::write(&archive_path, archive)?;
    let listing = tar(&["-tzvf", &archive_path.to_string_lossy()])?;
    if let Some(member) = unsafe_member(&listing) {
        anyhow::bail!("refusing {asset}: member {member} is not a plain file inside the archive");
    }
    let unpack = work.join("unpack");
    fs::create_dir_all(&unpack)?;
    tar(&[
        "-xzf",
        &archive_path.to_string_lossy(),
        "-C",
        &unpack.to_string_lossy(),
    ])?;
    let bundle = fs::read_dir(&unpack)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("tasks-pending-"))
        })
        .context("the archive holds no tasks-pending-* directory")?;
    anyhow::ensure!(
        bundle.join("bin/tasks-pending").is_file(),
        "the archive holds no executable"
    );
    anyhow::ensure!(
        bundle.join("scripts/install.sh").is_file(),
        "the archive holds no installer"
    );

    // Swap: keep the previous release until the new one is in place.
    let staged = lib_dir.with_extension("staged");
    let previous = lib_dir.with_extension("previous");
    let _ = fs::remove_dir_all(&staged);
    let _ = fs::remove_dir_all(&previous);
    fs::rename(&bundle, &staged).context("staging the new release")?;
    if lib_dir.exists() {
        fs::rename(lib_dir, &previous).context("setting the previous release aside")?;
    }
    if let Err(error) = fs::rename(&staged, lib_dir) {
        if previous.exists() {
            let _ = fs::rename(&previous, lib_dir);
        }
        return Err(error).context("putting the new release in place");
    }
    let _ = fs::remove_dir_all(&previous);

    // The bundle's installer links the executable, installs the desktop
    // entry, icon and service, and restarts the daemon.
    let prefix = bin_dir.parent().unwrap_or(bin_dir);
    let status = Command::new(lib_dir.join("scripts/install.sh"))
        .arg("--start")
        .env("PREFIX", prefix)
        .env("INSTALL_LINK", "1")
        .env("RELEASE_BIN", lib_dir.join("bin"))
        .env("SKIP_BUILD", "1")
        .status()
        .context("running the bundle's installer")?;
    anyhow::ensure!(status.success(), "the bundle's installer failed");
    println!(
        "upgraded to tasks-pending {target} in {}",
        lib_dir.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_a_bundle_install_from_a_package_or_a_checkout() {
        let root =
            std::env::temp_dir().join(format!("tasks-pending-upgrade-{}", std::process::id()));
        let lib = root.join("lib/tasks-pending");
        fs::create_dir_all(lib.join("bin")).unwrap();
        fs::write(lib.join("bin/tasks-pending"), "").unwrap();
        assert_eq!(
            installed_from(&lib.join("bin/tasks-pending"), &lib),
            Install::Bundle
        );
        assert_eq!(
            installed_from(Path::new("/usr/bin/tasks-pending"), &lib),
            Install::Package
        );
        let checkout = root.join("target/release/tasks-pending");
        assert!(matches!(installed_from(&checkout, &lib), Install::Other(_)));
        let _ = fs::remove_dir_all(&root);
    }

    /// The published checksum file is `<hash>  <asset>`; the hash must
    /// match and the name must be the asset's.
    #[test]
    fn checks_the_published_checksum() {
        let bytes = b"release";
        let hash = format!("{:x}", Sha256::digest(bytes));
        assert!(checksum_matches(
            bytes,
            &format!("{hash}  bundle.tar.gz\n"),
            "bundle.tar.gz"
        ));
        assert!(checksum_matches(
            bytes,
            &format!("{hash} *bundle.tar.gz\n"),
            "bundle.tar.gz"
        ));
        assert!(!checksum_matches(
            bytes,
            &format!("{hash}  other.tar.gz\n"),
            "bundle.tar.gz"
        ));
        assert!(!checksum_matches(
            b"tampered",
            &format!("{hash}  bundle.tar.gz\n"),
            "bundle.tar.gz"
        ));
        assert!(!checksum_matches(bytes, "", "bundle.tar.gz"));
    }

    /// Members that could write outside the unpack directory, or that are
    /// not plain files, are refused.
    #[test]
    fn refuses_members_that_escape_or_link() {
        let fine = "drwxr-xr-x me/me 0 2026-10-27 10:00 tasks-pending-0.6.0/\n-rwxr-xr-x me/me 10 2026-10-27 10:00 tasks-pending-0.6.0/bin/tasks-pending\n";
        assert_eq!(unsafe_member(fine), None);
        assert_eq!(
            unsafe_member("-rw-r--r-- me/me 0 2026-10-27 10:00 ../outside\n"),
            Some("../outside".to_owned())
        );
        assert_eq!(
            unsafe_member("-rw-r--r-- me/me 0 2026-10-27 10:00 /etc/passwd\n"),
            Some("/etc/passwd".to_owned())
        );
        assert_eq!(
            unsafe_member("lrwxrwxrwx me/me 0 2026-10-27 10:00 x/link -> /etc/passwd\n"),
            Some("/etc/passwd".to_owned())
        );
    }
}
