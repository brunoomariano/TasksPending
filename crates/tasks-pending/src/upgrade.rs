//! `tasks-pending upgrade`: fetches the latest release the way the
//! installer does (bundle and published checksum), swaps it in under the
//! release directory and restarts the daemon. Only for installs made by
//! the release installer; a package install upgrades with the package
//! manager.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use pending_core::release::{bundle_asset, is_newer, parse_version, process_var, release_base_url};
use sha2::{Digest, Sha256};

/// Longest wait to reach the releases host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest pause in a download before giving up on it.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest bundle accepted; the real one is a few tens of megabytes.
const BUNDLE_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Largest checksum file accepted; the real one is a line.
const CHECKSUM_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Default)]
pub struct Options {
    /// Only say whether a newer version exists.
    pub check: bool,
    /// Install this version instead of the latest; `latest` is the latest.
    pub version: Option<String>,
}

/// How the running executable was installed.
#[derive(Debug, PartialEq, Eq)]
enum Install {
    /// It is the release directory's executable: the installer or a
    /// previous upgrade put it there.
    Bundle,
    /// By the package manager, under `/usr`.
    Package,
    /// Somewhere else (`/usr/local`, a prefix of your own, a checkout).
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

/// Where a bundle install lives.
#[derive(Debug)]
struct Layout {
    /// The unpacked release: `TASKS_PENDING_LIB_DIR`, or
    /// `~/.local/lib/tasks-pending`.
    lib_dir: PathBuf,
    /// Where the `tasks-pending` link goes: `TASKS_PENDING_BIN_DIR`, or
    /// `~/.local/bin`.
    bin_dir: PathBuf,
    /// The user's home, whose `~/.local` holds the desktop entry and icons.
    home: PathBuf,
}

impl Layout {
    fn from_env() -> anyhow::Result<Self> {
        let home = home()?;
        Ok(Self {
            lib_dir: env_dir("TASKS_PENDING_LIB_DIR", || {
                home.join(".local/lib/tasks-pending")
            }),
            bin_dir: env_dir("TASKS_PENDING_BIN_DIR", || home.join(".local/bin")),
            home,
        })
    }
}

/// The executable a release directory holds.
fn release_executable(lib_dir: &Path) -> PathBuf {
    lib_dir.join("bin/tasks-pending")
}

/// Where `exe` was installed, given the release directory.
fn installed_from(exe: &Path, lib_dir: &Path) -> Install {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let released = release_executable(lib_dir);
    let released = released.canonicalize().unwrap_or(released);
    if exe == released {
        Install::Bundle
    } else if exe.starts_with("/usr") && !exe.starts_with("/usr/local") {
        Install::Package
    } else {
        Install::Other(exe)
    }
}

/// `lib_dir` with `suffix` appended to its name (`tasks-pending.staged`),
/// whatever the name holds and however many slashes end it.
fn sibling(lib_dir: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = lib_dir.components().collect::<PathBuf>().into_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// The environment the bundle's installer runs with: link the release's
/// executable into the bin directory, and put the desktop entry, icons
/// and examples under `~/.local`, where launchers look.
fn installer_env(layout: &Layout) -> Vec<(&'static str, OsString)> {
    vec![
        ("PREFIX", layout.home.join(".local").into()),
        ("BIN_DIR", layout.bin_dir.clone().into()),
        ("INSTALL_LINK", "1".into()),
        ("RELEASE_BIN", layout.lib_dir.join("bin").into()),
        ("SKIP_BUILD", "1".into()),
    ]
}

/// The version asked for on the command line, or `None` for the latest.
fn requested_version(text: &str) -> anyhow::Result<Option<String>> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("latest") {
        return Ok(None);
    }
    let version = text.strip_prefix('v').unwrap_or(text);
    anyhow::ensure!(
        parse_version(version).is_some(),
        "{text} is not a version (expected major.minor.patch, e.g. 0.6.1)"
    );
    Ok(Some(version.to_owned()))
}

/// Whether `checksum_text` (the published `.sha256` file) names `asset`
/// with `hash` (lower-case hex).
fn checksum_matches(hash: &str, checksum_text: &str, asset: &str) -> bool {
    checksum_text.lines().any(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(expected), Some(name)) => {
                expected.eq_ignore_ascii_case(hash)
                    && (name == asset || name.strip_prefix('*') == Some(asset))
            }
            _ => false,
        }
    })
}

fn sha256_of(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Checks an archive's members from two listings of it: `names` from
/// `tar -tzf` (one name per line) and `verbose` from `tar -tzvf` (the
/// type in the first column). Every member must be a plain file or a
/// directory inside the archive. Says what is wrong otherwise.
fn check_members(names: &str, verbose: &str) -> Result<(), String> {
    let names: Vec<&str> = names.lines().filter(|line| !line.is_empty()).collect();
    let kinds: Vec<char> = verbose
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.chars().next())
        .collect();
    if names.len() != kinds.len() {
        return Err("the two listings of the archive do not agree".to_owned());
    }
    for (name, kind) in names.into_iter().zip(kinds) {
        if !matches!(kind, '-' | 'd') {
            return Err(format!("member {name} is not a plain file or directory"));
        }
        if name.starts_with('/')
            || Path::new(name)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(format!("member {name} escapes the archive"));
        }
    }
    Ok(())
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

/// Downloads `url` into `path`, giving up past `max_bytes`: the size is
/// checked as the body arrives, so a wrong or endless answer is cut, not
/// kept.
async fn download(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    max_bytes: u64,
) -> anyhow::Result<()> {
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("downloading {url}: HTTP {status}");
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        anyhow::bail!("downloading {url}: larger than {max_bytes} bytes");
    }
    let mut file =
        fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut written = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("downloading {url}"))?
    {
        written += chunk.len() as u64;
        if written > max_bytes {
            anyhow::bail!("downloading {url}: larger than {max_bytes} bytes");
        }
        file.write_all(&chunk)?;
    }
    file.flush()?;
    Ok(())
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let base = release_base_url(&process_var)?;
    let pinned = options
        .version
        .as_deref()
        .map(requested_version)
        .transpose()?
        .flatten();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    // https only, also for the redirects GitHub sends downloads through.
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent(concat!("tasks-pending/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let target = match &pinned {
        Some(version) => version.clone(),
        None => runtime
            .block_on(pending_api::update::latest_release(&base))
            .with_context(|| format!("could not read the latest release from {base}/latest"))?,
    };
    if pinned.is_none() && !is_newer(&target, current) {
        println!("tasks-pending {current} is the latest release");
        return Ok(());
    }
    if options.check {
        println!(
            "tasks-pending {current} is installed; {target} is available (run `tasks-pending upgrade`)"
        );
        return Ok(());
    }

    crate::setup::refuse_root("upgrade")?;
    let layout = Layout::from_env()?;
    let lib_dir = &layout.lib_dir;
    let exe = std::env::current_exe().context("finding the running executable")?;
    match installed_from(&exe, lib_dir) {
        Install::Bundle => {}
        Install::Package => anyhow::bail!(
            "this tasks-pending was installed by the package manager; upgrade it with pacman, or with the release installer"
        ),
        Install::Other(path) => anyhow::bail!(
            "{} is not {} (the release directory's executable); upgrade the way it was installed (make install / make deploy), or run the release installer",
            path.display(),
            release_executable(lib_dir).display()
        ),
    }

    let parent = lib_dir
        .parent()
        .map(Path::to_path_buf)
        .context("the release directory has no parent")?;
    fs::create_dir_all(&parent)?;
    let work = parent.join(format!(".tasks-pending-upgrade-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = upgrade_in(&runtime, &client, &base, &target, &work, &layout);
    let _ = fs::remove_dir_all(&work);
    result
}

/// Downloads release `target` into `work`, checks it and installs it.
fn upgrade_in(
    runtime: &tokio::runtime::Runtime,
    client: &reqwest::Client,
    base: &str,
    target: &str,
    work: &Path,
    layout: &Layout,
) -> anyhow::Result<()> {
    let asset = bundle_asset(target);
    let url = format!("{base}/download/v{target}/{asset}");
    let archive = work.join(&asset);
    let checksum = work.join(format!("{asset}.sha256"));
    println!("downloading {url}");
    runtime.block_on(async {
        download(client, &url, &archive, BUNDLE_MAX_BYTES).await?;
        download(
            client,
            &format!("{url}.sha256"),
            &checksum,
            CHECKSUM_MAX_BYTES,
        )
        .await
    })?;
    let checksum = fs::read_to_string(&checksum)?;
    if !checksum_matches(&sha256_of(&archive)?, &checksum, &asset) {
        anyhow::bail!("checksum mismatch for {asset}; nothing was installed");
    }
    install_archive(&archive, &asset, work, layout, target)
}

fn install_archive(
    archive: &Path,
    asset: &str,
    work: &Path,
    layout: &Layout,
    target: &str,
) -> anyhow::Result<()> {
    let lib_dir = layout.lib_dir.as_path();
    let archive_path = archive.to_string_lossy();
    let names = tar(&["-tzf", &archive_path])?;
    let verbose = tar(&["-tzvf", &archive_path])?;
    check_members(&names, &verbose).map_err(|why| anyhow::anyhow!("refusing {asset}: {why}"))?;
    let unpack = work.join("unpack");
    fs::create_dir_all(&unpack)?;
    tar(&["-xzf", &archive_path, "-C", &unpack.to_string_lossy()])?;
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
        release_executable(&bundle).is_file(),
        "the archive holds no executable"
    );
    anyhow::ensure!(
        bundle.join("scripts/install.sh").is_file(),
        "the archive holds no installer"
    );
    if lib_dir.exists() {
        anyhow::ensure!(
            release_executable(lib_dir).is_file(),
            "refusing to replace {}: it does not hold a TasksPending release",
            lib_dir.display()
        );
    }

    // Stage the new release beside the current one and make sure it runs
    // on this machine (its glibc and GTK) before anything is replaced.
    let staged = sibling(lib_dir, ".staged");
    let previous = sibling(lib_dir, ".previous");
    let _ = fs::remove_dir_all(&staged);
    let _ = fs::remove_dir_all(&previous);
    fs::rename(&bundle, &staged).context("staging the new release")?;
    let probe = Command::new(release_executable(&staged))
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .context("running the new release")?;
    let reported = String::from_utf8_lossy(&probe.stdout);
    anyhow::ensure!(
        probe.status.success() && reported.starts_with("tasks-pending "),
        "the new release does not run here ({}); nothing was replaced",
        String::from_utf8_lossy(&probe.stderr).trim()
    );

    // Swap: the previous release is kept until the new one is in place
    // and set up, and comes back if either step fails.
    if lib_dir.exists() {
        fs::rename(lib_dir, &previous).context("setting the previous release aside")?;
    }
    if let Err(error) = fs::rename(&staged, lib_dir) {
        if previous.exists() {
            let _ = fs::rename(&previous, lib_dir);
        }
        return Err(error).context("putting the new release in place");
    }

    // The bundle's installer links the executable, installs the desktop
    // entry, icon and service, and restarts the daemon. It is part of the
    // release whose checksum was just verified, and runs with the same
    // trust as the executable it installs.
    let installer = Command::new(lib_dir.join("scripts/install.sh"))
        .arg("--start")
        .envs(installer_env(layout))
        .status()
        .context("running the bundle's installer");
    match installer {
        Ok(status) if status.success() => {}
        failed => {
            let why = match failed {
                Ok(status) => format!("exited with {status}"),
                Err(error) => error.to_string(),
            };
            if previous.exists() {
                let _ = fs::rename(lib_dir, &staged);
                let _ = fs::rename(&previous, lib_dir);
                let _ = fs::remove_dir_all(&staged);
                anyhow::bail!(
                    "the bundle's installer {why}; the previous release is back in {}",
                    lib_dir.display()
                );
            }
            anyhow::bail!(
                "the bundle's installer {why}; the new release is in {}, run its scripts/install.sh --start",
                lib_dir.display()
            );
        }
    }
    let _ = fs::remove_dir_all(&previous);
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
        // Under the release directory is not enough: it must be its
        // executable, or a stray directory could be swapped away.
        fs::write(lib.join("bin/other"), "").unwrap();
        assert!(matches!(
            installed_from(&lib.join("bin/other"), &lib),
            Install::Other(_)
        ));
        assert_eq!(
            installed_from(Path::new("/usr/bin/tasks-pending"), &lib),
            Install::Package
        );
        assert!(
            matches!(
                installed_from(Path::new("/usr/local/bin/tasks-pending"), &lib),
                Install::Other(_)
            ),
            "/usr/local is a source install, not a package"
        );
        let checkout = root.join("target/release/tasks-pending");
        assert!(matches!(installed_from(&checkout, &lib), Install::Other(_)));
        let _ = fs::remove_dir_all(&root);
    }

    /// The staging directories sit beside the release directory, whatever
    /// its name: a dot in it is not an extension to replace.
    #[test]
    fn stages_beside_the_release_directory() {
        assert_eq!(
            sibling(Path::new("/home/me/.local/lib/tasks-pending"), ".staged"),
            PathBuf::from("/home/me/.local/lib/tasks-pending.staged")
        );
        assert_eq!(
            sibling(Path::new("/opt/tp.v1"), ".previous"),
            PathBuf::from("/opt/tp.v1.previous")
        );
        // TASKS_PENDING_LIB_DIR=/opt/tp/ is /opt/tp, not a directory in it.
        assert_eq!(
            sibling(Path::new("/opt/tp/"), ".staged"),
            PathBuf::from("/opt/tp.staged")
        );
    }

    /// The bundle's installer links the release into the executable's
    /// directory; the desktop entry, icons and examples stay under
    /// `~/.local`, wherever that directory is.
    #[test]
    fn tells_the_installer_where_things_go() {
        let env = installer_env(&Layout {
            lib_dir: PathBuf::from("/home/me/.local/lib/tasks-pending"),
            bin_dir: PathBuf::from("/home/me/bin"),
            home: PathBuf::from("/home/me"),
        });
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| PathBuf::from(value))
        };
        assert_eq!(get("PREFIX"), Some(PathBuf::from("/home/me/.local")));
        assert_eq!(get("BIN_DIR"), Some(PathBuf::from("/home/me/bin")));
        assert_eq!(
            get("RELEASE_BIN"),
            Some(PathBuf::from("/home/me/.local/lib/tasks-pending/bin"))
        );
        assert_eq!(get("INSTALL_LINK"), Some(PathBuf::from("1")));
        assert_eq!(get("SKIP_BUILD"), Some(PathBuf::from("1")));
    }

    /// `--version` takes a version, with or without the `v`; `latest` and
    /// nothing mean the latest; anything else is refused.
    #[test]
    fn reads_the_requested_version() {
        assert_eq!(
            requested_version("0.6.1").unwrap().as_deref(),
            Some("0.6.1")
        );
        assert_eq!(
            requested_version(" v0.6.1 ").unwrap().as_deref(),
            Some("0.6.1")
        );
        assert_eq!(requested_version("latest").unwrap(), None);
        assert_eq!(requested_version("").unwrap(), None);
        for odd in ["0.6", "nightly", "0.6.1-rc1", "../x"] {
            assert!(requested_version(odd).is_err(), "{odd}");
        }
    }

    /// The published checksum file is `<hash>  <asset>`; the hash must
    /// match and the name must be the asset's.
    #[test]
    fn checks_the_published_checksum() {
        let hash = format!("{:x}", Sha256::digest(b"release"));
        assert!(checksum_matches(
            &hash,
            &format!("{hash}  bundle.tar.gz\n"),
            "bundle.tar.gz"
        ));
        assert!(checksum_matches(
            &hash,
            &format!("{} *bundle.tar.gz\n", hash.to_uppercase()),
            "bundle.tar.gz"
        ));
        assert!(!checksum_matches(
            &hash,
            &format!("{hash}  other.tar.gz\n"),
            "bundle.tar.gz"
        ));
        let other = format!("{:x}", Sha256::digest(b"tampered"));
        assert!(!checksum_matches(
            &other,
            &format!("{hash}  bundle.tar.gz\n"),
            "bundle.tar.gz"
        ));
        assert!(!checksum_matches(&hash, "", "bundle.tar.gz"));

        let dir = std::env::temp_dir().join(format!("tasks-pending-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bundle"), b"release").unwrap();
        assert_eq!(sha256_of(&dir.join("bundle")).unwrap(), hash);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Members that could write outside the unpack directory, or that are
    /// not plain files or directories, are refused by name, even when the
    /// name has spaces or the member is a link to somewhere else.
    #[test]
    fn refuses_members_that_escape_or_link() {
        let names = "tasks-pending-0.6.0/\ntasks-pending-0.6.0/bin/tasks-pending\ntasks-pending-0.6.0/a file\n";
        let verbose = "drwxr-xr-x me/me 0 2026-10-27 10:00 tasks-pending-0.6.0/\n-rwxr-xr-x me/me 10 2026-10-27 10:00 tasks-pending-0.6.0/bin/tasks-pending\n-rw-r--r-- me/me 1 2026-10-27 10:00 tasks-pending-0.6.0/a file\n";
        assert_eq!(check_members(names, verbose), Ok(()));
        assert_eq!(
            check_members(
                "../outside\n",
                "-rw-r--r-- me/me 0 2026-10-27 10:00 ../outside\n"
            ),
            Err("member ../outside escapes the archive".to_owned())
        );
        assert_eq!(
            check_members(
                "/etc/passwd\n",
                "-rw-r--r-- me/me 0 2026-10-27 10:00 /etc/passwd\n"
            ),
            Err("member /etc/passwd escapes the archive".to_owned())
        );
        assert_eq!(
            check_members(
                "x/link\n",
                "lrwxrwxrwx me/me 0 2026-10-27 10:00 x/link -> /etc/passwd\n"
            ),
            Err("member x/link is not a plain file or directory".to_owned())
        );
        assert_eq!(
            check_members(
                "x/hard\n",
                "hrw-r--r-- me/me 0 2026-10-27 10:00 x/hard link to x/other\n"
            ),
            Err("member x/hard is not a plain file or directory".to_owned())
        );
        assert_eq!(
            check_members("a\nb\n", "-rw-r--r-- me/me 0 2026-10-27 10:00 a\n"),
            Err("the two listings of the archive do not agree".to_owned())
        );
    }
}
