//! TasksPending's icon on Omarchy's bar: a shell plugin the app carries
//! and, when Settings turns it on, writes into Omarchy's plugin folder and
//! enables, the way flea does with its shelf. A click on it runs
//! `tasks-pending toggle` (see [`crate::dropdown`]).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The plugin's id, which is also its folder's name.
pub const PLUGIN_ID: &str = "io.github.brunoomariano.tasks-pending";

const MANIFEST: &str = include_str!("omarchy-bar/manifest.json");
const WIDGET: &str = include_str!("omarchy-bar/BarWidget.qml");

/// The plugin's files, by name: the manifest carries the app's version.
pub fn files() -> [(&'static str, String); 2] {
    [
        (
            "manifest.json",
            MANIFEST.replace("{{version}}", env!("CARGO_PKG_VERSION")),
        ),
        ("BarWidget.qml", WIDGET.to_owned()),
    ]
}

/// Where Omarchy looks for the plugin, under `home`.
pub fn plugin_dir(home: &Path) -> PathBuf {
    home.join(".config/omarchy/plugins").join(PLUGIN_ID)
}

/// Omarchy's shell settings under `home`, which place the bar's widgets.
pub fn shell_config(home: &Path) -> PathBuf {
    home.join(".config/omarchy/shell.json")
}

/// Writes the plugin into `dir`, only the files that differ, so the
/// shell reloads nothing for an unchanged copy. Says whether it wrote.
pub fn write(dir: &Path) -> io::Result<bool> {
    fs::create_dir_all(dir)?;
    let mut wrote = false;
    for (name, text) in files() {
        let path = dir.join(name);
        if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            fs::write(&path, text)?;
            wrote = true;
        }
    }
    Ok(wrote)
}

/// What the window tells the icon, right in the session's runtime
/// directory (not in a folder of its own, which Quickshell would not see
/// appear): the window writes it, the plugin watches it.
pub const SIGNAL: &str = "tasks-pending-bar.json";

/// The file the window tells the icon through, from the environment
/// (`env` reads a variable); `None` without a runtime directory.
pub fn signal_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let runtime = env("XDG_RUNTIME_DIR").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(runtime).join(SIGNAL))
}

/// What the file says, given the titles of the cards whose alerts rang
/// while the window was away ([`crate::alert::Missed`]): whether the icon
/// shows its dot, and the tooltip naming them.
pub fn signal(missed: &[&str]) -> String {
    let tooltip = match missed {
        [] => "TasksPending".to_owned(),
        [title] => format!("TasksPending · Alert: {title}"),
        titles => format!(
            "TasksPending · {} alerts: {}",
            titles.len(),
            titles.join("; ")
        ),
    };
    serde_json::json!({ "dot": !missed.is_empty(), "tooltip": tooltip }).to_string()
}

/// Writes the file at `path` in one go, a new file moved over the old, so
/// the icon never reads half of one.
pub fn tell(path: &Path, missed: &[&str]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let fresh = path.with_extension("new");
    if let Err(error) = fs::write(&fresh, signal(missed)) {
        let _ = fs::remove_file(&fresh);
        return Err(error);
    }
    fs::rename(&fresh, path)
}

/// Whether the bar shows the icon: Omarchy's `shell.json` places it in
/// one of the bar's sections, as an entry with its id or as the bare id.
pub fn in_bar(shell_json: &str) -> bool {
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(shell_json) else {
        return false;
    };
    let Some(sections) = settings["bar"]["layout"].as_object() else {
        return false;
    };
    sections
        .values()
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .any(|widget| widget == PLUGIN_ID || widget["id"] == PLUGIN_ID)
}

/// Whether Omarchy's shell is here to host the plugin: `omarchy-shell` is
/// on `path` (a `PATH` value).
pub fn available(path: Option<&OsStr>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path).any(|dir| {
        fs::metadata(dir.join("omarchy-shell"))
            .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
    })
}

/// A step in putting the icon on the bar or taking it off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Run this command; it failing stops the rest.
    Run(Vec<&'static str>),
    /// Wait, a couple of seconds at most, until Omarchy's plugin list
    /// ([`LIST`]) holds the plugin (see [`listed`]).
    AwaitListed,
}

/// The command that prints the plugins Omarchy's shell knows, as JSON.
pub const LIST: [&str; 4] = ["omarchy", "plugin", "list", "--json"];

/// The steps that put the icon on the bar (`true`) or take it off, once
/// the plugin is written. The shell learns of a new plugin folder only
/// when asked to look again, and it answers before it has looked, so the
/// plugin is waited for before it is enabled, as Omarchy's own
/// `plugin add` does.
pub fn steps(on: bool) -> Vec<Step> {
    if on {
        vec![
            Step::Run(vec!["omarchy-shell", "shell", "rescanPlugins"]),
            Step::AwaitListed,
            Step::Run(vec!["omarchy", "plugin", "enable", PLUGIN_ID]),
        ]
    } else {
        vec![Step::Run(vec!["omarchy", "plugin", "disable", PLUGIN_ID])]
    }
}

/// Whether Omarchy's plugin list (what [`LIST`] prints) holds the plugin.
pub fn listed(list: &str) -> bool {
    serde_json::from_str::<Vec<serde_json::Value>>(list)
        .is_ok_and(|plugins| plugins.iter().any(|plugin| plugin["id"] == PLUGIN_ID))
}

/// Why a command refused: the last line it wrote to its error output.
pub fn refusal(errors: &str) -> Option<&str> {
    errors
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tasks-pending-bar-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// The plugin is Omarchy's kind of bar widget: its manifest names it,
    /// at the app's version, with an entry point that is one of its files,
    /// and a click runs `tasks-pending toggle` (the command a setting).
    #[test]
    fn carries_a_bar_widget_that_toggles_the_window() {
        let files = files();
        let manifest: serde_json::Value = serde_json::from_str(&files[0].1).unwrap();
        assert_eq!(files[0].0, "manifest.json");
        assert_eq!(manifest["schemaVersion"], 1);
        assert_eq!(manifest["id"], PLUGIN_ID);
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest["kinds"], serde_json::json!(["bar-widget"]));
        let entry = manifest["entryPoints"]["barWidget"].as_str().unwrap();
        let widget = &files.iter().find(|(name, _)| *name == entry).unwrap().1;
        assert_eq!(
            manifest["barWidget"]["defaults"]["command"],
            "tasks-pending"
        );
        assert!(widget.contains(&format!("moduleName: \"{PLUGIN_ID}\"")));
        // Font Awesome's tasks mark: without a glyph the bar hides the
        // button, and the click and the tooltip with it.
        assert!(widget.contains(r#"text: "\uf0ae""#), "{widget}");
        assert!(
            widget
                .contains(r#"Util.execArgv([root.setting("command", "tasks-pending"), "toggle"])"#),
            "{widget}"
        );
    }

    /// The window tells the icon through a file right in the session's
    /// runtime directory, which is there before the shell starts: Quickshell
    /// sees a file appear in a folder it watches, never in one that did not
    /// exist yet. Without a runtime directory, it tells it nothing.
    #[test]
    fn tells_the_icon_through_the_runtime_directory() {
        let runtime = |value: &'static str| {
            move |name: &str| (name == "XDG_RUNTIME_DIR").then(|| value.to_owned())
        };
        assert_eq!(
            signal_path(&runtime("/run/user/1000")),
            Some(PathBuf::from("/run/user/1000/tasks-pending-bar.json"))
        );
        assert_eq!(signal_path(&runtime("")), None);
        assert_eq!(signal_path(&|_| None), None);
    }

    /// The icon shows its dot while alerts were missed, and its tooltip
    /// names them; with none, no dot and its own name.
    #[test]
    fn the_dot_names_the_alerts_missed() {
        let told = |missed: &[&str]| -> serde_json::Value {
            serde_json::from_str(&signal(missed)).unwrap()
        };
        assert_eq!(
            told(&[]),
            serde_json::json!({"dot": false, "tooltip": "TasksPending"})
        );
        assert_eq!(
            told(&["Review the release checklist"]),
            serde_json::json!({
                "dot": true,
                "tooltip": "TasksPending · Alert: Review the release checklist"
            })
        );
        assert_eq!(
            told(&["Review", "Deploy"]),
            serde_json::json!({"dot": true, "tooltip": "TasksPending · 2 alerts: Review; Deploy"})
        );
    }

    /// Telling writes the folder and the file whole, and telling again
    /// replaces it, leaving nothing else beside it.
    #[test]
    fn tells_in_one_go() {
        let dir = scratch("tell");
        let path = dir.join("tasks-pending-bar.json");
        tell(&path, &["Review"]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), signal(&["Review"]));
        tell(&path, &[]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), signal(&[]));
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["tasks-pending-bar.json"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The icon watches that file, the dot and the tooltip coming from
    /// what it says.
    #[test]
    fn the_icon_watches_what_the_window_tells_it() {
        let widget = &files()[1].1;
        for expected in [
            format!(r#"Quickshell.env("XDG_RUNTIME_DIR") + "/{SIGNAL}""#),
            "watchChanges: true".to_owned(),
            "told.dot === true".to_owned(),
            "told.tooltip".to_owned(),
            "visible: root.dot".to_owned(),
            "tooltipText: root.tip".to_owned(),
        ] {
            assert!(widget.contains(&expected), "{expected}");
        }
    }

    /// Where Omarchy keeps its user plugins and its shell settings.
    #[test]
    fn writes_where_omarchy_looks() {
        let home = Path::new("/home/ana");
        assert_eq!(
            plugin_dir(home),
            Path::new("/home/ana/.config/omarchy/plugins/io.github.brunoomariano.tasks-pending")
        );
        assert_eq!(
            shell_config(home),
            Path::new("/home/ana/.config/omarchy/shell.json")
        );
    }

    /// Writing creates the folder and the files; writing the same again
    /// touches nothing; a file that differs (an older version, an edit) is
    /// written over.
    #[test]
    fn writes_the_plugin_only_where_it_differs() {
        let dir = scratch("write");
        assert!(write(&dir).unwrap());
        for (name, text) in files() {
            assert_eq!(fs::read_to_string(dir.join(name)).unwrap(), text, "{name}");
        }
        assert!(!write(&dir).unwrap(), "unchanged");
        fs::write(dir.join("BarWidget.qml"), "edited").unwrap();
        assert!(write(&dir).unwrap());
        assert_eq!(
            fs::read_to_string(dir.join("BarWidget.qml")).unwrap(),
            files()[1].1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The icon is on the bar when a section of the bar's layout holds it;
    /// a layout without it, or settings that cannot be read, say no.
    #[test]
    fn knows_when_the_bar_shows_the_icon() {
        let shown = r#"{"bar": {"layout": {
            "left": [{"id": "omarchy.workspaces"}],
            "right": [{"id": "omarchy.clock"}, {"id": "io.github.brunoomariano.tasks-pending"}]
        }}}"#;
        assert!(in_bar(shown));
        let centre =
            r#"{"bar": {"layout": {"center": [{"id": "io.github.brunoomariano.tasks-pending"}]}}}"#;
        assert!(in_bar(centre));
        let bare = r#"{"bar": {"layout": {"left": ["io.github.brunoomariano.tasks-pending"]}}}"#;
        assert!(in_bar(bare), "Omarchy takes a bare id too");
        assert!(!in_bar(
            r#"{"bar": {"layout": {"right": [{"id": "omarchy.clock"}]}}}"#
        ));
        assert!(!in_bar(
            r#"{"plugins": ["io.github.brunoomariano.tasks-pending"]}"#
        ));
        assert!(!in_bar("not json"));
    }

    /// Omarchy's shell is here when `omarchy-shell` is an executable on
    /// the PATH.
    #[test]
    fn finds_omarchys_shell_on_the_path() {
        let dir = scratch("path");
        fs::create_dir_all(&dir).unwrap();
        let path = std::env::join_paths([Path::new("/nowhere"), &dir]).unwrap();
        assert!(!available(Some(&path)));
        let shell = dir.join("omarchy-shell");
        fs::write(&shell, "#!/bin/sh\n").unwrap();
        assert!(!available(Some(&path)), "not executable");
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(available(Some(&path)));
        assert!(!available(None));
        let _ = fs::remove_dir_all(&dir);
    }

    /// On: the shell looks for plugins again, which it answers before it
    /// has looked, so the plugin is waited for in its list (as Omarchy's
    /// own `plugin add` does) before it is enabled and takes its place on
    /// the bar; off: it is disabled, its folder kept.
    #[test]
    fn turns_the_icon_on_and_off_with_omarchys_commands() {
        assert_eq!(
            steps(true),
            [
                Step::Run(vec!["omarchy-shell", "shell", "rescanPlugins"]),
                Step::AwaitListed,
                Step::Run(vec!["omarchy", "plugin", "enable", PLUGIN_ID]),
            ]
        );
        assert_eq!(
            steps(false),
            [Step::Run(vec!["omarchy", "plugin", "disable", PLUGIN_ID])]
        );
        assert_eq!(LIST, ["omarchy", "plugin", "list", "--json"]);
    }

    /// Omarchy's plugin list holds the plugin once the shell has found it;
    /// a list without it, or one that cannot be read, does not.
    #[test]
    fn knows_when_omarchy_has_found_the_plugin() {
        assert!(listed(
            r#"[{"id": "omarchy.clock", "enabled": true},
                {"id": "io.github.brunoomariano.tasks-pending", "enabled": false}]"#
        ));
        assert!(!listed(r#"[{"id": "omarchy.clock", "enabled": true}]"#));
        assert!(!listed(
            r#"{"id": "io.github.brunoomariano.tasks-pending"}"#
        ));
        assert!(!listed("omarchy-shell is not responding"));
    }

    /// Why Omarchy refused: the last line a command wrote to its error
    /// output, trimmed; none when it wrote nothing but blank lines.
    #[test]
    fn tells_why_omarchy_refused() {
        assert_eq!(
            refusal("warming up\nomarchy-plugin-enable: plugin 'x' is not known  \n\n"),
            Some("omarchy-plugin-enable: plugin 'x' is not known")
        );
        assert_eq!(refusal(" \n\n"), None);
        assert_eq!(refusal(""), None);
    }

    /// Where Omarchy is installed, its own validator takes the plugin as
    /// written.
    #[test]
    fn omarchys_validator_accepts_the_plugin() {
        let Ok(validator) = which("omarchy-plugin-validate") else {
            return;
        };
        let dir = scratch("validate");
        write(&dir).unwrap();
        let checked = std::process::Command::new(validator)
            .arg(&dir)
            .output()
            .unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
    }

    fn which(name: &str) -> Result<PathBuf, ()> {
        let path = std::env::var_os("PATH").ok_or(())?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
            .ok_or(())
    }
}
