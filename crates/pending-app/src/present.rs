//! What the window shows, worked out from the snapshot and the view, with
//! no widgets involved: the Activity area and its stages, the groups and
//! stacks of the Board, the control strip, and the words and icon names
//! the widgets put on screen.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use pending_core::api::REFRESH_COOLDOWN;
use pending_core::{
    DashboardSnapshot, Icon, PendingCard, SourceHealth, SourceStatus, WeatherCondition, is_http_url,
};

use crate::alert;
use crate::backend::{Command, Refusal};
use crate::session::{Session, Todo};
use crate::snooze::{self, until_text};
use crate::state::ViewState;
use crate::theme::OmarchyTheme;
use crate::view::{AUTO_REFRESH_CHOICES, Stage, Theme, View, column_key, group_key, hide_key};

/// Cards a stack shows before "show more".
pub const COLUMN_LIMIT: usize = 5;

/// How a time is written on the page: `28 Sep 10:30`.
pub const TIME_FORMAT: &str = "%-d %b %H:%M";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardView {
    pub id: String,
    pub title: String,
    /// An http(s) link for the title, when the card has one.
    pub url: Option<String>,
    pub body: String,
    /// The configured name of the source the card comes from.
    pub source: String,
    /// Where the card stands in the Activity area, once picked.
    pub stage: Option<Stage>,
    pub changed: bool,
    /// "Alert at 15:45", while an alert is set on the card.
    pub alert: Option<String>,
    /// A todo written down in the window, on no source.
    pub local: bool,
}

impl CardView {
    /// Picked: in the Activity area.
    pub fn picked(&self) -> bool {
        self.stage.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackView {
    pub key: String,
    pub name: String,
    pub count: usize,
    /// A card in the stack changed since the user last looked.
    pub changed: bool,
    pub collapsed: bool,
    /// Showing every card, not just the first [`COLUMN_LIMIT`].
    pub open: bool,
    pub cards: Vec<CardView>,
    /// Cards not shown because the stack is not open.
    pub more: usize,
    /// The button that shows the rest or fewer, when there is one.
    pub more_label: Option<String>,
    /// What to say instead of cards when there are none.
    pub empty_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupView {
    pub key: String,
    /// The badge after the name: the board.
    pub board: String,
    pub source: String,
    pub icon: Option<Icon>,
    /// Showing the empty stacks too.
    pub expanded: bool,
    /// Stacks without cards, shown or not.
    pub empty: usize,
    /// The button that shows or hides them, when there are some.
    pub empty_toggle: Option<EmptyToggle>,
    pub stacks: Vec<StackView>,
}

/// The button at the end of a group's header for its empty stacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyToggle {
    pub icon: &'static str,
    pub label: String,
    pub tooltip: &'static str,
}

impl EmptyToggle {
    fn new(empty: usize, expanded: bool) -> Option<Self> {
        (empty > 0).then(|| {
            if expanded {
                Self {
                    icon: "view-conceal-symbolic",
                    label: "hide empty".to_owned(),
                    tooltip: "Hide empty stacks",
                }
            } else {
                Self {
                    icon: "view-reveal-symbolic",
                    label: format!("{empty} empty"),
                    tooltip: "Show empty stacks",
                }
            }
        })
    }
}

/// What a stack's "more" button says: the rest to show, or fewer once it
/// shows them all; none when every card fits.
fn more_label(count: usize, more: usize, open: bool) -> Option<String> {
    if more > 0 {
        Some(format!("Show {more} more"))
    } else if open && count > COLUMN_LIMIT {
        Some("Show less".to_owned())
    } else {
        None
    }
}

/// A picked card in the Activity area, with where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityItem {
    pub card: CardView,
    pub source: String,
    pub icon: Option<Icon>,
}

/// One column of the Activity area: a stage and the cards at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageView {
    pub stage: Stage,
    pub name: &'static str,
    pub cards: Vec<ActivityItem>,
    /// What to say when there are none.
    pub empty_note: &'static str,
    /// It has the button that writes down a todo, which lands here:
    /// Picked alone.
    pub adds_todo: bool,
}

/// The Activity area: the picked cards by stage, in picking order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityView {
    pub stages: Vec<StageView>,
    pub count: usize,
    /// What to say under the stages while nothing is picked.
    pub hint: Option<&'static str>,
}

/// The hint under an empty Activity area.
pub const ACTIVITY_HINT: &str = "Pick cards on the Board to bring them here.";

/// What a stage says when no card is at it.
fn stage_empty_note(stage: Stage) -> &'static str {
    match stage {
        Stage::Picked => "Nothing picked",
        Stage::InProgress => "Nothing under way",
        Stage::Done => "Nothing to mark",
    }
}

/// The two parts of the page under the hero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// The picked cards by stage: the main screen.
    #[default]
    Activity,
    /// Every source's stacks, where cards are picked.
    Board,
}

/// A tab's button: its name and, for the Activity, how many cards it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabView {
    pub tab: Tab,
    pub label: &'static str,
    pub count: Option<usize>,
}

/// The tab buttons for `page`.
pub fn tabs(page: Option<&Page>) -> [TabView; 2] {
    [
        TabView {
            tab: Tab::Activity,
            label: "Activity",
            count: page
                .map(|page| page.activity.count)
                .filter(|count| *count > 0),
        },
        TabView {
            tab: Tab::Board,
            label: "Board",
            count: None,
        },
    ]
}

/// Where a card is drawn, which decides what its menu offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In its stack on the Board.
    Board,
    /// In the Activity area.
    Activity,
}

/// What the user can do to a card from its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    /// Pick the card: it goes to the Activity area.
    Pick,
    /// Unpick it: it leaves the Activity area.
    Unpick,
    /// Move it to this stage of the Activity area.
    Stage(Stage),
    Snooze(snooze::Choice),
    /// Be reminded of the card in a while.
    Alert(alert::Choice),
    CancelAlert,
    /// Rewrite a todo.
    Edit,
    /// Forget a todo.
    Delete,
}

/// One entry of a card's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Action {
        label: String,
        action: CardAction,
    },
    Submenu {
        label: String,
        items: Vec<(String, CardAction)>,
    },
}

impl MenuItem {
    fn action(label: &str, action: CardAction) -> Self {
        Self::Action {
            label: label.to_owned(),
            action,
        }
    }
}

/// The menu behind a card's three dots at `place`: sending the card to the
/// Activity area or taking it back; in the Activity area, moving it on to
/// the next stage or back; on the Board, snoozing it; anywhere, being
/// reminded of it in a while, and calling that off while it is set. A
/// todo lives in the Activity area, so it is neither sent there nor taken
/// out, and is not snoozed (it is the window's own), but it can be
/// rewritten or forgotten.
pub fn card_menu(card: &CardView, place: Place) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if let Some(stage) = card.stage.filter(|_| place == Place::Activity) {
        for other in Stage::ALL {
            if other != stage {
                let label = if other > stage {
                    format!("Move to {}", stage_short_name(other))
                } else {
                    format!("Back to {}", stage_short_name(other))
                };
                items.push(MenuItem::Action {
                    label,
                    action: CardAction::Stage(other),
                });
            }
        }
    }
    if !card.local {
        items.push(if card.picked() {
            MenuItem::action("Remove from Activity", CardAction::Unpick)
        } else {
            MenuItem::action("Send to Activity", CardAction::Pick)
        });
    }
    if place == Place::Board && !card.local {
        items.push(MenuItem::Submenu {
            label: "Snooze".to_owned(),
            items: snooze::CHOICES
                .iter()
                .map(|(choice, label)| ((*label).to_owned(), CardAction::Snooze(*choice)))
                .collect(),
        });
    }
    items.push(MenuItem::Submenu {
        label: "Alert me about".to_owned(),
        items: alert::CHOICES
            .iter()
            .map(|(choice, label)| ((*label).to_owned(), CardAction::Alert(*choice)))
            .collect(),
    });
    if card.alert.is_some() {
        items.push(MenuItem::action(
            "Cancel the alert",
            CardAction::CancelAlert,
        ));
    }
    if card.local {
        items.push(MenuItem::action("Edit", CardAction::Edit));
        items.push(MenuItem::action("Delete", CardAction::Delete));
    }
    items
}

/// What a todo says it comes from, where a card names its source.
pub const TODO_GROUP: &str = "Todo";
/// What the todo dialog says about keeping them.
pub const TODO_NOTE: &str = "Kept only while TasksPending runs: quitting it forgets every todo.";

/// The todo dialog's title and button, for a new todo or one being
/// rewritten.
pub fn todo_dialog_words(editing: bool) -> (&'static str, &'static str) {
    if editing {
        ("Edit todo", "Save")
    } else {
        ("New todo", "Add")
    }
}

/// A stage's name in a menu entry, where "Done / to mark" is too long.
fn stage_short_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Done => "Done",
        other => other.name(),
    }
}

/// One source in the Sources dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceView {
    pub name: String,
    pub icon: Option<Icon>,
    pub boards: String,
    pub status_text: &'static str,
    /// The symbolic icon for the status.
    pub status_icon: &'static str,
    /// The CSS class that colours the status.
    pub status_class: &'static str,
    pub time: String,
    pub message: Option<String>,
}

/// One snoozed card in the Snoozed dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnoozedView {
    pub card: CardView,
    pub source: String,
    pub icon: Option<Icon>,
    pub until: String,
}

impl SnoozedView {
    /// "github · until Thu 8 Oct 08:00": the line under the card.
    pub fn footer(&self) -> String {
        format!("{} · {}", self.source, self.until)
    }
}

/// The whole page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub activity: ActivityView,
    pub groups: Vec<GroupView>,
    pub sources: Vec<SourceView>,
    pub snoozed: Vec<SnoozedView>,
    /// A source is degraded or failed, or the config was rejected.
    pub attention: bool,
    /// "Updated 28 Sep 10:30".
    pub updated: String,
    pub version: String,
    /// "· v0.6.0", when the daemon says which version it runs.
    pub version_line: Option<String>,
    /// "· 2 snoozed", when any card is.
    pub snoozed_line: Option<String>,
    pub config_error: Option<String>,
}

/// What the newer-version line explains when pointed at. Not only the
/// install script's command: a copy built or installed by hand is updated
/// the same way it was installed.
pub const UPDATE_TOOLTIP: &str = "A newer version is published. Update it the way you \
     installed it: tasks-pending upgrade, after the install script.";

/// "· v0.7.0 available": a newer published `version`.
pub fn update_line(version: &str) -> String {
    format!("· v{version} available")
}

/// A pill in Settings: what it says, what it sets, and whether it is the
/// one in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice<T> {
    pub label: String,
    pub value: T,
    pub active: bool,
}

/// Settings' board filter: every board, or one of them.
pub fn board_choices(snapshot: &DashboardSnapshot, view: &View) -> Vec<Choice<Option<String>>> {
    std::iter::once(("All".to_owned(), None))
        .chain(
            snapshot
                .boards
                .iter()
                .map(|board| (board.name.clone(), Some(board.name.clone()))),
        )
        .map(|(label, value)| Choice {
            active: view.board == value,
            label,
            value,
        })
        .collect()
}

/// Settings' auto-refresh intervals, in minutes.
pub fn interval_choices(view: &View) -> Vec<Choice<u32>> {
    AUTO_REFRESH_CHOICES
        .into_iter()
        .map(|minutes| Choice {
            label: if minutes == 0 {
                "Off".to_owned()
            } else {
                format!("{minutes} min")
            },
            value: minutes,
            active: view.auto_refresh_minutes == minutes,
        })
        .collect()
}

/// Settings' themes: the window's own colours, or Omarchy's.
pub fn theme_choices(view: &View) -> Vec<Choice<Theme>> {
    [
        (Theme::TasksPending, "TasksPending"),
        (Theme::Omarchy, "Omarchy"),
    ]
    .into_iter()
    .map(|(theme, label)| Choice {
        label: label.to_owned(),
        value: theme,
        active: view.theme == theme,
    })
    .collect()
}

/// The line under the themes while the window follows Omarchy: the theme
/// it follows, or why there is none to follow (`omarchy` is the last
/// reading, `None` before the first).
pub fn theme_note(view: &View, omarchy: Option<&Result<OmarchyTheme, String>>) -> Option<String> {
    if view.theme != Theme::Omarchy {
        return None;
    }
    Some(match omarchy? {
        Ok(OmarchyTheme {
            name: Some(name), ..
        }) => format!("Follows Omarchy's theme, {name}, and changes with it."),
        Ok(_) => "Follows Omarchy's theme and changes with it.".to_owned(),
        Err(why) => format!("Nothing to follow ({why}): the window keeps its own colours."),
    })
}

/// Settings' Omarchy bar: TasksPending's icon on it, or not.
pub fn bar_choices(on: bool) -> Vec<Choice<bool>> {
    [(false, "Off"), (true, "On")]
        .into_iter()
        .map(|(value, label)| Choice {
            label: label.to_owned(),
            value,
            active: value == on,
        })
        .collect()
}

/// Where the icon on Omarchy's bar stands, as Settings says it under its
/// choices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarState {
    Off,
    On,
    /// Omarchy is putting it on (`true`) or taking it off.
    Changing(bool),
    /// Omarchy refused, saying why.
    Failed(String),
}

/// How long Settings trusts the last change it made to the icon over
/// Omarchy's settings, which the shell writes a moment after it answers.
pub const BAR_SETTLE: Duration = Duration::from_secs(10);

/// What Settings shows for the icon on Omarchy's bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarView {
    /// Whether it is on.
    pub on: bool,
    /// The line under the choices.
    pub state: BarState,
    /// Whether Settings still holds on to its last change.
    pub keep_asked: bool,
}

/// What Settings shows for the icon: what Omarchy's settings say
/// (`placed`), unless Settings changed it (`asked`, and how long ago)
/// within [`BAR_SETTLE`] and they do not say so yet; and a change under
/// way or refused (`change`) in place of On or Off.
pub fn bar_view(
    placed: bool,
    asked: Option<(bool, Duration)>,
    change: Option<&BarState>,
) -> BarView {
    let pending = asked.filter(|&(on, age)| on != placed && age < BAR_SETTLE);
    let on = pending.map_or(placed, |(on, _)| on);
    let state = change
        .cloned()
        .unwrap_or(if on { BarState::On } else { BarState::Off });
    BarView {
        on,
        state,
        keep_asked: pending.is_some(),
    }
}

/// The line under the Omarchy bar's choices.
pub fn bar_note(state: &BarState) -> String {
    match state {
        BarState::Off => {
            "Puts an icon on Omarchy's bar that drops this window down over the screen.".to_owned()
        }
        BarState::On => "A click on the icon drops this window down over the screen and the next \
             sends it away; closing it then only hides it (Ctrl+Q quits). \
             tasks-pending toggle does the same from a key."
            .to_owned(),
        BarState::Changing(true) => "Putting the icon on the bar…".to_owned(),
        BarState::Changing(false) => "Taking the icon off the bar…".to_owned(),
        BarState::Failed(why) => format!("Omarchy did not take it: {why}"),
    }
}

/// A show/hide switch in Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchView {
    pub label: String,
    /// Its entry in the view's hidden set (see [`hide_key`]).
    pub key: String,
    pub on: bool,
    /// A board or group above it is hidden: the switch is greyed out and
    /// keeps its own setting for when that comes back.
    pub inherited: bool,
}

/// A board's switch, then its groups'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardSwitches {
    pub board: SwitchView,
    pub groups: Vec<GroupSwitches>,
}

/// A group's switch, with its logo, then its stacks'.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSwitches {
    pub group: SwitchView,
    pub icon: Option<Icon>,
    pub stacks: Vec<SwitchView>,
}

/// Every board, group and stack that Settings can show or hide.
pub fn switches(snapshot: &DashboardSnapshot, view: &View) -> Vec<BoardSwitches> {
    let switch = |label: &str, key: String, inherited: bool| SwitchView {
        label: label.to_owned(),
        on: !view.hidden.contains(&key),
        key,
        inherited,
    };
    snapshot
        .boards
        .iter()
        .map(|board| {
            let board_switch = switch(&board.name, hide_key::board(&board.name), false);
            let board_off = !board_switch.on;
            let groups = board
                .groups
                .iter()
                .map(|group| {
                    let key = hide_key::group(&board.name, &group.source);
                    let group_switch = switch(&group.source, key, board_off);
                    let off = board_off || !group_switch.on;
                    GroupSwitches {
                        stacks: group
                            .columns
                            .iter()
                            .map(|column| {
                                let key =
                                    hide_key::column(&board.name, &group.source, &column.name);
                                switch(&column.name, key, off)
                            })
                            .collect(),
                        group: group_switch,
                        icon: group.icon.clone(),
                    }
                })
                .collect();
            BoardSwitches {
                board: board_switch,
                groups,
            }
        })
        .collect()
}

/// What stands above the board, or instead of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Nothing has answered yet.
    Loading,
    /// The daemon did not answer and there is no dashboard to show: why.
    Unavailable(String),
    /// The daemon stopped answering; the dashboard is from earlier.
    Stale(String),
}

/// What the page under the hero draws: the tab in front, with what it
/// shows. The window rebuilds the page only when this changes, so a fetch
/// that changed nothing on it (only the sources' fetch times, say) keeps
/// the focus and the reading position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardView {
    pub notice: Option<Notice>,
    pub tab: Tab,
    /// The Activity area, drawn on its tab.
    pub activity: Option<ActivityView>,
    /// The groups, drawn on the Board tab.
    pub groups: Vec<GroupView>,
}

/// The page for `state` with `tab` in front, whose snapshot `page` was
/// worked out from.
pub fn board_view(state: &ViewState, page: Option<&Page>, tab: Tab) -> BoardView {
    let notice = match state {
        ViewState::Loading => Some(Notice::Loading),
        ViewState::Unavailable { error } => Some(Notice::Unavailable(error.clone())),
        ViewState::Stale {
            error, fetched_at, ..
        } => Some(Notice::Stale(stale_notice(error, *fetched_at))),
        ViewState::Ready { .. } => None,
    };
    BoardView {
        notice,
        tab,
        activity: page.map(|page| page.activity.clone()),
        groups: page.map(|page| page.groups.clone()).unwrap_or_default(),
    }
}

/// How the window shows the answer to a refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refreshed {
    /// The Refresh button stays off this long, saying this.
    pub hold: Option<(Duration, String)>,
    /// A note under the controls.
    pub note: Option<String>,
}

/// The answer to a refresh, `manual` when the user asked for it. One the
/// daemon took holds the button until it would take another. A refusal
/// shows only for a refresh the user asked for, since the auto-refresh
/// timer tries again at its next turn: a wait holds the button for as long
/// as the daemon said, and a failure is a note, too long for a button.
pub fn refreshed(manual: bool, result: &Result<(), Refusal>) -> Refreshed {
    let (hold, note) = match result {
        Ok(()) => (Some((REFRESH_COOLDOWN, REFRESHING.to_owned())), None),
        Err(_) if !manual => (None, None),
        Err(Refusal::Wait(secs)) => (Some((Duration::from_secs(*secs), wait_label(*secs))), None),
        Err(Refusal::Failed(error)) => (None, Some(format!("Could not refresh ({error})"))),
    };
    Refreshed { hold, note }
}

/// The note when a card's link could not be opened.
pub fn link_note(error: &str) -> String {
    format!("Could not open the link ({error})")
}

/// `hold` if it still lasts at `now`: the timer set for one hold must not
/// end a later one.
pub fn lasting(hold: Option<(Instant, String)>, now: Instant) -> Option<(Instant, String)> {
    hold.filter(|(until, _)| now < *until)
}

/// The Refresh button's label while a refresh it asked for runs.
const REFRESHING: &str = "Refreshing…";

/// The Refresh button's label while the daemon asks to wait `secs`.
fn wait_label(secs: u64) -> String {
    format!("Wait {secs}s")
}

/// The texts the Refresh button is sized for, so it keeps one width
/// whatever it says: at rest, refreshing, and waiting as long as a daemon
/// can ask (the cooldown is seconds; two digits leave room for a longer
/// one).
pub fn refresh_widths() -> Vec<String> {
    vec![
        refresh_button(None, Instant::now()).0,
        REFRESHING.to_owned(),
        wait_label(99),
    ]
}

/// The Refresh button's label, and whether it can be pressed, at `now`.
pub fn refresh_button(hold: Option<&(Instant, String)>, now: Instant) -> (String, bool) {
    match hold {
        Some((until, label)) if now < *until => (label.clone(), false),
        _ => ("Refresh".to_owned(), true),
    }
}

/// The window's timers.
#[derive(Clone, Copy, Debug)]
pub enum Timer {
    Poll,
    Weather,
    Version,
    AutoRefresh,
}

/// What a timer asks of the daemon when it fires. While the window is
/// hidden nothing is read or refreshed; the version is asked for anyway.
pub fn on_timer(timer: Timer, hidden: bool) -> Option<Command> {
    match timer {
        Timer::Version => Some(Command::Version),
        _ if hidden => None,
        Timer::Poll => Some(Command::Poll),
        Timer::Weather => Some(Command::Weather),
        Timer::AutoRefresh => Some(Command::Refresh { manual: false }),
    }
}

/// How often every source is refreshed for the setting of `minutes`, or
/// `None` when it is off.
pub fn auto_refresh_every(minutes: u32) -> Option<Duration> {
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
}

/// `at` in local time, as written on the page.
pub fn local_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format(TIME_FORMAT).to_string()
}

/// The notice over a dashboard kept from before the daemon stopped
/// answering.
pub fn stale_notice(error: &str, fetched_at: DateTime<Local>) -> String {
    format!(
        "Dashboard unreachable ({error}); showing data from {}",
        fetched_at.format(TIME_FORMAT)
    )
}

/// The icon name for a weather condition.
pub fn weather_icon_name(condition: WeatherCondition) -> &'static str {
    match condition {
        WeatherCondition::ClearDay => "weather-clear-symbolic",
        WeatherCondition::ClearNight => "weather-clear-night-symbolic",
        WeatherCondition::PartlyCloudyDay => "weather-few-clouds-symbolic",
        WeatherCondition::PartlyCloudyNight => "weather-few-clouds-night-symbolic",
        WeatherCondition::Cloudy | WeatherCondition::Unknown => "weather-overcast-symbolic",
        WeatherCondition::Fog => "weather-fog-symbolic",
        WeatherCondition::Rain => "weather-showers-symbolic",
        WeatherCondition::Sleet => "weather-showers-scattered-symbolic",
        WeatherCondition::Snow => "weather-snow-symbolic",
        WeatherCondition::Thunder => "weather-storm-symbolic",
    }
}

fn status_text(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Ready => "Ready",
        SourceStatus::Refreshing => "Refreshing",
        SourceStatus::Degraded => "Degraded",
        SourceStatus::Failed => "Failed",
    }
}

fn status_icon(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Ready => "object-select-symbolic",
        SourceStatus::Refreshing => "view-refresh-symbolic",
        SourceStatus::Degraded => "dialog-warning-symbolic",
        SourceStatus::Failed => "process-stop-symbolic",
    }
}

fn status_class(status: SourceStatus) -> &'static str {
    match status {
        SourceStatus::Ready => "ready",
        SourceStatus::Refreshing => "refreshing",
        SourceStatus::Degraded => "degraded",
        SourceStatus::Failed => "failed",
    }
}

/// The ids the snapshot flags, the view's stages and the session's
/// alerts, for building card views.
struct Flags<'a> {
    marked: HashSet<&'a str>,
    changed: HashSet<&'a str>,
    view: &'a View,
    session: &'a Session,
}

impl<'a> Flags<'a> {
    fn of(snapshot: &'a DashboardSnapshot, view: &'a View, session: &'a Session) -> Self {
        Self {
            marked: snapshot.marked.iter().map(String::as_str).collect(),
            changed: snapshot.changed.iter().map(String::as_str).collect(),
            view,
            session,
        }
    }

    fn card(&self, card: &PendingCard) -> CardView {
        CardView {
            id: card.id.clone(),
            title: card.title.clone(),
            url: card
                .url
                .as_deref()
                .map(str::trim)
                .filter(|url| is_http_url(url))
                .map(str::to_owned),
            body: card.body.clone(),
            source: card.source.clone(),
            stage: self
                .marked
                .contains(card.id.as_str())
                .then(|| self.view.stage(&card.id)),
            changed: self.changed.contains(card.id.as_str()),
            alert: self.session.alerts.text_for(&card.id),
            local: false,
        }
    }

    fn todo(&self, todo: &Todo) -> CardView {
        CardView {
            id: todo.id.clone(),
            title: todo.title.clone(),
            url: None,
            body: todo.body.clone(),
            source: TODO_GROUP.to_owned(),
            stage: Some(todo.stage),
            changed: false,
            alert: self.session.alerts.text_for(&todo.id),
            local: true,
        }
    }
}

/// The page for `snapshot` as `view` wants it, with what `session` adds.
pub fn page(snapshot: &DashboardSnapshot, view: &View, session: &Session) -> Page {
    let flags = Flags::of(snapshot, view, session);
    let icons: HashMap<&str, Option<Icon>> = snapshot
        .boards
        .iter()
        .flat_map(|board| &board.groups)
        .map(|group| (group.source.as_str(), group.icon.clone()))
        .collect();
    let attention = snapshot.config_error.is_some()
        || snapshot
            .sources
            .iter()
            .any(|source| matches!(source.status, SourceStatus::Degraded | SourceStatus::Failed));

    Page {
        activity: activity(snapshot, &flags),
        groups: groups(snapshot, view, &flags),
        sources: sources(snapshot, &icons),
        snoozed: snoozed(snapshot, &flags, &icons),
        attention,
        updated: format!("Updated {}", local_time(snapshot.generated_at)),
        version: snapshot.version.clone(),
        version_line: (!snapshot.version.is_empty()).then(|| format!("· v{}", snapshot.version)),
        snoozed_line: (!snapshot.snoozed.is_empty())
            .then(|| format!("· {} snoozed", snapshot.snoozed.len())),
        config_error: snapshot.config_error.clone(),
    }
}

/// The groups the view lets through, in config order whatever the board.
fn groups(snapshot: &DashboardSnapshot, view: &View, flags: &Flags) -> Vec<GroupView> {
    let health: HashMap<&str, &SourceHealth> = snapshot
        .sources
        .iter()
        .map(|source| (source.name.as_str(), source))
        .collect();
    let config_index = |source: &str| {
        snapshot
            .sources
            .iter()
            .position(|health| health.name == source)
            .unwrap_or(usize::MAX)
    };
    let mut groups: Vec<(usize, GroupView)> = Vec::new();
    for board in &snapshot.boards {
        if view.board.as_deref().is_some_and(|only| only != board.name)
            || view.hidden.contains(&hide_key::board(&board.name))
        {
            continue;
        }
        for group in &board.groups {
            if view
                .hidden
                .contains(&hide_key::group(&board.name, &group.source))
            {
                continue;
            }
            let key = group_key(&board.name, &group.source);
            let expanded = view.expanded.contains(&key);
            let visible: Vec<_> = group
                .columns
                .iter()
                .filter(|column| {
                    !view.hidden.contains(&hide_key::column(
                        &board.name,
                        &group.source,
                        &column.name,
                    ))
                })
                .collect();
            if visible.is_empty() {
                continue;
            }
            let empty = visible.iter().filter(|c| c.cards.is_empty()).count();
            let source_health = health.get(group.source.as_str()).copied();
            let stacks = visible
                .iter()
                .filter(|column| !column.cards.is_empty() || expanded)
                .map(|column| {
                    let key = column_key(&board.name, &group.source, &column.name);
                    let cards: Vec<CardView> =
                        column.cards.iter().map(|card| flags.card(card)).collect();
                    let empty_note = column.cards.is_empty().then(|| {
                        source_health.and_then(|h| h.last_refresh_at).map_or_else(
                            || "not refreshed yet".to_owned(),
                            |at| format!("updated {}", local_time(at)),
                        )
                    });
                    stack_view(key, &column.name, cards, view, empty_note)
                })
                .collect();
            groups.push((
                config_index(&group.source),
                GroupView {
                    key,
                    board: board.name.clone(),
                    source: group.source.clone(),
                    icon: group.icon.clone(),
                    expanded,
                    empty,
                    empty_toggle: EmptyToggle::new(empty, expanded),
                    stacks,
                },
            ));
        }
    }
    groups.sort_by_key(|(index, _)| *index);
    groups.into_iter().map(|(_, group)| group).collect()
}

/// The stack `name` at `key` holding `cards`: the first [`COLUMN_LIMIT`]
/// unless the view opened it, collapsed when the view says so.
fn stack_view(
    key: String,
    name: &str,
    cards: Vec<CardView>,
    view: &View,
    empty_note: Option<String>,
) -> StackView {
    let open = view.open_columns.contains(&key);
    let count = cards.len();
    let shown = if open { count } else { count.min(COLUMN_LIMIT) };
    StackView {
        changed: cards.iter().any(|card| card.changed),
        collapsed: view.collapsed.contains(&key),
        open,
        count,
        more: count - shown,
        more_label: more_label(count, count - shown, open),
        cards: cards.into_iter().take(shown).collect(),
        empty_note,
        key,
        name: name.to_owned(),
    }
}

/// The picked cards by stage, in picking order and once each, whatever
/// the filter: picking is an explicit choice. An id that is on no board is
/// skipped. The todos follow them.
fn activity(snapshot: &DashboardSnapshot, flags: &Flags) -> ActivityView {
    let mut found: HashMap<&str, (&PendingCard, &str, &Option<Icon>)> = HashMap::new();
    for board in &snapshot.boards {
        for group in &board.groups {
            for column in &group.columns {
                for card in &column.cards {
                    found.entry(card.id.as_str()).or_insert((
                        card,
                        group.source.as_str(),
                        &group.icon,
                    ));
                }
            }
        }
    }
    let items: Vec<ActivityItem> = snapshot
        .marked
        .iter()
        .filter_map(|id| found.get(id.as_str()))
        .map(|(card, source, icon)| ActivityItem {
            card: flags.card(card),
            source: (*source).to_owned(),
            icon: (*icon).clone(),
        })
        .chain(flags.session.todos.iter().map(|todo| ActivityItem {
            card: flags.todo(todo),
            source: TODO_GROUP.to_owned(),
            icon: None,
        }))
        .collect();
    let count = items.len();
    let stages = Stage::ALL
        .into_iter()
        .map(|stage| StageView {
            stage,
            name: stage.name(),
            cards: items
                .iter()
                .filter(|item| item.card.stage == Some(stage))
                .cloned()
                .collect(),
            empty_note: stage_empty_note(stage),
            adds_todo: stage == Stage::Picked,
        })
        .collect();
    ActivityView {
        stages,
        count,
        hint: (count == 0).then_some(ACTIVITY_HINT),
    }
}

/// Every source as the Sources dialog lists it, in config order.
fn sources(snapshot: &DashboardSnapshot, icons: &HashMap<&str, Option<Icon>>) -> Vec<SourceView> {
    let board_of = |source: &str| {
        snapshot
            .boards
            .iter()
            .filter(|board| board.groups.iter().any(|group| group.source == source))
            .map(|board| board.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    snapshot
        .sources
        .iter()
        .map(|source| SourceView {
            name: source.name.clone(),
            icon: icons.get(source.name.as_str()).cloned().flatten(),
            boards: board_of(&source.name),
            status_text: status_text(source.status),
            status_icon: status_icon(source.status),
            status_class: status_class(source.status),
            time: source.last_refresh_at.map_or_else(
                || "not refreshed yet".to_owned(),
                |at| format!("last fetch {}", local_time(at)),
            ),
            message: source.message.clone(),
        })
        .collect()
}

/// The snoozed cards as the Snoozed dialog lists them.
fn snoozed(
    snapshot: &DashboardSnapshot,
    flags: &Flags,
    icons: &HashMap<&str, Option<Icon>>,
) -> Vec<SnoozedView> {
    snapshot
        .snoozed
        .iter()
        .map(|entry| SnoozedView {
            card: flags.card(&entry.card),
            source: entry.source.clone(),
            icon: icons.get(entry.source.as_str()).cloned().flatten(),
            until: until_text(entry.until),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use pending_core::{Board, CardSeverity, Column, Group};

    use super::*;
    use crate::alert::Alert;

    fn page(snapshot: &DashboardSnapshot, view: &View) -> Page {
        super::page(snapshot, view, &Session::default())
    }

    fn card(id: &str, title: &str) -> PendingCard {
        PendingCard {
            id: id.to_owned(),
            title: title.to_owned(),
            body: format!("{title} body"),
            source: "x".to_owned(),
            url: Some(format!("https://example.com/{id}")),
            due_at: None,
            severity: CardSeverity::Info,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    fn health(name: &str, status: SourceStatus) -> SourceHealth {
        SourceHealth {
            name: name.to_owned(),
            status,
            last_refresh_at: None,
            message: None,
        }
    }

    /// Two boards; the sources are configured as jira, then github, so the
    /// github group (first on its board) comes after jira on the page.
    fn snapshot() -> DashboardSnapshot {
        DashboardSnapshot {
            generated_at: DateTime::UNIX_EPOCH,
            boards: vec![
                Board {
                    name: "Work".to_owned(),
                    groups: vec![Group {
                        source: "github".to_owned(),
                        icon: None,
                        columns: vec![
                            Column {
                                name: "Review".to_owned(),
                                cards: (1..=7).map(|n| card(&format!("gh:{n}"), "PR")).collect(),
                            },
                            Column {
                                name: "Drafts".to_owned(),
                                cards: Vec::new(),
                            },
                        ],
                    }],
                },
                Board {
                    name: "Personal".to_owned(),
                    groups: vec![Group {
                        source: "jira".to_owned(),
                        icon: Some(Icon {
                            url: "https://icons.example/jira.png".to_owned(),
                            dark_url: None,
                        }),
                        columns: vec![Column {
                            name: "Todo".to_owned(),
                            cards: vec![card("j:1", "Issue")],
                        }],
                    }],
                },
            ],
            sources: vec![
                health("jira", SourceStatus::Ready),
                health("github", SourceStatus::Ready),
            ],
            config_error: None,
            marked: vec!["j:1".to_owned(), "gh:2".to_owned(), "nope".to_owned()],
            version: "0.6.0".to_owned(),
            snoozed: Vec::new(),
            changed: vec!["gh:3".to_owned()],
        }
    }

    /// Groups follow config order; a stack shows five
    /// cards and counts the rest; the empty stack is counted but not shown
    /// until expanded.
    #[test]
    fn lays_out_groups_and_stacks() {
        let shown = page(&snapshot(), &View::default());
        let sources: Vec<_> = shown.groups.iter().map(|g| g.source.as_str()).collect();
        assert_eq!(sources, ["jira", "github"]);

        let github = &shown.groups[1];
        assert_eq!(github.key, "Work/github");
        assert_eq!(github.empty, 1);
        assert_eq!(github.stacks.len(), 1, "the empty stack is hidden");
        let review = &github.stacks[0];
        assert_eq!((review.count, review.cards.len(), review.more), (7, 5, 2));
        assert!(review.changed, "gh:3 changed");
        assert!(review.cards[2].changed);
        assert_eq!(review.cards[1].stage, Some(Stage::Picked));
        assert_eq!(review.cards[0].stage, None);
        assert_eq!(
            review.cards[0].url.as_deref(),
            Some("https://example.com/gh:1")
        );

        let mut view = View::default();
        view.expanded.insert("Work/github".to_owned());
        view.open_columns.insert("Work/github/Review".to_owned());
        let shown = page(&snapshot(), &view);
        let github = &shown.groups[1];
        assert_eq!(github.stacks.len(), 2);
        assert_eq!(github.stacks[0].more, 0);
        assert_eq!(github.stacks[0].cards.len(), 7);
        assert_eq!(
            github.stacks[1].empty_note.as_deref(),
            Some("not refreshed yet")
        );
    }

    /// A card's menu sends it to the Activity area, or takes it back when
    /// it is there; on the Board it also snoozes the card, with the four
    /// snooze choices. In the Activity area it moves the card on to the
    /// stages ahead or back to the ones behind, and never snoozes. Anywhere
    /// it sets an alert, with the four waits.
    #[test]
    fn menus_offer_what_fits_the_card() {
        let mut view = View::default();
        view.set_stage("gh:2", Stage::Done);
        let shown = page(&snapshot(), &view);
        let review = &shown.groups[1].stacks[0];
        let plain = card_menu(&review.cards[0], Place::Board);
        assert_eq!(
            plain[0],
            MenuItem::action("Send to Activity", CardAction::Pick)
        );
        let MenuItem::Submenu { label, items } = &plain[1] else {
            panic!("expected the snooze submenu, got {:?}", plain[1]);
        };
        assert_eq!(label, "Snooze");
        assert_eq!(
            items
                .iter()
                .map(|(label, action)| (label.as_str(), *action))
                .collect::<Vec<_>>(),
            [
                ("1 hour", CardAction::Snooze(snooze::Choice::Hour)),
                ("Tomorrow", CardAction::Snooze(snooze::Choice::Tomorrow)),
                ("Next week", CardAction::Snooze(snooze::Choice::NextWeek)),
                (
                    "Until it changes",
                    CardAction::Snooze(snooze::Choice::UntilChange)
                ),
            ]
        );
        let MenuItem::Submenu { label, items } = &plain[2] else {
            panic!("expected the alert submenu, got {:?}", plain[2]);
        };
        assert_eq!(label, "Alert me about");
        assert_eq!(
            items
                .iter()
                .map(|(label, action)| (label.as_str(), *action))
                .collect::<Vec<_>>(),
            [
                ("in 15 min", CardAction::Alert(alert::Choice::Quarter)),
                ("in 30 min", CardAction::Alert(alert::Choice::Half)),
                ("in 1 hour", CardAction::Alert(alert::Choice::Hour)),
                ("in 3 hours", CardAction::Alert(alert::Choice::ThreeHours)),
            ]
        );
        assert_eq!(plain.len(), 3);

        let picked = card_menu(&review.cards[1], Place::Board);
        assert_eq!(
            picked[0],
            MenuItem::action("Remove from Activity", CardAction::Unpick)
        );
        assert_eq!(
            picked.len(),
            3,
            "a picked card on the Board still snoozes, and does not move"
        );

        let labels = |items: Vec<MenuItem>| {
            items
                .into_iter()
                .map(|item| match item {
                    MenuItem::Action { label, action } => (label, Some(action)),
                    MenuItem::Submenu { label, .. } => (label, None),
                })
                .collect::<Vec<_>>()
        };
        let just_picked = &shown.activity.stages[0].cards[0].card;
        assert_eq!(
            labels(card_menu(just_picked, Place::Activity)),
            [
                (
                    "Move to In Progress".to_owned(),
                    Some(CardAction::Stage(Stage::InProgress))
                ),
                (
                    "Move to Done".to_owned(),
                    Some(CardAction::Stage(Stage::Done))
                ),
                ("Remove from Activity".to_owned(), Some(CardAction::Unpick)),
                ("Alert me about".to_owned(), None),
            ]
        );
        let done = &shown.activity.stages[2].cards[0].card;
        assert_eq!(
            labels(card_menu(done, Place::Activity)),
            [
                (
                    "Back to Picked".to_owned(),
                    Some(CardAction::Stage(Stage::Picked))
                ),
                (
                    "Back to In Progress".to_owned(),
                    Some(CardAction::Stage(Stage::InProgress))
                ),
                ("Remove from Activity".to_owned(), Some(CardAction::Unpick)),
                ("Alert me about".to_owned(), None),
            ]
        );
    }

    /// A card with an alert set says when it rings, wherever it is drawn,
    /// and its menu can call the alert off; without one, there is nothing
    /// to call off. The alert changes the page, so it is redrawn.
    #[test]
    fn shows_the_alert_set_on_a_card() {
        let at = Local.with_ymd_and_hms(2026, 10, 7, 15, 45, 0).unwrap();
        let mut session = Session::default();
        let before = super::page(&snapshot(), &View::default(), &session);
        session.alerts.set(
            "gh:2",
            Alert {
                at,
                title: "PR".to_owned(),
                source: "github".to_owned(),
            },
        );
        let shown = super::page(&snapshot(), &View::default(), &session);
        assert_ne!(
            board_view(&ViewState::Loading, Some(&before), Tab::Board),
            board_view(&ViewState::Loading, Some(&shown), Tab::Board)
        );
        let in_stack = &shown.groups[1].stacks[0].cards[1];
        assert_eq!(in_stack.alert.as_deref(), Some("Alert at 15:45"));
        assert_eq!(in_stack.source, "x");
        assert_eq!(
            shown.activity.stages[0].cards[1].card.alert.as_deref(),
            Some("Alert at 15:45")
        );
        assert_eq!(shown.groups[1].stacks[0].cards[0].alert, None);
        let menu = card_menu(in_stack, Place::Board);
        assert_eq!(
            menu.last(),
            Some(&MenuItem::action(
                "Cancel the alert",
                CardAction::CancelAlert
            ))
        );
        assert_eq!(menu.len(), 4);
        let without = card_menu(&shown.groups[1].stacks[0].cards[0], Place::Board);
        assert_eq!(without.len(), 3, "nothing to call off");
    }

    /// The todos written down in the window live in the Activity area,
    /// not on the Board: one lands in Picked, after the sources' cards,
    /// whatever the board filter, and moves along the stages from its
    /// menu, which cannot take it out of the Activity (it has nowhere else
    /// to be) and has Edit and Delete instead. Only Picked offers to write
    /// one down.
    #[test]
    fn keeps_the_todos_in_the_activity() {
        let mut session = Session::default();
        let first = session
            .todos
            .add("Write the release notes", "for 0.7")
            .unwrap();
        let second = session.todos.add("Call the bank", "").unwrap();
        assert!(session.todos.move_to(&second, Stage::InProgress));
        let view = View {
            board: Some("Work".to_owned()),
            ..View::default()
        };
        let shown = super::page(&snapshot(), &view, &session);
        assert!(
            shown.groups.iter().all(|group| group.source != "Todo"),
            "not on the Board"
        );

        let [picked, doing, done] = &shown.activity.stages[..] else {
            panic!("three stages");
        };
        let ids: Vec<_> = picked
            .cards
            .iter()
            .map(|item| item.card.id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["j:1", "gh:2", first.as_str()],
            "after the sources' cards"
        );
        let card = &picked.cards[2].card;
        assert_eq!(
            (card.title.as_str(), card.body.as_str()),
            ("Write the release notes", "for 0.7")
        );
        assert!(card.local && card.url.is_none());
        assert_eq!(picked.cards[2].source, "Todo");
        assert_eq!(picked.cards[2].icon, None);
        assert_eq!(doing.cards.len(), 1);
        assert_eq!(doing.cards[0].card.id, second);
        assert_eq!(shown.activity.count, 4);
        assert!(picked.adds_todo && !doing.adds_todo && !done.adds_todo);

        let labels = |items: Vec<MenuItem>| {
            items
                .into_iter()
                .map(|item| match item {
                    MenuItem::Action { label, .. } | MenuItem::Submenu { label, .. } => label,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(card_menu(card, Place::Activity)),
            [
                "Move to In Progress",
                "Move to Done",
                "Alert me about",
                "Edit",
                "Delete"
            ]
        );
        assert_eq!(
            labels(card_menu(&doing.cards[0].card, Place::Activity)),
            [
                "Back to Picked",
                "Move to Done",
                "Alert me about",
                "Edit",
                "Delete"
            ]
        );
        assert_eq!(todo_dialog_words(false), ("New todo", "Add"));
        assert_eq!(todo_dialog_words(true), ("Edit todo", "Save"));
        assert_eq!(
            TODO_NOTE,
            "Kept only while TasksPending runs: quitting it forgets every todo."
        );
    }

    /// Settings shows the icon as Omarchy's settings place it, except
    /// right after Settings changed it, while the shell has yet to write
    /// them: that change is trusted until they agree, and for a few
    /// seconds at most, so a change Omarchy lost does not stay on screen.
    /// A change under way or refused says so in place of On or Off.
    #[test]
    fn shows_the_icon_as_omarchy_places_it() {
        let view = |placed, asked, change| bar_view(placed, asked, change);
        let soon = Duration::from_secs(1);
        let late = BAR_SETTLE + Duration::from_millis(1);
        let shown = |on, state, keep_asked| BarView {
            on,
            state,
            keep_asked,
        };
        assert_eq!(view(true, None, None), shown(true, BarState::On, false));
        assert_eq!(view(false, None, None), shown(false, BarState::Off, false));
        assert_eq!(
            view(false, Some((true, soon)), None),
            shown(true, BarState::On, true),
            "just turned on"
        );
        assert_eq!(
            view(true, Some((false, soon)), None),
            shown(false, BarState::Off, true),
            "just turned off"
        );
        assert_eq!(
            view(true, Some((true, soon)), None),
            shown(true, BarState::On, false),
            "they agree"
        );
        assert_eq!(
            view(false, Some((true, late)), None),
            shown(false, BarState::Off, false),
            "Omarchy never placed it"
        );
        assert_eq!(
            view(false, None, Some(&BarState::Changing(true))),
            shown(false, BarState::Changing(true), false)
        );
        assert_eq!(
            view(true, None, Some(&BarState::Failed("why".to_owned()))),
            shown(true, BarState::Failed("why".to_owned()), false)
        );
    }

    /// Settings offers the icon on Omarchy's bar off or on, and says what
    /// it does, what is under way and what went wrong.
    #[test]
    fn offers_the_icon_on_omarchys_bar() {
        let labels = |on| {
            bar_choices(on)
                .into_iter()
                .map(|choice| (choice.label, choice.value, choice.active))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(false),
            [
                ("Off".to_owned(), false, true),
                ("On".to_owned(), true, false)
            ]
        );
        assert_eq!(
            labels(true),
            [
                ("Off".to_owned(), false, false),
                ("On".to_owned(), true, true)
            ]
        );
        assert!(bar_note(&BarState::Off).contains("drops this window down"));
        let on = bar_note(&BarState::On);
        assert!(
            on.contains("only hides it") && on.contains("Ctrl+Q"),
            "{on}"
        );
        assert!(on.contains("tasks-pending toggle"), "{on}");
        assert_eq!(
            bar_note(&BarState::Changing(true)),
            "Putting the icon on the bar…"
        );
        assert_eq!(
            bar_note(&BarState::Changing(false)),
            "Taking the icon off the bar…"
        );
        assert_eq!(
            bar_note(&BarState::Failed("plugin 'x' is not known".to_owned())),
            "Omarchy did not take it: plugin 'x' is not known"
        );
    }

    /// Only an http(s) URL becomes a link: a source could hand the window
    /// a `javascript:` or `file:` URL, and the title must not open it. The
    /// link is the URL without the spaces around it, which the browser
    /// would not take.
    #[test]
    fn links_only_to_http_urls() {
        let mut snapshot = snapshot();
        let cards = &mut snapshot.boards[0].groups[0].columns[0].cards;
        cards[0].url = Some("javascript:alert(1)".to_owned());
        cards[1].url = Some("file:///etc/passwd".to_owned());
        cards[2].url = Some(" HTTPS://example.com/ok ".to_owned());
        cards[3].url = None;
        let shown = page(&snapshot, &View::default());
        let urls: Vec<_> = shown.groups[1].stacks[0].cards[..4]
            .iter()
            .map(|card| card.url.as_deref())
            .collect();
        assert_eq!(urls, [None, None, Some("HTTPS://example.com/ok"), None]);
        assert_eq!(
            link_note("no browser"),
            "Could not open the link (no browser)"
        );
    }

    /// The Activity area has three stages, Picked, In Progress and Done /
    /// to mark; a picked card is at Picked until moved, and the cards at a
    /// stage keep their picking order. Each empty stage says so, and while
    /// nothing is picked a hint points to the Board; the Activity tab
    /// counts the picked cards and the Board tab counts nothing.
    #[test]
    fn lays_out_the_activity_by_stage() {
        let mut view = View::default();
        view.set_stage("gh:2", Stage::InProgress);
        view.set_stage("gh:3", Stage::Done);
        let shown = page(&snapshot(), &view);
        let activity = &shown.activity;
        assert_eq!(activity.count, 2);
        assert_eq!(activity.hint, None);
        let stages: Vec<_> = activity
            .stages
            .iter()
            .map(|stage| {
                (
                    stage.name,
                    stage
                        .cards
                        .iter()
                        .map(|item| item.card.id.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            stages,
            [
                ("Picked", vec!["j:1"]),
                ("In Progress", vec!["gh:2"]),
                ("Done / to mark", vec![]),
            ],
            "gh:3 is not picked, so its stage means nothing"
        );
        assert_eq!(activity.stages[0].stage, Stage::Picked);
        assert_eq!(activity.stages[2].empty_note, "Nothing to mark");
        assert_eq!(
            activity.stages[1].cards[0].card.stage,
            Some(Stage::InProgress)
        );
        assert_eq!(
            tabs(Some(&shown)).map(|tab| (tab.label, tab.count)),
            [("Activity", Some(2)), ("Board", None)]
        );

        let mut unpicked = snapshot();
        unpicked.marked.clear();
        let shown = page(&unpicked, &view);
        assert_eq!(shown.activity.count, 0);
        assert_eq!(shown.activity.hint, Some(ACTIVITY_HINT));
        assert_eq!(shown.activity.stages[0].empty_note, "Nothing picked");
        assert_eq!(shown.activity.stages[1].empty_note, "Nothing under way");
        assert!(
            shown
                .activity
                .stages
                .iter()
                .all(|stage| stage.cards.is_empty())
        );
        assert_eq!(
            tabs(Some(&shown)).map(|tab| tab.count),
            [None, None],
            "no count on an empty Activity"
        );
        assert_eq!(tabs(None)[0].count, None);
    }

    /// The board filter and hidden items leave groups out; the picked cards
    /// stay in the Activity area whatever the filter, in picking order,
    /// skipping ids that are not on the boards.
    #[test]
    fn filters_groups_but_not_the_picked_cards() {
        let view = View {
            board: Some("Work".to_owned()),
            ..View::default()
        };
        let shown = page(&snapshot(), &view);
        assert_eq!(shown.groups.len(), 1);
        assert_eq!(shown.groups[0].source, "github");
        let picked = &shown.activity.stages[0].cards;
        let ids: Vec<_> = picked.iter().map(|n| n.card.id.as_str()).collect();
        assert_eq!(ids, ["j:1", "gh:2"]);
        assert_eq!(picked[0].source, "jira");
        assert!(picked[0].icon.is_some());

        let mut view = View::default();
        view.hidden
            .insert(hide_key::column("Work", "github", "Review"));
        view.hidden
            .insert(hide_key::column("Work", "github", "Drafts"));
        let shown = page(&snapshot(), &view);
        assert_eq!(
            shown.groups.len(),
            1,
            "a group with every stack hidden is gone"
        );
        assert_eq!(shown.groups[0].source, "jira");
    }

    /// The controls know when a source needs attention, and the dialogs
    /// list sources and snoozed cards with their texts.
    #[test]
    fn summarizes_sources_and_snoozes() {
        let mut snapshot = snapshot();
        let shown = page(&snapshot, &View::default());
        assert!(!shown.attention);
        assert_eq!(shown.version, "0.6.0");
        assert_eq!(shown.sources[1].boards, "Work");
        assert_eq!(shown.sources[1].status_text, "Ready");
        assert_eq!(shown.sources[1].status_icon, "object-select-symbolic");
        assert_eq!(shown.sources[1].status_class, "ready");
        assert_eq!(shown.sources[1].time, "not refreshed yet");

        snapshot.sources[0].status = SourceStatus::Failed;
        snapshot.sources[0].message = Some("401".to_owned());
        snapshot.snoozed.push(pending_core::SnoozedCard {
            card: card("gh:9", "Later"),
            source: "github".to_owned(),
            until: None,
        });
        let shown = page(&snapshot, &View::default());
        assert!(shown.attention);
        assert_eq!(shown.sources[0].message.as_deref(), Some("401"));
        assert_eq!(shown.sources[0].status_icon, "process-stop-symbolic");
        assert_eq!(shown.sources[0].status_class, "failed");
        assert_eq!(shown.snoozed[0].until, "until it changes");
        assert_eq!(shown.snoozed[0].source, "github");
        assert_eq!(shown.snoozed[0].footer(), "github · until it changes");

        snapshot.sources[0].status = SourceStatus::Ready;
        snapshot.config_error = Some("bad toml".to_owned());
        assert!(page(&snapshot, &View::default()).attention);
    }

    fn drawn(state: &ViewState, view: &View) -> BoardView {
        drawn_tab(state, view, Tab::Board)
    }

    fn drawn_tab(state: &ViewState, view: &View, tab: Tab) -> BoardView {
        board_view(state, state.snapshot().map(|s| page(s, view)).as_ref(), tab)
    }

    /// A fetch that only moved the snapshot's time and the sources' fetch
    /// times draws the same board, so the page is not rebuilt; it is when
    /// an empty stack on show says when its source was fetched, or when a
    /// card or the daemon's answer changed.
    #[test]
    fn draws_the_same_board_until_something_on_it_changes() {
        let now = Local::now();
        let first = ViewState::Loading.next(Ok(snapshot()), now);
        let mut fetched = snapshot();
        fetched.generated_at = Utc::now();
        for source in &mut fetched.sources {
            source.last_refresh_at = Some(Utc::now());
        }
        let later = ViewState::Loading.next(Ok(fetched), now);
        assert_eq!(
            drawn(&first, &View::default()),
            drawn(&later, &View::default())
        );

        let mut view = View::default();
        view.expanded.insert("Work/github".to_owned());
        assert_ne!(drawn(&first, &view), drawn(&later, &view));

        let mut unmarked = snapshot();
        unmarked.marked.clear();
        let unmarked = ViewState::Loading.next(Ok(unmarked), now);
        assert_ne!(
            drawn(&first, &View::default()),
            drawn(&unmarked, &View::default())
        );
        let stale = first.clone().next(Err("down".to_owned()), now);
        assert_ne!(
            drawn(&first, &View::default()),
            drawn(&stale, &View::default())
        );
        assert_ne!(
            drawn_tab(&first, &View::default(), Tab::Activity),
            drawn(&first, &View::default()),
            "switching tabs redraws"
        );
        assert_eq!(
            Tab::default(),
            Tab::Activity,
            "the window opens on the Activity"
        );
    }

    /// Loading and an unreachable daemon stand instead of the board; a
    /// stale dashboard keeps its cards under the notice.
    #[test]
    fn notices_before_and_over_the_board() {
        let at = Local.with_ymd_and_hms(2026, 9, 28, 10, 30, 0).unwrap();
        let view = View::default();
        let loading = drawn(&ViewState::Loading, &view);
        assert_eq!(loading.notice, Some(Notice::Loading));
        assert!(loading.groups.is_empty() && loading.activity.is_none());

        let unavailable = ViewState::Loading.next(Err("refused".to_owned()), at);
        let unavailable = drawn(&unavailable, &view);
        assert_eq!(
            unavailable.notice,
            Some(Notice::Unavailable("refused".to_owned()))
        );
        assert!(unavailable.groups.is_empty());

        let stale = ViewState::Loading
            .next(Ok(snapshot()), at)
            .next(Err("refused".to_owned()), at);
        let stale = drawn(&stale, &view);
        assert_eq!(
            stale.notice,
            Some(Notice::Stale(
                "Dashboard unreachable (refused); showing data from 28 Sep 10:30".to_owned()
            ))
        );
        assert_eq!(stale.groups.len(), 2);
        assert_eq!(stale.activity.map(|a| a.count), Some(2));
    }

    /// A refresh the daemon took holds the button until it would take
    /// another. A refusal shows only for a refresh the user asked for: as
    /// a wait on the button, or as a note when it failed outright.
    #[test]
    fn shows_the_answer_to_a_refresh() {
        let none = Refreshed {
            hold: None,
            note: None,
        };
        let accepted = Some((REFRESH_COOLDOWN, "Refreshing…".to_owned()));
        assert_eq!(refreshed(false, &Ok(())).hold, accepted);
        assert_eq!(refreshed(true, &Ok(())).hold, accepted);
        let failed = Err(Refusal::Failed("HTTP 500".to_owned()));
        assert_eq!(refreshed(false, &Err(Refusal::Wait(7))), none);
        assert_eq!(refreshed(false, &failed), none);
        assert_eq!(
            refreshed(true, &Err(Refusal::Wait(7))),
            Refreshed {
                hold: Some((Duration::from_secs(7), "Wait 7s".to_owned())),
                note: None,
            }
        );
        assert_eq!(
            refreshed(true, &failed),
            Refreshed {
                hold: None,
                note: Some("Could not refresh (HTTP 500)".to_owned()),
            }
        );
    }

    /// The button is off while its hold lasts, and the timer of an earlier
    /// hold firing meanwhile does not end a later one.
    #[test]
    fn holds_the_refresh_button_while_the_hold_lasts() {
        let now = Instant::now();
        let hold = (now + Duration::from_secs(5), "Refreshing…".to_owned());
        assert_eq!(
            refresh_button(Some(&hold), now),
            ("Refreshing…".to_owned(), false)
        );
        let ended = now + Duration::from_secs(5);
        assert_eq!(
            refresh_button(Some(&hold), ended),
            ("Refresh".to_owned(), true)
        );
        assert_eq!(refresh_button(None, now), ("Refresh".to_owned(), true));
        assert_eq!(lasting(Some(hold.clone()), now), Some(hold.clone()));
        assert_eq!(lasting(Some(hold), ended), None);
    }

    /// The timers read and refresh only while the window is shown; the
    /// version is asked for regardless, being the daemon's own concern.
    #[test]
    fn asks_nothing_of_the_daemon_while_hidden() {
        let asked = |timer, hidden| format!("{:?}", on_timer(timer, hidden));
        assert_eq!(asked(Timer::Poll, false), "Some(Poll)");
        assert_eq!(asked(Timer::Weather, false), "Some(Weather)");
        assert_eq!(asked(Timer::Version, false), "Some(Version)");
        assert_eq!(
            asked(Timer::AutoRefresh, false),
            "Some(Refresh { manual: false })"
        );
        assert_eq!(asked(Timer::Poll, true), "None");
        assert_eq!(asked(Timer::Weather, true), "None");
        assert_eq!(asked(Timer::AutoRefresh, true), "None");
        assert_eq!(asked(Timer::Version, true), "Some(Version)");
    }

    /// The auto-refresh setting is in minutes, and none means off.
    #[test]
    fn refreshes_every_source_as_often_as_set() {
        assert_eq!(auto_refresh_every(0), None);
        assert_eq!(auto_refresh_every(15), Some(Duration::from_secs(15 * 60)));
    }

    /// What the group header's toggle and a stack's "more" button say.
    #[test]
    fn labels_the_toggles() {
        let shown = page(&snapshot(), &View::default());
        let github = &shown.groups[1];
        assert_eq!(
            github.empty_toggle,
            Some(EmptyToggle {
                icon: "view-reveal-symbolic",
                label: "1 empty".to_owned(),
                tooltip: "Show empty stacks",
            })
        );
        assert_eq!(shown.groups[0].empty_toggle, None, "jira has none");
        assert_eq!(github.stacks[0].more_label.as_deref(), Some("Show 2 more"));
        assert_eq!(shown.groups[0].stacks[0].more_label, None);

        let mut view = View::default();
        view.expanded.insert("Work/github".to_owned());
        view.open_columns.insert("Work/github/Review".to_owned());
        let shown = page(&snapshot(), &view);
        let github = &shown.groups[1];
        let toggle = github.empty_toggle.as_ref().unwrap();
        assert_eq!(
            (toggle.icon, toggle.label.as_str(), toggle.tooltip),
            ("view-conceal-symbolic", "hide empty", "Hide empty stacks")
        );
        assert_eq!(github.stacks[0].more_label.as_deref(), Some("Show less"));
    }

    /// The line under the controls: the version, the snoozed count, and a
    /// newer version when there is one.
    #[test]
    fn writes_the_status_line() {
        let mut snapshot = snapshot();
        let shown = page(&snapshot, &View::default());
        assert_eq!(shown.version_line.as_deref(), Some("· v0.6.0"));
        assert_eq!(shown.snoozed_line, None);
        snapshot.version.clear();
        snapshot.snoozed.push(pending_core::SnoozedCard {
            card: card("gh:9", "Later"),
            source: "github".to_owned(),
            until: None,
        });
        let shown = page(&snapshot, &View::default());
        assert_eq!(shown.version_line, None);
        assert_eq!(shown.snoozed_line.as_deref(), Some("· 1 snoozed"));
        assert_eq!(update_line("0.7.0"), "· v0.7.0 available");
    }

    /// Settings offers every board and every interval, the one in effect
    /// marked; a switch under a hidden board or group is greyed out and
    /// keeps its own setting.
    #[test]
    fn lays_out_the_settings() {
        let mut view = View {
            board: Some("Work".to_owned()),
            ..View::default()
        };
        let boards: Vec<_> = board_choices(&snapshot(), &view)
            .into_iter()
            .map(|c| (c.label, c.value, c.active))
            .collect();
        assert_eq!(
            boards,
            [
                ("All".to_owned(), None, false),
                ("Work".to_owned(), Some("Work".to_owned()), true),
                ("Personal".to_owned(), Some("Personal".to_owned()), false),
            ]
        );
        let intervals: Vec<_> = interval_choices(&view)
            .into_iter()
            .map(|c| (c.label, c.value, c.active))
            .collect();
        assert_eq!(intervals[0], ("Off".to_owned(), 0, false));
        assert_eq!(intervals[1], ("1 min".to_owned(), 1, true));
        assert_eq!(intervals.len(), 5);

        view.hidden.insert(hide_key::board("Work"));
        view.hidden
            .insert(hide_key::column("Work", "github", "Drafts"));
        let tree = switches(&snapshot(), &view);
        let work = &tree[0];
        assert_eq!(work.board.label, "Work");
        assert!(!work.board.on && !work.board.inherited);
        let github = &work.groups[0];
        assert_eq!(github.group.key, hide_key::group("Work", "github"));
        assert!(github.group.on && github.group.inherited);
        let stacks: Vec<_> = github
            .stacks
            .iter()
            .map(|s| (s.label.as_str(), s.on, s.inherited))
            .collect();
        assert_eq!(stacks, [("Review", true, true), ("Drafts", false, true)]);
        let personal = &tree[1].groups[0];
        assert!(personal.icon.is_some());
        assert!(!personal.stacks[0].inherited);
    }

    /// The words and icons that do not depend on the snapshot.
    #[test]
    fn names_the_notice_and_the_weather_icons() {
        let at = Local.with_ymd_and_hms(2026, 9, 28, 10, 30, 0).unwrap();
        assert_eq!(
            stale_notice("refused", at),
            "Dashboard unreachable (refused); showing data from 28 Sep 10:30"
        );
        assert_eq!(
            weather_icon_name(WeatherCondition::ClearNight),
            "weather-clear-night-symbolic"
        );
        assert_eq!(
            weather_icon_name(WeatherCondition::Unknown),
            "weather-overcast-symbolic"
        );
    }

    /// The Refresh button is sized for every text it shows, so it keeps
    /// its width while it refreshes or waits: each label it can show
    /// fits one it is sized for (the same words, no more digits), the
    /// longest wait a daemon can ask included.
    #[test]
    fn sizes_the_refresh_button_for_every_label() {
        let widths = refresh_widths();
        let digits = |text: &str| text.chars().filter(char::is_ascii_digit).count();
        let words =
            |text: &str| -> String { text.chars().filter(|c| !c.is_ascii_digit()).collect() };
        let mut shown = vec![refresh_button(None, Instant::now()).0];
        shown.extend(refreshed(true, &Ok(())).hold.map(|(_, label)| label));
        for secs in 1..=REFRESH_COOLDOWN.as_secs() {
            shown.extend(
                refreshed(true, &Err(Refusal::Wait(secs)))
                    .hold
                    .map(|(_, label)| label),
            );
        }
        for label in shown {
            assert!(
                widths
                    .iter()
                    .any(|width| words(width) == words(&label) && digits(&label) <= digits(width)),
                "{label} is wider than every one of {widths:?}"
            );
        }
    }

    /// Settings offers the window's own colours and Omarchy's, the one in
    /// use lit. Following Omarchy, it says which theme it follows, or why
    /// the window keeps its own colours.
    #[test]
    fn offers_its_own_colours_or_omarchy() {
        let lit = |view: &View| -> Vec<(String, bool)> {
            theme_choices(view)
                .into_iter()
                .map(|choice| (choice.label, choice.active))
                .collect()
        };
        let mut view = View::default();
        assert_eq!(
            lit(&view),
            [
                ("TasksPending".to_owned(), true),
                ("Omarchy".to_owned(), false)
            ]
        );
        let sample = crate::theme::parse_omarchy(crate::theme::SAMPLE).unwrap();
        assert_eq!(theme_note(&view, Some(&Ok(sample.clone()))), None);

        view.theme = Theme::Omarchy;
        assert_eq!(theme_choices(&view)[1].value, Theme::Omarchy);
        assert_eq!(lit(&view)[1], ("Omarchy".to_owned(), true));
        let named = crate::theme::OmarchyTheme {
            name: Some("Osaka Jade".to_owned()),
            ..sample.clone()
        };
        assert_eq!(
            theme_note(&view, Some(&Ok(named))).as_deref(),
            Some("Follows Omarchy's theme, Osaka Jade, and changes with it.")
        );
        assert_eq!(
            theme_note(&view, Some(&Ok(sample))).as_deref(),
            Some("Follows Omarchy's theme and changes with it.")
        );
        assert_eq!(
            theme_note(&view, Some(&Err("no Omarchy theme here".to_owned()))).as_deref(),
            Some("Nothing to follow (no Omarchy theme here): the window keeps its own colours.")
        );
    }
}
