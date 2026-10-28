//! `tasks-pending setup`: the per-user pieces that installing the files
//! does not cover. Safe to run again: it creates what is missing, keeps
//! what exists, and starts or restarts the daemon.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;
use pending_client::{Client, DEFAULT_DAEMON_URL};

/// The systemd user unit the installers write.
const SERVICE: &str = "tasks-pending.service";
/// How long to wait for the daemon after starting it.
const DAEMON_WAIT: Duration = Duration::from_secs(20);

const CONFIG_EXAMPLE: &str = include_str!("../../../config.example.toml");
const ENV_EXAMPLE: &str = include_str!("../../../packaging/env.example");

#[derive(Debug, Default)]
pub struct Options {
    /// Leave the daemon alone: only the files.
    pub no_start: bool,
}

/// `$XDG_CONFIG_HOME/tasks-pending`, or `~/.config/tasks-pending`.
fn config_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join("tasks-pending"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config").join("tasks-pending"))
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
fn best_effort(program: &str, args: &[&str]) {
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn data_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
}

pub fn run(options: Options) -> anyhow::Result<()> {
    let config_dir = config_dir()?;
    let config = config_dir.join("config.toml");
    if ensure_file(&config, CONFIG_EXAMPLE, 0o644)? {
        println!(
            "created {} from the example: edit it to add your sources",
            config.display()
        );
    } else {
        println!("kept {}", config.display());
    }
    let env = config_dir.join("env");
    if ensure_file(&env, ENV_EXAMPLE, 0o600)? {
        println!(
            "created {} (mode 600): put your tokens there, then run `tasks-pending setup` again",
            env.display()
        );
    } else {
        println!("kept {}", env.display());
    }

    // Launchers read these caches; refreshing them is polite, not required.
    if let Some(data) = data_home() {
        let applications = data.join("applications");
        if applications.is_dir() {
            best_effort(
                "update-desktop-database",
                &[&applications.to_string_lossy()],
            );
        }
        let icons = data.join("icons/hicolor");
        if icons.is_dir() {
            best_effort(
                "gtk-update-icon-cache",
                &["-q", "-t", "-f", &icons.to_string_lossy()],
            );
        }
    }

    if options.no_start {
        println!("daemon left alone (--no-start)");
        return Ok(());
    }
    if Command::new("systemctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        anyhow::bail!(
            "systemctl is required to run the daemon as a user service; start `tasks-pending serve` yourself"
        );
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
    let health = runtime.block_on(async {
        let client = Client::new(&url)?;
        let deadline = Instant::now() + DAEMON_WAIT;
        loop {
            match client.health().await {
                Ok(health) => return Ok(health),
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(error) => return Err(anyhow::anyhow!(error)),
            }
        }
    });
    match health {
        Ok(health) => println!(
            "daemon {} answering at {url}; open the window with `tasks-pending` or from the launcher",
            health.version
        ),
        // The service runs but does not answer where expected: say so
        // and leave it running, rather than fail a setup that worked.
        Err(error) if service_is_active()? => println!(
            "{SERVICE} is active but did not answer at {url} ({error}); if it listens elsewhere, open the window with `tasks-pending --daemon <url>`; see `journalctl --user -u tasks-pending`"
        ),
        Err(error) => anyhow::bail!(
            "{SERVICE} is not running: no answer at {url} ({error}); see `journalctl --user -u tasks-pending`"
        ),
    }
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

    /// The config directory follows XDG_CONFIG_HOME, then HOME.
    #[test]
    fn config_dir_follows_xdg() {
        // Environment variables are process-wide: only read them here.
        let dir = config_dir().unwrap();
        assert!(dir.ends_with("tasks-pending"));
        let example: toml::Value = toml::from_str(CONFIG_EXAMPLE).unwrap();
        assert!(
            example.get("sources").is_some(),
            "the embedded example is a config"
        );
    }
}
