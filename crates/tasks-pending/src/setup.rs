//! `tasks-pending setup`: the per-user pieces that installing the files
//! does not cover. Safe to run again: it creates what is missing, keeps
//! what exists, and starts or restarts the daemon.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;
use pending_client::{Client, DEFAULT_DAEMON_URL};
use pending_runtime::config;

/// The systemd user unit the installers write.
const SERVICE: &str = "tasks-pending.service";
/// How long to wait for the daemon after starting it.
const DAEMON_WAIT: Duration = Duration::from_secs(20);
/// How often to look while waiting.
const DAEMON_POLL: Duration = Duration::from_millis(250);

const CONFIG_EXAMPLE: &str = include_str!("../../../config.example.toml");
const ENV_EXAMPLE: &str = include_str!("../../../packaging/env.example");
#[cfg(test)]
const SERVICE_TEMPLATE: &str = include_str!("../../../packaging/systemd/tasks-pending.service");

#[derive(Debug, Default)]
pub struct Options {
    /// Leave the daemon alone: only the files.
    pub no_start: bool,
}

/// Where setup writes.
#[derive(Debug)]
struct Paths {
    /// Where the daemon looks for its config: `TASKS_PENDING_CONFIG`, or
    /// `config.toml` in the config directory.
    config: PathBuf,
    /// Where the unit reads the tokens from (`%E/tasks-pending/env`): the
    /// config directory, whatever `TASKS_PENDING_CONFIG` says.
    env: PathBuf,
}

/// The config directory is `$XDG_CONFIG_HOME/tasks-pending`, or
/// `~/.config/tasks-pending`; relative values are ignored, as the daemon
/// ignores them.
fn setup_paths(env: &dyn Fn(&str) -> Option<OsString>) -> anyhow::Result<Paths> {
    let missing = || anyhow::anyhow!("neither XDG_CONFIG_HOME nor HOME is an absolute path");
    let config = config::locate(None, env).ok_or_else(missing)?.path;
    let default = config::locate(None, &|key: &str| {
        if key == config::CONFIG_ENV {
            None
        } else {
            env(key)
        }
    })
    .ok_or_else(missing)?
    .path;
    Ok(Paths {
        config,
        env: default.with_file_name("env"),
    })
}

/// The user this process acts as: the kernel makes it the owner of
/// `/proc/self`.
fn effective_uid() -> anyhow::Result<u32> {
    use std::os::unix::fs::MetadataExt;
    Ok(fs::metadata("/proc/self")
        .context("reading /proc/self")?
        .uid())
}

/// Refuses to go on as root: `command` works on one user's home and user
/// service, and root's would serve nobody.
pub(crate) fn refuse_root(command: &str) -> anyhow::Result<()> {
    refuse_root_as(effective_uid()?, command)
}

fn refuse_root_as(euid: u32, command: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        euid != 0,
        "run `tasks-pending {command}` as the user who will use TasksPending, not as root: it works on your home and your user service"
    );
    Ok(())
}

/// Writes `content` to `path` with `mode` unless the file exists. Says
/// whether it was created.
fn ensure_file(path: &Path, content: &str, mode: u32) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(content.as_bytes())?;
    Ok(true)
}

/// The systemctl invocations that start a daemon that is not running, or
/// restart one that is, so a new version takes over.
fn service_plan(active: bool) -> Vec<Vec<&'static str>> {
    let mut plan = vec![vec!["--user", "daemon-reload"]];
    if active {
        plan.push(vec!["--user", "restart", SERVICE]);
    } else {
        plan.push(vec!["--user", "enable", "--now", SERVICE]);
    }
    plan
}

fn systemctl(args: &[&str]) -> anyhow::Result<bool> {
    let status = Command::new("systemctl")
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("running systemctl {}", args.join(" ")))?;
    Ok(status.success())
}

/// Whether a systemd user manager is there to run the daemon.
#[derive(Debug, PartialEq, Eq)]
enum UserManager {
    /// No systemctl: no systemd here.
    Missing,
    /// systemctl is there, but no user manager answers it.
    Unreachable,
    /// It answers, with the environment its services start from
    /// (`show-environment`'s `KEY=value` lines).
    Running(String),
}

/// Asks `systemctl` (run with the given arguments, giving its output when
/// it succeeded) whether a user manager answers.
fn user_manager(systemctl: impl Fn(&[&str]) -> std::io::Result<Option<String>>) -> UserManager {
    match systemctl(&["--user", "show-environment"]) {
        Err(_) => UserManager::Missing,
        Ok(None) => UserManager::Unreachable,
        Ok(Some(environment)) => UserManager::Running(environment),
    }
}

/// `systemctl`'s output when it succeeded; its errors are silenced.
fn systemctl_output(args: &[&str]) -> std::io::Result<Option<String>> {
    let output = Command::new("systemctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// The variables that decide where the daemon reads, as the service sees
/// them: `XDG_CONFIG_HOME` from the user manager when one answers (the
/// unit's `%E`, and the daemon's own), whether or not this shell has
/// another; the rest from `shell`. A value systemd had to quote (`$'…'`)
/// is not an absolute path, and is ignored like any other.
fn service_env<'a>(
    shell: &'a dyn Fn(&str) -> Option<OsString>,
    manager: &'a UserManager,
) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |key| match manager {
        UserManager::Running(environment) if key == "XDG_CONFIG_HOME" => environment
            .lines()
            .find_map(|line| line.strip_prefix("XDG_CONFIG_HOME="))
            .map(OsString::from),
        _ => shell(key),
    }
}

/// What setup reports once the service was started or restarted, from
/// the daemon's answer at `url` (its version, or why there was none). It
/// must answer, and as this release (`own`): anything else means the new
/// daemon is not the one running.
fn judge_answer(answer: Result<String, String>, own: &str, url: &str) -> anyhow::Result<String> {
    match answer {
        Ok(version) if version == own => Ok(format!(
            "daemon {version} answering at {url}; open the window with `tasks-pending` or from the launcher"
        )),
        Ok(version) => anyhow::bail!(
            "the daemon answering at {url} is {version}, not {own}: another tasks-pending holds that address, or {SERVICE} runs another executable; see `systemctl --user status tasks-pending`"
        ),
        Err(error) => anyhow::bail!(
            "{SERVICE} did not answer at {url} within {} s ({error}); see `systemctl --user status tasks-pending` and `journalctl --user -u tasks-pending`",
            DAEMON_WAIT.as_secs()
        ),
    }
}

/// The daemon's version once it answers at `url`, or why it did not
/// within [`DAEMON_WAIT`].
async fn wait_for_daemon(url: &str) -> Result<String, String> {
    let client = Client::new(url).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + DAEMON_WAIT;
    loop {
        match client.health().await {
            Ok(health) => return Ok(health.version),
            Err(_) if Instant::now() < deadline => tokio::time::sleep(DAEMON_POLL).await,
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn service_is_active() -> anyhow::Result<bool> {
    systemctl(&["--user", "is-active", "--quiet", SERVICE])
}

/// The daemon's address from the unit's `ExecStart`, as `systemctl show`
/// prints it (`{ path=... ; argv[]=... serve --listen 127.0.0.1:8090 ;
/// ... }`): a drop-in may have moved it off the default port.
fn listen_from_exec_start(shown: &str) -> Option<String> {
    let argv = shown.split("argv[]=").nth(1)?;
    let argv = argv.split(" ; ").next()?;
    let mut words = argv.split_whitespace();
    while let Some(word) = words.next() {
        let listen = match word {
            "--listen" => words.next(),
            _ => word.strip_prefix("--listen="),
        };
        if let Some(listen) = listen {
            return Some(format!("http://{listen}"));
        }
    }
    None
}

/// Where the service's daemon answers: its `ExecStart`'s `--listen`, or
/// the default address.
fn daemon_url() -> String {
    Command::new("systemctl")
        .args(["--user", "show", "-p", "ExecStart", "--value", SERVICE])
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| listen_from_exec_start(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_else(|| DEFAULT_DAEMON_URL.to_owned())
}

/// Runs a desktop helper if it exists; its failure is not ours.
fn best_effort(program: &str, args: &[String]) {
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The launcher caches to refresh under `data` (`XDG_DATA_HOME`), as the
/// helper and its arguments: the desktop database when there is an
/// applications directory, and the icon cache only when there is one. A
/// new icon cache would hide icons added later, since GTK trusts it while
/// the theme directory's own time is unchanged; without one GTK reads the
/// directories themselves.
fn cache_refreshes(data: &Path) -> Vec<(&'static str, Vec<String>)> {
    let mut refreshes = Vec::new();
    let applications = data.join("applications");
    if applications.is_dir() {
        refreshes.push((
            "update-desktop-database",
            vec![applications.to_string_lossy().into_owned()],
        ));
    }
    let icons = data.join("icons/hicolor");
    if icons.join("icon-theme.cache").is_file() {
        refreshes.push((
            "gtk-update-icon-cache",
            vec![
                "-q".to_owned(),
                "-t".to_owned(),
                "-f".to_owned(),
                icons.to_string_lossy().into_owned(),
            ],
        ));
    }
    refreshes
}

fn data_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
}

pub fn run(options: Options) -> anyhow::Result<()> {
    refuse_root("setup")?;
    let manager = user_manager(systemctl_output);
    let shell = |key: &str| std::env::var_os(key);
    let paths = setup_paths(&service_env(&shell, &manager))?;
    let config = &paths.config;
    if ensure_file(config, CONFIG_EXAMPLE, 0o644)? {
        println!(
            "created {} from the example: edit it to add your sources",
            config.display()
        );
    } else {
        println!("kept {}", config.display());
    }
    let env = &paths.env;
    if ensure_file(env, ENV_EXAMPLE, 0o600)? {
        println!(
            "created {} (mode 600): put your tokens there, then run `tasks-pending setup` again",
            env.display()
        );
    } else {
        println!("kept {}", env.display());
    }

    // Launchers read these caches; refreshing them is polite, not required.
    if let Some(data) = data_home() {
        for (program, args) in cache_refreshes(&data) {
            best_effort(program, &args);
        }
    }

    if options.no_start {
        println!("daemon left alone (--no-start)");
        return Ok(());
    }
    // Without a user manager there is no service to start: the files are
    // in place, and the daemon is the user's to run.
    match manager {
        UserManager::Running(_) => {}
        UserManager::Missing => {
            println!(
                "systemctl not found, so no user service: run `tasks-pending serve` yourself, then open the window"
            );
            return Ok(());
        }
        UserManager::Unreachable => {
            println!(
                "no systemd user manager answers here, so the daemon was not started: run `tasks-pending setup` again from your desktop session, or `tasks-pending serve` yourself"
            );
            return Ok(());
        }
    }
    let active = service_is_active()?;
    for step in service_plan(active) {
        if !systemctl(&step)? {
            anyhow::bail!(
                "systemctl {} failed; see `journalctl --user -u tasks-pending`",
                step.join(" ")
            );
        }
    }
    println!(
        "{} {}",
        if active { "restarted" } else { "started" },
        SERVICE
    );

    let url = daemon_url();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting async runtime")?;
    let answer = runtime.block_on(wait_for_daemon(&url));
    println!("{}", judge_answer(answer, env!("CARGO_PKG_VERSION"), &url)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A missing file is created with the asked mode; an existing one is
    /// kept as it is, tokens included.
    #[test]
    fn creates_missing_files_and_keeps_existing_ones() {
        let dir = std::env::temp_dir().join(format!("tasks-pending-setup-{}", std::process::id()));
        let env = dir.join("env");
        assert!(ensure_file(&env, "TOKEN=\n", 0o600).unwrap());
        assert_eq!(
            fs::metadata(&env).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::write(&env, "TOKEN=mine\n").unwrap();
        assert!(!ensure_file(&env, "TOKEN=\n", 0o600).unwrap());
        assert_eq!(fs::read_to_string(&env).unwrap(), "TOKEN=mine\n");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A running daemon is restarted so the new version takes over; a
    /// stopped one is enabled and started.
    #[test]
    fn restarts_a_running_daemon_and_starts_a_stopped_one() {
        assert_eq!(
            service_plan(true),
            vec![
                vec!["--user", "daemon-reload"],
                vec!["--user", "restart", "tasks-pending.service"]
            ]
        );
        assert_eq!(
            service_plan(false)[1],
            vec!["--user", "enable", "--now", "tasks-pending.service"]
        );
    }

    /// Without systemctl, or without a user manager to answer it (a
    /// container, an ssh session), there is no service to start: setup
    /// says so and leaves the daemon to the user instead of failing.
    #[test]
    fn tells_whether_a_user_manager_runs() {
        let missing = |_: &[&str]| Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(user_manager(missing), UserManager::Missing);
        assert_eq!(
            user_manager(|_: &[&str]| Ok(None)),
            UserManager::Unreachable
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let running = |args: &[&str]| {
            asked.borrow_mut().push(args.join(" "));
            Ok(Some("HOME=/home/me\n".to_owned()))
        };
        assert_eq!(
            user_manager(running),
            UserManager::Running("HOME=/home/me\n".to_owned())
        );
        assert_eq!(asked.into_inner(), ["--user show-environment"]);
    }

    /// The service starts from the user manager's environment, not this
    /// shell's: the env file goes where its `%E` points, and the config
    /// where the daemon looks, both under the manager's XDG_CONFIG_HOME,
    /// or ~/.config when the manager has none. Without a manager the
    /// shell's is all there is.
    #[test]
    fn reads_the_config_home_the_service_sees() {
        let shell = env_of(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "/shell/cfg")]);

        let manager = UserManager::Running("HOME=/home/me\nXDG_CONFIG_HOME=/srv/cfg\n".to_owned());
        let paths = setup_paths(&service_env(&shell, &manager)).unwrap();
        assert_eq!(paths.env, PathBuf::from("/srv/cfg/tasks-pending/env"));
        assert_eq!(
            paths.config,
            PathBuf::from("/srv/cfg/tasks-pending/config.toml")
        );

        let manager = UserManager::Running("HOME=/home/me\nPATH=/usr/bin\n".to_owned());
        let paths = setup_paths(&service_env(&shell, &manager)).unwrap();
        assert_eq!(
            paths.env,
            PathBuf::from("/home/me/.config/tasks-pending/env")
        );

        for manager in [UserManager::Missing, UserManager::Unreachable] {
            let paths = setup_paths(&service_env(&shell, &manager)).unwrap();
            assert_eq!(paths.env, PathBuf::from("/shell/cfg/tasks-pending/env"));
        }
    }

    /// Once the service is started, the daemon must answer, and as this
    /// release: an old daemon still holding the address, or one that never
    /// answers, fails setup, and with it an upgrade, which then puts the
    /// previous release back.
    #[test]
    fn fails_unless_this_release_answers() {
        let url = "http://127.0.0.1:61000";
        let fine = judge_answer(Ok("0.6.0".to_owned()), "0.6.0", url).unwrap();
        assert!(
            fine.contains("daemon 0.6.0 answering at http://127.0.0.1:61000"),
            "{fine}"
        );

        let old = judge_answer(Ok("0.5.0".to_owned()), "0.6.0", url)
            .unwrap_err()
            .to_string();
        assert!(old.contains("is 0.5.0, not 0.6.0"), "{old}");

        let silent = judge_answer(Err("connection refused".to_owned()), "0.6.0", url)
            .unwrap_err()
            .to_string();
        assert!(
            silent.contains("did not answer at http://127.0.0.1:61000"),
            "{silent}"
        );
        assert!(silent.contains("connection refused"), "{silent}");
    }

    /// The daemon's address is read from the unit as systemctl shows it,
    /// so a drop-in with another port is waited for where it listens.
    #[test]
    fn reads_the_listen_address_from_the_unit() {
        let shown = "{ path=/usr/bin/tasks-pending ; argv[]=/usr/bin/tasks-pending serve --listen 127.0.0.1:8090 ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }\n";
        assert_eq!(
            listen_from_exec_start(shown).as_deref(),
            Some("http://127.0.0.1:8090")
        );
        assert_eq!(
            listen_from_exec_start("{ path=/x ; argv[]=/x serve --listen=[::1]:61000 ; pid=0 }")
                .as_deref(),
            Some("http://[::1]:61000")
        );
        assert_eq!(
            listen_from_exec_start("{ path=/x ; argv[]=/x serve ; pid=0 }"),
            None
        );
        assert_eq!(listen_from_exec_start(""), None);
    }

    /// Root is refused: setup and upgrade work on one user's home and user
    /// service, and root's would serve nobody.
    #[test]
    fn refuses_to_run_as_root() {
        let error = refuse_root_as(0, "setup").unwrap_err().to_string();
        assert!(error.contains("not as root"), "{error}");
        assert!(error.contains("tasks-pending setup"), "{error}");
        assert!(refuse_root_as(1000, "setup").is_ok());
        // The effective user is the one that owns what this process creates.
        let file = std::env::temp_dir().join(format!("tasks-pending-owner-{}", std::process::id()));
        fs::write(&file, "").unwrap();
        let owner = std::os::unix::fs::MetadataExt::uid(&fs::metadata(&file).unwrap());
        let _ = fs::remove_file(&file);
        assert_eq!(effective_uid().unwrap(), owner);
    }

    /// The desktop database is refreshed when there is one to refresh. An
    /// icon cache is only ever refreshed, never created: GTK trusts a
    /// cache while the theme directory's own time is unchanged, so a new
    /// one would hide icons added later to a size directory, while without
    /// one GTK reads the directories.
    #[test]
    fn refreshes_only_the_caches_that_exist() {
        let data =
            std::env::temp_dir().join(format!("tasks-pending-caches-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        fs::create_dir_all(data.join("icons/hicolor/256x256/apps")).unwrap();
        assert!(
            cache_refreshes(&data).is_empty(),
            "{:?}",
            cache_refreshes(&data)
        );

        fs::create_dir_all(data.join("applications")).unwrap();
        fs::write(data.join("icons/hicolor/icon-theme.cache"), "").unwrap();
        let programs: Vec<&str> = cache_refreshes(&data)
            .into_iter()
            .map(|(program, _)| program)
            .collect();
        assert_eq!(
            programs,
            ["update-desktop-database", "gtk-update-icon-cache"]
        );
        let _ = fs::remove_dir_all(&data);
    }

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect();
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        }
    }

    /// The config file goes where the daemon looks for it, and the env
    /// file where the unit reads it (`%E/tasks-pending/env`: the
    /// XDG_CONFIG_HOME of the user manager, or ~/.config).
    #[test]
    fn writes_where_the_daemon_and_the_unit_read() {
        let paths = setup_paths(&env_of(&[("HOME", "/home/me")])).unwrap();
        assert_eq!(
            paths.config,
            PathBuf::from("/home/me/.config/tasks-pending/config.toml")
        );
        assert_eq!(
            paths.env,
            PathBuf::from("/home/me/.config/tasks-pending/env")
        );

        let paths = setup_paths(&env_of(&[
            ("HOME", "/home/me"),
            ("XDG_CONFIG_HOME", "/cfg"),
        ]))
        .unwrap();
        assert_eq!(
            paths.config,
            PathBuf::from("/cfg/tasks-pending/config.toml")
        );
        assert_eq!(paths.env, PathBuf::from("/cfg/tasks-pending/env"));

        // A relative XDG_CONFIG_HOME is ignored, per the XDG spec.
        let paths =
            setup_paths(&env_of(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "cfg")])).unwrap();
        assert_eq!(
            paths.env,
            PathBuf::from("/home/me/.config/tasks-pending/env")
        );

        // TASKS_PENDING_CONFIG moves the config, not the env file.
        let paths = setup_paths(&env_of(&[
            ("HOME", "/home/me"),
            ("TASKS_PENDING_CONFIG", "/elsewhere/tp.toml"),
        ]))
        .unwrap();
        assert_eq!(paths.config, PathBuf::from("/elsewhere/tp.toml"));
        assert_eq!(
            paths.env,
            PathBuf::from("/home/me/.config/tasks-pending/env")
        );

        assert!(setup_paths(&env_of(&[])).is_err());

        assert!(
            SERVICE_TEMPLATE
                .lines()
                .any(|line| line == "EnvironmentFile=-%E/tasks-pending/env"),
            "the unit reads the env file from %E, where setup writes it"
        );
        let example: toml::Value = toml::from_str(CONFIG_EXAMPLE).unwrap();
        assert!(
            example.get("sources").is_some(),
            "the embedded example is a config"
        );
    }
}
