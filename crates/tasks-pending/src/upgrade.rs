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
use pending_api::update::try_latest_release;
use pending_core::release::{bundle_asset, is_newer, parse_version, process_var, release_base_url};
use sha2::{Digest, Sha256};

/// Longest wait to reach the releases host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest pause in a download before giving up on it.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest a whole download may take, like the installer's `--max-time`:
/// the read timeout alone lets a server that trickles go on for ever.
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(15 * 60);
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
    /// Install a release older than the running one.
    pub allow_downgrade: bool,
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

/// The installer a release directory holds.
fn release_installer(lib_dir: &Path) -> PathBuf {
    lib_dir.join("scripts/install.sh")
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

/// What to do about release `target` with `current` running.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Install,
    /// `target` is the running version.
    Installed,
    /// `target` is older, and a downgrade was not asked for.
    Older,
}

fn decide(current: &str, target: &str, allow_downgrade: bool) -> Decision {
    if is_newer(target, current) {
        Decision::Install
    } else if !is_newer(current, target) {
        Decision::Installed
    } else if allow_downgrade {
        Decision::Install
    } else {
        Decision::Older
    }
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

/// Downloads `url` into `path`, giving up past `max_bytes` or past
/// `deadline`: the size is checked as the body arrives, so a wrong or
/// endless answer is cut, not kept.
async fn download(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    max_bytes: u64,
    deadline: Duration,
) -> anyhow::Result<()> {
    tokio::time::timeout(deadline, fetch(client, url, path, max_bytes))
        .await
        .map_err(|_| anyhow::anyhow!("downloading {url} took longer than {deadline:?}"))?
}

async fn fetch(
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
            .block_on(try_latest_release(&base))
            .context("could not read the latest release")?,
    };
    match decide(current, &target, options.allow_downgrade) {
        Decision::Install => {}
        Decision::Installed if pinned.is_none() => {
            println!("tasks-pending {current} is the latest release");
            return Ok(());
        }
        Decision::Installed => {
            println!("tasks-pending {current} is already installed");
            return Ok(());
        }
        Decision::Older if pinned.is_none() => {
            println!("tasks-pending {current} is newer than the latest release, {target}");
            return Ok(());
        }
        Decision::Older => anyhow::bail!(
            "{target} is older than the installed {current}; pass --allow-downgrade to install it anyway"
        ),
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
        download(client, &url, &archive, BUNDLE_MAX_BYTES, DOWNLOAD_DEADLINE).await?;
        download(
            client,
            &format!("{url}.sha256"),
            &checksum,
            CHECKSUM_MAX_BYTES,
            DOWNLOAD_DEADLINE,
        )
        .await
    })?;
    let checksum = fs::read_to_string(&checksum)?;
    if !checksum_matches(&sha256_of(&archive)?, &checksum, &asset) {
        anyhow::bail!("checksum mismatch for {asset}; nothing was installed");
    }
    install_archive(&archive, &asset, work, layout, target)
}

/// Installs release `target` from `archive`, whose checksum was verified,
/// in place of the one in the release directory, in steps that each leave
/// a working release there: unpack and check it in `work`, stage it beside
/// the release directory and run it once, swap it in, then run its
/// installer. When the installer fails, the previous release comes back.
fn install_archive(
    archive: &Path,
    asset: &str,
    work: &Path,
    layout: &Layout,
    target: &str,
) -> anyhow::Result<()> {
    let lib_dir = layout.lib_dir.as_path();
    if lib_dir.exists() {
        anyhow::ensure!(
            release_executable(lib_dir).is_file(),
            "refusing to replace {}: it does not hold a TasksPending release",
            lib_dir.display()
        );
    }
    let bundle = unpack_release(archive, asset, work)?;

    // What an earlier run left beside the release directory goes first.
    let staged = sibling(lib_dir, ".staged");
    let previous = sibling(lib_dir, ".previous");
    remove_if_present(&staged)?;
    remove_if_present(&previous)?;
    fs::rename(&bundle, &staged).context("staging the new release")?;
    if let Err(error) = probe(&staged, target) {
        // The next run removes it if this cannot.
        let _ = fs::remove_dir_all(&staged);
        return Err(error);
    }
    swap_in(&staged, lib_dir, &previous)?;

    match run_installer(layout) {
        Ok(()) => {}
        Err(why) if previous.exists() => return Err(roll_back(layout, &staged, &previous, &why)),
        Err(why) => anyhow::bail!(
            "the bundle's installer {why}; the new release is in {}, run {} --start",
            lib_dir.display(),
            release_installer(lib_dir).display()
        ),
    }
    if let Err(error) = remove_if_present(&previous) {
        eprintln!("the previous release was left in place: {error:#}; remove it by hand");
    }
    println!(
        "upgraded to tasks-pending {target} in {}",
        lib_dir.display()
    );
    Ok(())
}

/// Removes `dir` and what it holds, if it is there.
fn remove_if_present(dir: &Path) -> anyhow::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", dir.display()))
        }
        _ => Ok(()),
    }
}

/// Unpacks `archive` into `work`, once its members are checked, and
/// returns the release directory it holds.
fn unpack_release(archive: &Path, asset: &str, work: &Path) -> anyhow::Result<PathBuf> {
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
        release_installer(&bundle).is_file(),
        "the archive holds no installer"
    );
    Ok(bundle)
}

/// Runs the staged release once: it must run on this machine (its glibc
/// and GTK) and be release `target`.
fn probe(staged: &Path, target: &str) -> anyhow::Result<()> {
    let output = Command::new(release_executable(staged))
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .context("running the new release")?;
    anyhow::ensure!(
        output.status.success(),
        "the new release does not run here ({}); nothing was replaced",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let reported = String::from_utf8_lossy(&output.stdout);
    let reported = reported.trim();
    anyhow::ensure!(
        reported == format!("tasks-pending {target}"),
        "the new release reports {reported}, not {target}; nothing was replaced"
    );
    Ok(())
}

/// Puts the staged release in the release directory, setting the current
/// one aside as `previous`. On failure the current one is back in place,
/// or the error says where it is.
fn swap_in(staged: &Path, lib_dir: &Path, previous: &Path) -> anyhow::Result<()> {
    if lib_dir.exists() {
        fs::rename(lib_dir, previous)
            .context("setting the previous release aside; nothing was replaced")?;
    }
    if let Err(error) = fs::rename(staged, lib_dir) {
        if previous.exists()
            && let Err(back) = fs::rename(previous, lib_dir)
        {
            anyhow::bail!(
                "putting the new release in place failed ({error}), and so did putting the previous one back ({back}): it is in {}, move it to {}",
                previous.display(),
                lib_dir.display()
            );
        }
        return Err(error).context("putting the new release in place; nothing was replaced");
    }
    Ok(())
}

/// Runs the installer of the release in the release directory with
/// `--start`: it links the executable, installs the desktop entry, icon
/// and service, and starts or restarts the daemon. It is part of a release
/// whose checksum was verified, and runs with the same trust as the
/// executable it installs. Says why it failed.
fn run_installer(layout: &Layout) -> Result<(), String> {
    match Command::new(release_installer(&layout.lib_dir))
        .arg("--start")
        .envs(installer_env(layout))
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("exited with {status}")),
        Err(error) => Err(format!("could not run ({error})")),
    }
}

/// After the new release's installer failed (`why`), puts the previous
/// release back and runs its installer again, so its link, unit and daemon
/// are the ones in place. The error says where things stand.
fn roll_back(layout: &Layout, staged: &Path, previous: &Path, why: &str) -> anyhow::Error {
    let lib_dir = &layout.lib_dir;
    let installer = release_installer(lib_dir);
    if let Err(error) = fs::rename(lib_dir, staged) {
        return anyhow::anyhow!(
            "the bundle's installer {why}, and the new release could not be moved aside ({error}): the previous release is in {}; move it back to {} and run {} --start",
            previous.display(),
            lib_dir.display(),
            installer.display()
        );
    }
    if let Err(error) = fs::rename(previous, lib_dir) {
        return anyhow::anyhow!(
            "the bundle's installer {why}, and the previous release could not be put back ({error}): it is in {}; move it to {} and run {} --start",
            previous.display(),
            lib_dir.display(),
            installer.display()
        );
    }
    // The next run removes it if this cannot.
    let _ = fs::remove_dir_all(staged);
    match run_installer(layout) {
        Ok(()) => anyhow::anyhow!(
            "the bundle's installer {why}; the previous release is back in {} and its daemon restarted",
            lib_dir.display()
        ),
        Err(again) => anyhow::anyhow!(
            "the bundle's installer {why}; the previous release is back in {}, but its own installer failed too ({again}): run {} --start",
            lib_dir.display(),
            installer.display()
        ),
    }
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

    /// A release is installed when it is newer than the running one. The
    /// same version is already there, and an older one, pinned or the
    /// latest, only with --allow-downgrade.
    #[test]
    fn installs_only_what_moves_forward_unless_told() {
        assert_eq!(decide("0.6.0", "0.6.1", false), Decision::Install);
        assert_eq!(decide("0.6.0", "0.6.0", false), Decision::Installed);
        assert_eq!(decide("0.6.0", "0.6.0", true), Decision::Installed);
        assert_eq!(decide("0.6.0", "0.0.1", false), Decision::Older);
        assert_eq!(decide("0.6.0", "0.5.9", true), Decision::Install);
    }

    /// A download that keeps trickling is cut at its deadline, however
    /// short each pause is.
    #[test]
    fn gives_up_on_a_download_past_its_deadline() {
        use std::io::{Read, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/bundle", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n");
            for _ in 0..40 {
                if socket.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let scratch = Scratch::new("deadline");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let started = std::time::Instant::now();
        let result = runtime.block_on(download(
            &client,
            &url,
            &scratch.0.join("bundle"),
            1024,
            Duration::from_millis(300),
        ));
        let error = result.unwrap_err().to_string();
        assert!(error.contains("took longer than 300ms"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(runtime);
        server.join().unwrap();
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

    /// Tests that run the fake releases' scripts take turns: a script
    /// written while another test forks could not be run ("text file
    /// busy").
    static SCRIPTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A directory of its own for one test, removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tasks-pending-upgrade-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn layout(&self) -> Layout {
            Layout {
                lib_dir: self.0.join("lib/tasks-pending"),
                bin_dir: self.0.join("bin"),
                home: self.0.join("home"),
            }
        }

        fn log(&self) -> String {
            fs::read_to_string(self.0.join("installer.log")).unwrap_or_default()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_script(path: &Path, content: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A release in `dir` whose executable reports `version` (or cannot
    /// run here, for `None`) and whose installer logs its runs and exits
    /// with `installer_exit`.
    fn fake_release(scratch: &Scratch, dir: &Path, version: Option<&str>, installer_exit: i32) {
        let exe = match version {
            Some(version) => format!("#!/bin/sh\necho 'tasks-pending {version}'\n"),
            None => "#!/bin/sh\necho 'version GLIBC_9.9 not found' >&2\nexit 127\n".to_owned(),
        };
        write_script(&release_executable(dir), &exe);
        write_script(
            &release_installer(dir),
            &format!(
                "#!/bin/sh\necho \"{} $* PREFIX=$PREFIX BIN_DIR=$BIN_DIR RELEASE_BIN=$RELEASE_BIN\" >>'{}'\nexit {installer_exit}\n",
                version.unwrap_or("broken"),
                scratch.0.join("installer.log").display()
            ),
        );
    }

    /// The archive of a fake release, packed as the real bundle is.
    fn fake_archive(scratch: &Scratch, version: Option<&str>, installer_exit: i32) -> PathBuf {
        let build = scratch.0.join("build");
        let name = "tasks-pending-0.0.0-x86_64-unknown-linux-gnu";
        fake_release(scratch, &build.join(name), version, installer_exit);
        let archive = scratch.0.join("bundle.tar.gz");
        tar(&[
            "-C",
            &build.to_string_lossy(),
            "-czf",
            &archive.to_string_lossy(),
            name,
        ])
        .unwrap();
        fs::remove_dir_all(&build).unwrap();
        archive
    }

    /// Installs `archive` as release `target` over the scratch layout.
    fn install(scratch: &Scratch, archive: &Path, target: &str) -> anyhow::Result<()> {
        // `run` makes the release directory's parent, and works in it.
        let work = scratch.0.join("lib/work");
        fs::create_dir_all(&work).unwrap();
        let result = install_archive(archive, "bundle.tar.gz", &work, &scratch.layout(), target);
        let _ = fs::remove_dir_all(&work);
        result
    }

    /// The release the release directory holds, by its executable.
    fn installed(scratch: &Scratch) -> String {
        fs::read_to_string(release_executable(&scratch.layout().lib_dir)).unwrap_or_default()
    }

    fn assert_no_leftovers(scratch: &Scratch) {
        let lib_dir = scratch.layout().lib_dir;
        for leftover in [sibling(&lib_dir, ".staged"), sibling(&lib_dir, ".previous")] {
            assert!(!leftover.exists(), "{} was left behind", leftover.display());
        }
    }

    #[test]
    fn swaps_the_new_release_in_and_runs_its_installer() {
        let _turn = SCRIPTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("swap");
        let layout = scratch.layout();
        fake_release(&scratch, &layout.lib_dir, Some("0.6.0"), 0);
        let archive = fake_archive(&scratch, Some("0.6.1"), 0);

        install(&scratch, &archive, "0.6.1").unwrap();
        assert!(installed(&scratch).contains("0.6.1"));
        assert_no_leftovers(&scratch);
        assert_eq!(
            scratch.log(),
            format!(
                "0.6.1 --start PREFIX={} BIN_DIR={} RELEASE_BIN={}\n",
                layout.home.join(".local").display(),
                layout.bin_dir.display(),
                layout.lib_dir.join("bin").display()
            )
        );
    }

    /// A release that does not run here, or that is not the version asked
    /// for, replaces nothing and leaves nothing behind.
    #[test]
    fn keeps_the_release_when_the_new_one_is_not_right() {
        let _turn = SCRIPTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("probe");
        let layout = scratch.layout();
        fake_release(&scratch, &layout.lib_dir, Some("0.6.0"), 0);

        let broken = fake_archive(&scratch, None, 0);
        let error = install(&scratch, &broken, "0.6.1").unwrap_err().to_string();
        assert!(error.contains("does not run here"), "{error}");
        assert!(error.contains("GLIBC_9.9"), "{error}");
        assert!(installed(&scratch).contains("0.6.0"));
        assert_no_leftovers(&scratch);

        let other = fake_archive(&scratch, Some("0.6.0"), 0);
        let error = install(&scratch, &other, "0.6.1").unwrap_err().to_string();
        assert!(
            error.contains("reports tasks-pending 0.6.0, not 0.6.1"),
            "{error}"
        );
        assert!(installed(&scratch).contains("0.6.0"));
        assert_no_leftovers(&scratch);
        assert_eq!(scratch.log(), "", "no installer runs for a release refused");
    }

    /// When the new release's installer fails, the previous release comes
    /// back and its own installer runs again, so its links, unit and
    /// daemon are the ones in place.
    #[test]
    fn puts_the_previous_release_back_when_the_installer_fails() {
        let _turn = SCRIPTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("rollback");
        let layout = scratch.layout();
        fake_release(&scratch, &layout.lib_dir, Some("0.6.0"), 0);
        let archive = fake_archive(&scratch, Some("0.6.1"), 7);

        let error = install(&scratch, &archive, "0.6.1")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!(
                "the previous release is back in {}",
                layout.lib_dir.display()
            )),
            "{error}"
        );
        assert!(installed(&scratch).contains("0.6.0"));
        assert_no_leftovers(&scratch);
        let runs: Vec<String> = scratch
            .log()
            .lines()
            .map(|line| line.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(runs, ["0.6.1 --start", "0.6.0 --start"]);
    }

    /// When the previous release's installer fails too, the message says
    /// what to run, not that all is well.
    #[test]
    fn says_what_to_run_when_the_previous_installer_fails_too() {
        let _turn = SCRIPTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("rollback-fails");
        let layout = scratch.layout();
        fake_release(&scratch, &layout.lib_dir, Some("0.6.0"), 3);
        let archive = fake_archive(&scratch, Some("0.6.1"), 7);

        let error = install(&scratch, &archive, "0.6.1")
            .unwrap_err()
            .to_string();
        assert!(error.contains("its own installer failed too"), "{error}");
        assert!(
            error.contains(&format!(
                "run {} --start",
                release_installer(&layout.lib_dir).display()
            )),
            "{error}"
        );
        assert!(installed(&scratch).contains("0.6.0"));
        assert_no_leftovers(&scratch);
    }

    /// Without a previous release, the new one stays where it is, and the
    /// message says what to run.
    #[test]
    fn keeps_a_first_release_when_its_installer_fails() {
        let _turn = SCRIPTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("first");
        let layout = scratch.layout();
        let archive = fake_archive(&scratch, Some("0.6.1"), 7);

        let error = install(&scratch, &archive, "0.6.1")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!(
                "the new release is in {}",
                layout.lib_dir.display()
            )),
            "{error}"
        );
        assert!(installed(&scratch).contains("0.6.1"));
        assert_no_leftovers(&scratch);
    }
}
