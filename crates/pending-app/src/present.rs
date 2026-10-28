//! What the window shows, worked out from the snapshot and the view, with
//! no widgets involved: the groups and stacks to draw, the cards marked as
//! in progress, the control strip, and the words and icon names the
//! widgets put on screen.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local, Utc};
use pending_core::{
    DashboardSnapshot, Icon, PendingCard, SourceHealth, SourceStatus, WeatherCondition, is_http_url,
};

use crate::snooze::until_text;
use crate::state::ViewState;
use crate::view::{View, column_key, group_key, hide_key};

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
    pub marked: bool,
    pub changed: bool,
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
    /// What to say instead of cards when there are none.
    pub empty_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupView {
    pub key: String,
    pub board: String,
    pub source: String,
    pub icon: Option<Icon>,
    /// Showing the empty stacks too.
    pub expanded: bool,
    /// Stacks without cards, shown or not.
    pub empty: usize,
    pub stacks: Vec<StackView>,
}

/// A card marked as in progress, with where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowItem {
    pub card: CardView,
    pub source: String,
    pub icon: Option<Icon>,
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
    pub now: Vec<NowItem>,
    pub groups: Vec<GroupView>,
    pub sources: Vec<SourceView>,
    pub snoozed: Vec<SnoozedView>,
    /// A source is degraded or failed, or the config was rejected.
    pub attention: bool,
    /// "Updated 28 Sep 10:30".
    pub updated: String,
    pub version: String,
    pub config_error: Option<String>,
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

/// What the page under the hero draws. The window rebuilds the page only
/// when this changes, so a fetch that changed nothing on it (only the
/// sources' fetch times, say) keeps the focus and the reading position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardView {
    pub notice: Option<Notice>,
    pub now: Vec<NowItem>,
    pub groups: Vec<GroupView>,
}

/// The board for `state`, whose snapshot `page` was worked out from.
pub fn board_view(state: &ViewState, page: Option<&Page>) -> BoardView {
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
        now: page.map(|page| page.now.clone()).unwrap_or_default(),
        groups: page.map(|page| page.groups.clone()).unwrap_or_default(),
    }
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

/// The ids the snapshot flags, for building card views.
struct Flags<'a> {
    marked: HashSet<&'a str>,
    changed: HashSet<&'a str>,
}

impl<'a> Flags<'a> {
    fn of(snapshot: &'a DashboardSnapshot) -> Self {
        Self {
            marked: snapshot.marked.iter().map(String::as_str).collect(),
            changed: snapshot.changed.iter().map(String::as_str).collect(),
        }
    }

    fn card(&self, card: &PendingCard) -> CardView {
        CardView {
            id: card.id.clone(),
            title: card.title.clone(),
            url: card.url.clone().filter(|url| is_http_url(url)),
            body: card.body.clone(),
            marked: self.marked.contains(card.id.as_str()),
            changed: self.changed.contains(card.id.as_str()),
        }
    }
}

/// The page for `snapshot` as `view` wants it.
pub fn page(snapshot: &DashboardSnapshot, view: &View) -> Page {
    let flags = Flags::of(snapshot);
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
        now: now(snapshot, &flags),
        groups: groups(snapshot, view, &flags),
        sources: sources(snapshot, &icons),
        snoozed: snoozed(snapshot, &flags, &icons),
        attention,
        updated: format!("Updated {}", local_time(snapshot.generated_at)),
        version: snapshot.version.clone(),
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
                    let open = view.open_columns.contains(&key);
                    let shown = if open {
                        column.cards.len()
                    } else {
                        column.cards.len().min(COLUMN_LIMIT)
                    };
                    StackView {
                        changed: column
                            .cards
                            .iter()
                            .any(|c| flags.changed.contains(c.id.as_str())),
                        collapsed: view.collapsed.contains(&key),
                        open,
                        count: column.cards.len(),
                        more: column.cards.len() - shown,
                        cards: column.cards[..shown]
                            .iter()
                            .map(|card| flags.card(card))
                            .collect(),
                        empty_note: column.cards.is_empty().then(|| {
                            source_health.and_then(|h| h.last_refresh_at).map_or_else(
                                || "not refreshed yet".to_owned(),
                                |at| format!("updated {}", local_time(at)),
                            )
                        }),
                        key,
                        name: column.name.clone(),
                    }
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
                    stacks,
                },
            ));
        }
    }
    groups.sort_by_key(|(index, _)| *index);
    groups.into_iter().map(|(_, group)| group).collect()
}

/// The marked cards, in marking order and once each, whatever the filter:
/// marking is an explicit choice. An id that is on no board is skipped.
fn now(snapshot: &DashboardSnapshot, flags: &Flags) -> Vec<NowItem> {
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
    snapshot
        .marked
        .iter()
        .filter_map(|id| found.get(id.as_str()))
        .map(|(card, source, icon)| NowItem {
            card: flags.card(card),
            source: (*source).to_owned(),
            icon: (*icon).clone(),
        })
        .collect()
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

    /// Groups follow config order; a stack shows five cards and counts the
    /// rest; the empty stack is counted but not shown until expanded.
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
        assert!(review.cards[1].marked);
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

    /// Only an http(s) URL becomes a link: a source could hand the window
    /// a `javascript:` or `file:` URL, and the title must not open it.
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
        assert_eq!(urls, [None, None, Some(" HTTPS://example.com/ok "), None]);
    }

    /// The board filter and hidden items leave groups out; the marked cards
    /// stay in "Now" whatever the filter, in marking order, skipping ids
    /// that are not on the boards.
    #[test]
    fn filters_groups_but_not_the_marked_cards() {
        let view = View {
            board: Some("Work".to_owned()),
            ..View::default()
        };
        let shown = page(&snapshot(), &view);
        assert_eq!(shown.groups.len(), 1);
        assert_eq!(shown.groups[0].source, "github");
        let now: Vec<_> = shown.now.iter().map(|n| n.card.id.as_str()).collect();
        assert_eq!(now, ["j:1", "gh:2"]);
        assert_eq!(shown.now[0].source, "jira");
        assert!(shown.now[0].icon.is_some());

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
        board_view(state, state.snapshot().map(|s| page(s, view)).as_ref())
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
    }

    /// Loading and an unreachable daemon stand instead of the board; a
    /// stale dashboard keeps its cards under the notice.
    #[test]
    fn notices_before_and_over_the_board() {
        let at = Local.with_ymd_and_hms(2026, 9, 28, 10, 30, 0).unwrap();
        let view = View::default();
        let loading = drawn(&ViewState::Loading, &view);
        assert_eq!(loading.notice, Some(Notice::Loading));
        assert!(loading.groups.is_empty() && loading.now.is_empty());

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
        assert_eq!(stale.now.len(), 2);
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
}
