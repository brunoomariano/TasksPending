//! The window as a drop-down, from Omarchy's bar or a key: a click
//! summons it, floating over whatever is on screen, in a special
//! workspace of its own, and the next click sends it away with the page as
//! it was. Hyprland does the placing; this asks it, over its socket, where
//! the window is and what to do with it, in the Lua that Hyprland 0.55 and
//! later dispatch (Omarchy 4).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// The special workspace the drop-down lives in.
pub const WORKSPACE: &str = "special:tasks-pending";

/// The same, as `toggle_special` names it.
const SPECIAL: &str = "tasks-pending";

/// How much of the screen, across and down, the drop-down takes.
const SHARE: f64 = 0.8;

/// How long Hyprland has to answer before the window gives up on it.
const ANSWER_WITHIN: Duration = Duration::from_secs(1);

/// Where the window is, as Hyprland reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Hyprland's handle for the window, `0x…`.
    pub address: String,
    /// The name of the workspace it is on.
    pub workspace: String,
}

impl Placement {
    /// It is the drop-down: it lives in the drop-down's workspace.
    pub fn dropped(&self) -> bool {
        self.workspace == WORKSPACE
    }
}

/// The monitor in front of the user, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    /// The drop-down's workspace is the one shown over it.
    pub shows_dropdown: bool,
}

/// One thing Hyprland is asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// Shows the drop-down's workspace on the monitor in front, or hides
    /// it when it is shown there.
    ToggleSpecial,
    /// Moves the window at this address into the drop-down's workspace,
    /// which shows it.
    Bring(String),
    /// Makes the window float.
    Float(String),
    /// Gives the window this width and height.
    Resize(String, u32, u32),
    /// Centres the window on its monitor.
    Center(String),
    /// Runs this shell command, its window born as the drop-down:
    /// floating, centred, in the drop-down's workspace and shown.
    Spawn(String),
}

impl Dispatch {
    /// The Lua Hyprland runs for it, as `hyprctl dispatch` sends it.
    pub fn lua(&self) -> String {
        let window = |address: &str| lua_string(&format!("address:{address}"));
        match self {
            Dispatch::ToggleSpecial => {
                format!("hl.dsp.workspace.toggle_special({})", lua_string(SPECIAL))
            }
            Dispatch::Bring(address) => format!(
                "hl.dsp.window.move({{ workspace = {}, window = {} }})",
                lua_string(WORKSPACE),
                window(address)
            ),
            Dispatch::Float(address) => format!(
                "hl.dsp.window.float({{ action = \"enable\", window = {} }})",
                window(address)
            ),
            Dispatch::Resize(address, width, height) => format!(
                "hl.dsp.window.resize({{ x = {width}, y = {height}, window = {} }})",
                window(address)
            ),
            Dispatch::Center(address) => {
                format!("hl.dsp.window.center({{ window = {} }})", window(address))
            }
            Dispatch::Spawn(command) => format!(
                "hl.dsp.exec_cmd({}, {{ workspace = {}, float = true, center = true, \
                 size = {{ \"(monitor_w*{SHARE})\", \"(monitor_h*{SHARE})\" }} }})",
                lua_string(command),
                lua_string(WORKSPACE)
            ),
        }
    }
}

/// `text` as a Lua string literal.
fn lua_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// What a click on the bar's icon does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// Hyprland cannot place the window: it is brought up the usual way.
    Present,
    /// These steps, in order.
    Run(Vec<Dispatch>),
}

/// What a click does, from where the window is on `screen`: the drop-down
/// shown there goes away; one that is not comes over the screen in front,
/// sized to it (it may have been shown last on another screen) and
/// centred on it; a window opened the usual way becomes the drop-down.
pub fn toggle(window: Option<&Placement>, screen: Option<&Screen>) -> Toggle {
    let (Some(window), Some(screen)) = (window, screen) else {
        return Toggle::Present;
    };
    let address = || window.address.clone();
    let fit = || {
        [
            Dispatch::Resize(
                address(),
                (screen.width * SHARE).round() as u32,
                (screen.height * SHARE).round() as u32,
            ),
            Dispatch::Center(address()),
        ]
    };
    Toggle::Run(if !window.dropped() {
        [Dispatch::Bring(address()), Dispatch::Float(address())]
            .into_iter()
            .chain(fit())
            .collect()
    } else if screen.shows_dropdown {
        vec![Dispatch::ToggleSpecial]
    } else {
        std::iter::once(Dispatch::ToggleSpecial)
            .chain(fit())
            .collect()
    })
}

/// What closing the window does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Close {
    /// The window closes, and the app with it.
    Quit,
    /// The drop-down only goes away, by these steps, with its page kept
    /// for the next click.
    Hide(Vec<Dispatch>),
}

/// What closing does, from where the window is: the drop-down hides, any
/// other window closes.
pub fn close(window: Option<&Placement>, screen: Option<&Screen>) -> Close {
    match window {
        Some(window) if window.dropped() => {
            Close::Hide(if screen.is_some_and(|screen| screen.shows_dropdown) {
                vec![Dispatch::ToggleSpecial]
            } else {
                Vec::new()
            })
        }
        _ => Close::Quit,
    }
}

/// The shell command that starts `exe`'s window.
pub fn spawn_command(exe: &Path) -> String {
    let path = exe.to_string_lossy().replace('\'', r"'\''");
    format!("'{path}' app")
}

/// Hyprland's request socket, from the environment Hyprland gives what it
/// starts. `None` off Hyprland.
pub fn socket(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let known = |name: &str| env(name).filter(|value| !value.is_empty());
    let signature = known("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = known("XDG_RUNTIME_DIR")?;
    Some(
        PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket.sock"),
    )
}

/// The window this process shows, among Hyprland's `j/clients`: the first
/// with its pid and the app's id.
fn placement_in(clients: &str, pid: u32) -> Option<Placement> {
    let clients: Vec<Client> = serde_json::from_str(clients).ok()?;
    clients
        .into_iter()
        .find(|client| client.pid == i64::from(pid) && client.class == crate::APP_ID)
        .map(|client| Placement {
            address: client.address,
            workspace: client.workspace.name,
        })
}

/// The focused monitor among Hyprland's `j/monitors`.
fn screen_in(monitors: &str) -> Option<Screen> {
    let monitors: Vec<Monitor> = serde_json::from_str(monitors).ok()?;
    let monitor = monitors.into_iter().find(|monitor| monitor.focused)?;
    let scale = if monitor.scale > 0.0 {
        monitor.scale
    } else {
        1.0
    };
    let (width, height) = (monitor.width / scale, monitor.height / scale);
    // Hyprland's transforms 1, 3, 5 and 7 turn the monitor a quarter.
    let turned = monitor.transform % 2 == 1;
    Some(Screen {
        width: if turned { height } else { width },
        height: if turned { width } else { height },
        shows_dropdown: monitor.special_workspace.name == WORKSPACE,
    })
}

/// Hyprland, through its request socket.
#[derive(Clone, Debug)]
pub struct Hyprland {
    socket: PathBuf,
}

impl Hyprland {
    pub fn new(socket: PathBuf) -> Hyprland {
        Hyprland { socket }
    }

    /// One request and its answer: Hyprland answers and hangs up.
    fn request(&self, message: &str) -> std::io::Result<String> {
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(ANSWER_WITHIN))?;
        stream.set_write_timeout(Some(ANSWER_WITHIN))?;
        stream.write_all(message.as_bytes())?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer)?;
        Ok(answer)
    }

    /// Where the window of the process `pid` is.
    pub fn placement(&self, pid: u32) -> std::io::Result<Option<Placement>> {
        Ok(placement_in(&self.request("j/clients")?, pid))
    }

    /// The monitor in front of the user.
    pub fn screen(&self) -> std::io::Result<Option<Screen>> {
        Ok(screen_in(&self.request("j/monitors")?))
    }

    /// Runs `steps` in order, stopping at the first Hyprland refuses.
    pub fn run(&self, steps: &[Dispatch]) -> std::io::Result<()> {
        for step in steps {
            let answer = self.request(&format!("dispatch {}", step.lua()))?;
            if answer.trim() != "ok" {
                return Err(std::io::Error::other(format!(
                    "Hyprland refused {}: {}",
                    step.lua(),
                    answer.trim()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Client {
    address: String,
    pid: i64,
    class: String,
    workspace: Named,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Monitor {
    focused: bool,
    width: f64,
    height: f64,
    scale: f64,
    transform: i64,
    #[serde(rename = "specialWorkspace")]
    special_workspace: Named,
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    fn dropped(address: &str) -> Placement {
        Placement {
            address: address.to_owned(),
            workspace: WORKSPACE.to_owned(),
        }
    }

    fn tiled(address: &str) -> Placement {
        Placement {
            address: address.to_owned(),
            workspace: "3".to_owned(),
        }
    }

    const SCREEN: Screen = Screen {
        width: 1920.0,
        height: 1080.0,
        shows_dropdown: false,
    };

    /// A click hides the drop-down shown in front; shows one that is not,
    /// sized to four fifths of the screen in front and centred on it (it
    /// may have been shown last on another one, larger or smaller); and
    /// turns a window opened the usual way into the drop-down the same
    /// way.
    #[test]
    fn a_click_shows_or_hides_the_drop_down() {
        let shown = Screen {
            shows_dropdown: true,
            ..SCREEN
        };
        assert_eq!(
            toggle(Some(&dropped("0xa1")), Some(&shown)),
            Toggle::Run(vec![Dispatch::ToggleSpecial])
        );
        assert_eq!(
            toggle(Some(&dropped("0xa1")), Some(&SCREEN)),
            Toggle::Run(vec![
                Dispatch::ToggleSpecial,
                Dispatch::Resize("0xa1".to_owned(), 1536, 864),
                Dispatch::Center("0xa1".to_owned())
            ])
        );
        let address = || "0xb2".to_owned();
        let bring = Toggle::Run(vec![
            Dispatch::Bring(address()),
            Dispatch::Float(address()),
            Dispatch::Resize(address(), 1536, 864),
            Dispatch::Center(address()),
        ]);
        assert_eq!(toggle(Some(&tiled("0xb2")), Some(&SCREEN)), bring);
        assert_eq!(
            toggle(Some(&tiled("0xb2")), Some(&shown)),
            bring,
            "whatever is shown"
        );
    }

    /// Without a window Hyprland knows of, or without a screen, the window
    /// is brought up the usual way.
    #[test]
    fn a_click_hyprland_cannot_place_presents_the_window() {
        assert_eq!(toggle(None, Some(&SCREEN)), Toggle::Present);
        assert_eq!(toggle(Some(&tiled("0xb2")), None), Toggle::Present);
        assert_eq!(toggle(Some(&dropped("0xa1")), None), Toggle::Present);
    }

    /// Closing the drop-down hides it (it may be hidden already, closed
    /// from a key with nothing shown), keeping its page; any other window
    /// closes, and so does one off Hyprland.
    #[test]
    fn closing_the_drop_down_hides_it() {
        let shown = Screen {
            shows_dropdown: true,
            ..SCREEN
        };
        assert_eq!(
            close(Some(&dropped("0xa1")), Some(&shown)),
            Close::Hide(vec![Dispatch::ToggleSpecial])
        );
        assert_eq!(
            close(Some(&dropped("0xa1")), Some(&SCREEN)),
            Close::Hide(Vec::new())
        );
        assert_eq!(close(Some(&dropped("0xa1")), None), Close::Hide(Vec::new()));
        assert_eq!(close(Some(&tiled("0xb2")), Some(&shown)), Close::Quit);
        assert_eq!(close(None, Some(&shown)), Close::Quit);
    }

    /// Each step is the Lua Hyprland dispatches, naming the window by its
    /// address; strings are quoted for Lua, whatever they hold.
    #[test]
    fn writes_each_step_as_hyprland_lua() {
        let window = || "0xa1".to_owned();
        assert_eq!(
            Dispatch::ToggleSpecial.lua(),
            r#"hl.dsp.workspace.toggle_special("tasks-pending")"#
        );
        assert_eq!(
            Dispatch::Bring(window()).lua(),
            r#"hl.dsp.window.move({ workspace = "special:tasks-pending", window = "address:0xa1" })"#
        );
        assert_eq!(
            Dispatch::Float(window()).lua(),
            r#"hl.dsp.window.float({ action = "enable", window = "address:0xa1" })"#
        );
        assert_eq!(
            Dispatch::Resize(window(), 1536, 864).lua(),
            r#"hl.dsp.window.resize({ x = 1536, y = 864, window = "address:0xa1" })"#
        );
        assert_eq!(
            Dispatch::Center(window()).lua(),
            r#"hl.dsp.window.center({ window = "address:0xa1" })"#
        );
        assert_eq!(
            Dispatch::Spawn("'/opt/tasks pending' app".to_owned()).lua(),
            r#"hl.dsp.exec_cmd("'/opt/tasks pending' app", { workspace = "special:tasks-pending", float = true, center = true, size = { "(monitor_w*0.8)", "(monitor_h*0.8)" } })"#
        );
        assert_eq!(
            Dispatch::Spawn("say \"hi\\\"\n".to_owned()).lua(),
            r#"hl.dsp.exec_cmd("say \"hi\\\"\n", { workspace = "special:tasks-pending", float = true, center = true, size = { "(monitor_w*0.8)", "(monitor_h*0.8)" } })"#
        );
    }

    /// The window is started by its path, quoted for the shell Hyprland
    /// runs it with.
    #[test]
    fn starts_the_window_by_its_quoted_path() {
        assert_eq!(
            spawn_command(Path::new("/usr/bin/tasks-pending")),
            "'/usr/bin/tasks-pending' app"
        );
        assert_eq!(
            spawn_command(Path::new("/home/o'neil/bin/tasks-pending")),
            r"'/home/o'\''neil/bin/tasks-pending' app"
        );
    }

    /// The socket is where Hyprland says, under the runtime directory;
    /// off Hyprland there is none.
    #[test]
    fn finds_hyprland_by_its_signature() {
        let env = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_owned()),
            "HYPRLAND_INSTANCE_SIGNATURE" => Some("abc_123".to_owned()),
            _ => None,
        };
        assert_eq!(
            socket(&env),
            Some(PathBuf::from("/run/user/1000/hypr/abc_123/.socket.sock"))
        );
        let off = |name: &str| (name == "XDG_RUNTIME_DIR").then(|| "/run/user/1000".to_owned());
        assert_eq!(socket(&off), None);
        let blank = |name: &str| {
            Some(
                if name == "XDG_RUNTIME_DIR" {
                    "/run"
                } else {
                    ""
                }
                .to_owned(),
            )
        };
        assert_eq!(socket(&blank), None);
    }

    /// The window is this process's, with the app's id; another app's
    /// window of the same process (none today) or another process's is
    /// not it.
    #[test]
    fn finds_its_own_window_among_the_clients() {
        let clients = r#"[
            {"address": "0x1", "pid": 7, "class": "io.github.brunoomariano.TasksPending",
             "workspace": {"id": 3, "name": "3"}, "floating": false},
            {"address": "0x2", "pid": 42, "class": "Alacritty",
             "workspace": {"id": 1, "name": "1"}},
            {"address": "0x3", "pid": 42, "class": "io.github.brunoomariano.TasksPending",
             "workspace": {"id": -98, "name": "special:tasks-pending"}}
        ]"#;
        assert_eq!(
            placement_in(clients, 42),
            Some(Placement {
                address: "0x3".to_owned(),
                workspace: WORKSPACE.to_owned()
            })
        );
        assert_eq!(placement_in(clients, 9), None);
        assert_eq!(placement_in("not json", 42), None);
    }

    /// The screen is the focused monitor, in logical pixels (its size
    /// over its scale, turned when the monitor is), and says whether the
    /// drop-down is shown over it.
    #[test]
    fn measures_the_focused_monitor() {
        let monitors = r#"[
            {"name": "DP-1", "focused": false, "width": 3840, "height": 2160, "scale": 2.0,
             "transform": 0, "specialWorkspace": {"id": 0, "name": ""}},
            {"name": "eDP-1", "focused": true, "width": 2880, "height": 1800, "scale": 1.5,
             "transform": 0, "specialWorkspace": {"id": -98, "name": "special:tasks-pending"}}
        ]"#;
        assert_eq!(
            screen_in(monitors),
            Some(Screen {
                width: 1920.0,
                height: 1200.0,
                shows_dropdown: true
            })
        );
        let turned = r#"[{"focused": true, "width": 1920, "height": 1080, "scale": 1.0,
             "transform": 1, "specialWorkspace": {"id": -97, "name": "special:scratchpad"}}]"#;
        assert_eq!(
            screen_in(turned),
            Some(Screen {
                width: 1080.0,
                height: 1920.0,
                shows_dropdown: false
            })
        );
        assert_eq!(
            screen_in(
                r#"[{"focused": false, "width": 1, "height": 1, "scale": 1.0, "transform": 0, "specialWorkspace": {"id": 0, "name": ""}}]"#
            ),
            None
        );
        assert_eq!(screen_in("[]"), None);
    }

    /// A fake Hyprland on a socket of its own: it answers each request in
    /// turn and keeps what it was asked.
    fn fake_hyprland(
        answers: Vec<&'static str>,
    ) -> (Hyprland, std::thread::JoinHandle<Vec<String>>) {
        let dir = std::env::temp_dir().join(format!(
            "tasks-pending-hypr-{}-{}",
            std::process::id(),
            answers.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".socket.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let asked = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let read = stream.read(&mut request).unwrap();
                asked.push(String::from_utf8_lossy(&request[..read]).into_owned());
                stream.write_all(answer.as_bytes()).unwrap();
            }
            let _ = std::fs::remove_dir_all(&dir);
            asked
        });
        (Hyprland::new(path), asked)
    }

    /// Hyprland is asked over its socket: the clients and the monitors as
    /// JSON, then each step as its own dispatch, stopping at a refusal.
    #[test]
    fn asks_hyprland_over_its_socket() {
        let (hyprland, asked) = fake_hyprland(vec![
            r#"[{"address": "0x3", "pid": 42, "class": "io.github.brunoomariano.TasksPending", "workspace": {"name": "7"}}]"#,
            r#"[{"focused": true, "width": 1000, "height": 500, "scale": 1.0, "transform": 0, "specialWorkspace": {"name": ""}}]"#,
            "ok",
            "window not found",
        ]);
        assert_eq!(
            hyprland.placement(42).unwrap(),
            Some(Placement {
                address: "0x3".to_owned(),
                workspace: "7".to_owned()
            })
        );
        assert_eq!(
            hyprland.screen().unwrap(),
            Some(Screen {
                width: 1000.0,
                height: 500.0,
                shows_dropdown: false
            })
        );
        let refused = hyprland
            .run(&[
                Dispatch::ToggleSpecial,
                Dispatch::Center("0x3".to_owned()),
                Dispatch::Float("0x3".to_owned()),
            ])
            .unwrap_err();
        assert!(
            refused.to_string().contains("window not found"),
            "{refused}"
        );
        assert_eq!(
            asked.join().unwrap(),
            [
                "j/clients".to_owned(),
                "j/monitors".to_owned(),
                format!("dispatch {}", Dispatch::ToggleSpecial.lua()),
                format!("dispatch {}", Dispatch::Center("0x3".to_owned()).lua()),
            ],
            "nothing after the refusal"
        );
    }
}
