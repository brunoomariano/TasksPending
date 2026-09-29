//! Dashboard state and keyboard handling, independent of the terminal.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use pending_core::{Board, Column, DashboardSnapshot, PendingCard};
use ratatui::layout::{Position, Rect};

/// Minimum wait between manual refreshes. Each one queries every source, and
/// provider APIs rate-limit (GitHub search: 30 requests per minute, and every
/// GitHub column is one search).
pub const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);

struct Notice {
    text: String,
    is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    RefreshNow,
    Open(String),
}

/// A column of the current board, with the source (group) it belongs to.
pub struct ColumnRef<'a> {
    pub source: &'a str,
    pub column: &'a Column,
}

/// A popup over the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    /// The selected card, in full.
    Card,
    /// Every source's status and full message.
    Sources,
}

/// Screen geometry the last render reported, which keys and the mouse need:
/// where the columns are, how many cards fit, how far the popup scrolls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewport {
    /// The columns on screen.
    pub columns: Vec<ColumnArea>,
    /// Cards that fit in a column at once.
    pub card_page: usize,
    /// Popup lines that fit on screen at once.
    pub popup_page: u16,
    /// The furthest the popup scrolls, in lines.
    pub popup_max_scroll: u16,
}

/// A column as drawn: its title row, then two rows per card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnArea {
    /// Index into `App::columns()`.
    pub index: usize,
    pub area: Rect,
    /// The first card shown (the column scrolls to keep the selection visible).
    pub offset: usize,
}

/// Rows a card takes in a column: title and body.
pub const CARD_ROWS: u16 = 2;
/// Popup lines one mouse wheel step scrolls.
const WHEEL_LINES: i32 = 3;

/// Where the selection is, by name, so it survives refreshes.
struct Anchor {
    board: String,
    source: String,
    column: String,
    card: Option<String>,
}

pub struct App {
    snapshot: DashboardSnapshot,
    board: usize,
    /// Index into `columns()` of the current board.
    column: usize,
    /// Index into the selected column's cards.
    card: usize,
    /// Last action feedback, shown until the next key press.
    notice: Option<Notice>,
    last_refresh: Option<Instant>,
    popup: Option<Popup>,
    /// First popup line shown.
    popup_scroll: u16,
    viewport: Viewport,
}

impl App {
    pub fn new(snapshot: DashboardSnapshot) -> Self {
        Self {
            snapshot,
            board: 0,
            column: 0,
            card: 0,
            notice: None,
            last_refresh: None,
            popup: None,
            popup_scroll: 0,
            viewport: Viewport::default(),
        }
    }

    pub fn popup(&self) -> Option<Popup> {
        self.popup
    }

    pub fn popup_scroll(&self) -> u16 {
        self.popup_scroll
    }

    /// Records the geometry of the frame just drawn.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
    }

    pub fn snapshot(&self) -> &DashboardSnapshot {
        &self.snapshot
    }

    pub fn board_index(&self) -> usize {
        self.board
    }

    pub fn current_board(&self) -> Option<&Board> {
        self.snapshot.boards.get(self.board)
    }

    /// Columns of the current board, group after group.
    pub fn columns(&self) -> Vec<ColumnRef<'_>> {
        self.current_board()
            .map(|board| {
                board
                    .groups
                    .iter()
                    .flat_map(|group| {
                        group.columns.iter().map(|column| ColumnRef {
                            source: &group.source,
                            column,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn selected_column(&self) -> usize {
        self.column
    }

    pub fn selected_card(&self) -> Option<&PendingCard> {
        self.columns()
            .get(self.column)
            .and_then(|c| c.column.cards.get(self.card))
    }

    /// Whether the card at `card` in column `column` is the selected one; a
    /// card shown in several columns is selected in one of them only.
    pub fn is_selected(&self, column: usize, card: usize) -> bool {
        column == self.column && card == self.card
    }

    /// Replaces the snapshot, keeping the selection on the same board, column
    /// and card when they still exist, or the nearest position otherwise.
    pub fn update(&mut self, snapshot: DashboardSnapshot) {
        let anchor = self.anchor();
        self.snapshot = snapshot;
        let Some(anchor) = anchor else {
            self.clamp();
            self.close_card_popup_without_card();
            return;
        };

        if let Some(board) = self
            .snapshot
            .boards
            .iter()
            .position(|b| b.name == anchor.board)
        {
            self.board = board;
        }
        if let Some(column) = self
            .columns()
            .iter()
            .position(|c| c.source == anchor.source && c.column.name == anchor.column)
        {
            self.column = column;
            if let Some(card) = anchor.card.and_then(|id| {
                self.columns()[column]
                    .column
                    .cards
                    .iter()
                    .position(|c| c.id == id)
            }) {
                self.card = card;
            }
        }
        self.clamp();
        self.close_card_popup_without_card();
    }

    fn close_card_popup_without_card(&mut self) {
        if self.popup == Some(Popup::Card) && self.selected_card().is_none() {
            self.popup = None;
        }
    }

    fn anchor(&self) -> Option<Anchor> {
        let board = self.current_board()?;
        let columns = self.columns();
        let selected = columns.get(self.column)?;
        Some(Anchor {
            board: board.name.clone(),
            source: selected.source.to_owned(),
            column: selected.column.name.clone(),
            card: selected.column.cards.get(self.card).map(|c| c.id.clone()),
        })
    }

    fn clamp(&mut self) {
        self.board = self.board.min(self.snapshot.boards.len().saturating_sub(1));
        let columns = self.columns().len();
        self.column = self.column.min(columns.saturating_sub(1));
        let cards = self
            .columns()
            .get(self.column)
            .map_or(0, |c| c.column.cards.len());
        self.card = self.card.min(cards.saturating_sub(1));
    }

    /// Shows a failure in the footer until the next key press.
    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(Notice {
            text: notice.into(),
            is_error: true,
        });
    }

    /// Shows neutral feedback in the footer until the next key press.
    pub fn set_info(&mut self, info: impl Into<String>) {
        self.notice = Some(Notice {
            text: info.into(),
            is_error: false,
        });
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|notice| notice.text.as_str())
    }

    pub fn notice_is_error(&self) -> bool {
        self.notice.as_ref().is_some_and(|notice| notice.is_error)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        self.handle_key_at(key, Instant::now())
    }

    pub fn handle_key_at(&mut self, key: KeyEvent, now: Instant) -> Action {
        self.notice = None;
        if let Some(popup) = self.popup {
            return self.handle_popup_key(popup, key);
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Char('r') => self.request_refresh(now),
            KeyCode::Char(digit @ '1'..='9') => {
                let board = digit as usize - '1' as usize;
                if board < self.snapshot.boards.len() {
                    self.switch_board(board);
                }
                Action::None
            }
            KeyCode::Tab => {
                self.cycle_board(1);
                Action::None
            }
            KeyCode::BackTab => {
                self.cycle_board(-1);
                Action::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.move_column(1);
                Action::None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.move_column(-1);
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_card(1);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_card(-1);
                Action::None
            }
            KeyCode::PageDown => {
                self.move_card(self.card_page());
                Action::None
            }
            KeyCode::PageUp => {
                self.move_card(-self.card_page());
                Action::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.card = 0;
                Action::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.move_card(isize::MAX);
                Action::None
            }
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char(' ' | 'd') => {
                if self.selected_card().is_some() {
                    self.open_popup(Popup::Card);
                }
                Action::None
            }
            KeyCode::Char('s') => {
                self.open_popup(Popup::Sources);
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Keys while a popup is open: they scroll or close it, and never quit
    /// except for Ctrl-C.
    fn handle_popup_key(&mut self, popup: Popup, key: KeyEvent) -> Action {
        let page = i32::from(self.viewport.popup_page.max(1));
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::Quit;
            }
            KeyCode::Char('q') | KeyCode::Esc => self.popup = None,
            KeyCode::Char(' ' | 'd') if popup == Popup::Card => self.popup = None,
            KeyCode::Char('s') if popup == Popup::Sources => self.popup = None,
            KeyCode::Enter if popup == Popup::Card => return self.open_selected(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_popup(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_popup(-1),
            KeyCode::PageDown => self.scroll_popup(page),
            KeyCode::PageUp => self.scroll_popup(-page),
            KeyCode::Home | KeyCode::Char('g') => self.popup_scroll = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.popup_scroll = self.viewport.popup_max_scroll;
            }
            _ => {}
        }
        Action::None
    }

    fn open_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
        self.popup_scroll = 0;
        // Unknown until the popup is drawn.
        self.viewport.popup_max_scroll = 0;
    }

    /// Scrolls the open popup by `delta` lines, within its text.
    pub fn scroll_popup(&mut self, delta: i32) {
        let scroll = (i32::from(self.popup_scroll) + delta)
            .clamp(0, i32::from(self.viewport.popup_max_scroll));
        self.popup_scroll = scroll as u16;
    }

    /// The wheel scrolls an open popup, or else the column under the pointer
    /// (focusing it first) or the focused one; a left click selects the card
    /// under it, or focuses the column.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let delta = match mouse.kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::Down(MouseButton::Left) if self.popup.is_none() => {
                self.click(mouse.column, mouse.row);
                return;
            }
            _ => return,
        };
        self.notice = None;
        if self.popup.is_some() {
            self.scroll_popup(delta * WHEEL_LINES);
            return;
        }
        match self.column_at(mouse.column, mouse.row) {
            Some(column) if column.index != self.column => self.focus_column(column.index),
            _ => self.move_card(delta as isize),
        }
    }

    fn click(&mut self, x: u16, y: u16) {
        let Some(column) = self.column_at(x, y) else {
            return;
        };
        self.notice = None;
        self.focus_column(column.index);
        let row = y - column.area.y;
        if row == 0 {
            return;
        }
        let card = column.offset + usize::from((row - 1) / CARD_ROWS);
        if card < self.columns()[column.index].column.cards.len() {
            self.card = card;
        }
    }

    fn column_at(&self, x: u16, y: u16) -> Option<ColumnArea> {
        self.viewport
            .columns
            .iter()
            .find(|column| column.area.contains(Position::new(x, y)))
            .filter(|column| column.index < self.columns().len())
            .cloned()
    }

    fn focus_column(&mut self, column: usize) {
        if column != self.column {
            self.column = column;
            self.card = 0;
        }
    }

    /// Cards PageUp/PageDown move by: as many as fit in a column.
    fn card_page(&self) -> isize {
        self.viewport.card_page.max(1) as isize
    }

    fn open_selected(&self) -> Action {
        self.selected_card()
            .and_then(|card| card.url.clone())
            .map_or(Action::None, Action::Open)
    }

    fn switch_board(&mut self, board: usize) {
        if board != self.board {
            self.board = board;
            self.column = 0;
            self.card = 0;
        }
    }

    fn cycle_board(&mut self, delta: isize) {
        let count = self.snapshot.boards.len();
        if count > 0 {
            let next = (self.board as isize + delta).rem_euclid(count as isize) as usize;
            self.switch_board(next);
        }
    }

    fn move_column(&mut self, delta: isize) {
        let count = self.columns().len();
        if count > 0 {
            self.column = self.column.saturating_add_signed(delta).min(count - 1);
            self.card = 0;
        }
    }

    fn move_card(&mut self, delta: isize) {
        let count = self
            .columns()
            .get(self.column)
            .map_or(0, |c| c.column.cards.len());
        if count > 0 {
            self.card = self.card.saturating_add_signed(delta).min(count - 1);
        }
    }

    fn request_refresh(&mut self, now: Instant) -> Action {
        if let Some(last) = self.last_refresh {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < REFRESH_COOLDOWN {
                let wait = (REFRESH_COOLDOWN - elapsed).as_secs().max(1);
                self.set_info(format!("refresh already requested; wait {wait}s"));
                return Action::None;
            }
        }
        self.last_refresh = Some(now);
        self.set_info("refreshing all sources…");
        Action::RefreshNow
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pending_core::{
        CardSeverity, DashboardSnapshot, PendingCard, SourceBatch, SourceItem, SourceOutcome,
        SourceReport, build_snapshot,
    };

    use super::*;

    fn card(column: &str, id: &str, url: Option<&str>) -> SourceItem {
        SourceItem {
            column: column.to_owned(),
            card: PendingCard {
                id: id.to_owned(),
                title: id.to_owned(),
                body: String::new(),
                source: "test".to_owned(),
                url: url.map(str::to_owned),
                due_at: None,
                severity: CardSeverity::Info,
                updated_at: Utc.with_ymd_and_hms(2026, 9, 28, 10, 0, 0).unwrap(),
            },
        }
    }

    fn report(name: &str, board: &str, columns: &[&str], items: Vec<SourceItem>) -> SourceReport {
        let at = Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        SourceReport {
            name: name.to_owned(),
            board: board.to_owned(),
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            icon: None,
            outcome: SourceOutcome::Fresh {
                batch: SourceBatch {
                    items,
                    warnings: Vec::new(),
                },
                refreshed_at: at,
            },
        }
    }

    fn snapshot(reports: Vec<SourceReport>) -> DashboardSnapshot {
        build_snapshot(
            Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap(),
            reports,
        )
    }

    /// Two boards: Work (plane with two columns + github) and Personal.
    fn sample() -> DashboardSnapshot {
        snapshot(vec![
            report(
                "plane",
                "Work",
                &["Mine", "Inbox"],
                vec![
                    card("Mine", "p1", None),
                    card("Mine", "p2", None),
                    card("Inbox", "p3", None),
                ],
            ),
            report(
                "github",
                "Work",
                &["Review"],
                vec![card("Review", "g1", Some("https://github.com/o/r/pull/1"))],
            ),
            report(
                "todoist",
                "Personal",
                &["Today"],
                vec![card("Today", "t1", None)],
            ),
        ])
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn selected(app: &App) -> Option<&str> {
        app.selected_card().map(|c| c.id.as_str())
    }

    /// h/l move across the board's columns, crossing from one tool to the
    /// next; j/k move within a column; the ends don't wrap.
    #[test]
    fn keys_move_across_columns_and_within_a_column() {
        let mut app = App::new(sample());
        assert_eq!(selected(&app), Some("p1"));

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"), "stops at the end");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(selected(&app), Some("p3"), "next column starts at the top");
        app.handle_key(key(KeyCode::Right));
        assert_eq!(selected(&app), Some("g1"), "crosses into the next source");
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(selected(&app), Some("g1"), "stops at the last column");

        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Left));
        assert_eq!(selected(&app), Some("p1"));
    }

    /// Digits and Tab switch boards (tabs); the selection starts at the first column.
    #[test]
    fn digits_and_tab_switch_boards() {
        let mut app = App::new(sample());

        app.handle_key(key(KeyCode::Char('2')));
        assert_eq!(app.current_board().unwrap().name, "Personal");
        assert_eq!(selected(&app), Some("t1"));

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(
            app.current_board().unwrap().name,
            "Work",
            "tab wraps around"
        );
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.current_board().unwrap().name, "Personal");

        app.handle_key(key(KeyCode::Char('9')));
        assert_eq!(
            app.current_board().unwrap().name,
            "Personal",
            "missing board ignored"
        );
    }

    /// When the snapshot updates, the selection stays on the same board, column
    /// and card; if the card is gone, it keeps the same position in the column.
    #[test]
    fn selection_follows_board_column_and_card_across_refreshes() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"));

        // A new card above p2 in the same column.
        app.update(snapshot(vec![
            report(
                "plane",
                "Work",
                &["Mine", "Inbox"],
                vec![
                    card("Mine", "p0", None),
                    card("Mine", "p1", None),
                    card("Mine", "p2", None),
                ],
            ),
            report("todoist", "Personal", &["Today"], vec![]),
        ]));
        assert_eq!(selected(&app), Some("p2"));

        // p2 is gone: stay at the same position.
        app.update(snapshot(vec![report(
            "plane",
            "Work",
            &["Mine", "Inbox"],
            vec![card("Mine", "p0", None)],
        )]));
        assert_eq!(selected(&app), Some("p0"));

        app.update(snapshot(Vec::new()));
        assert_eq!(selected(&app), None);
    }

    /// Enter opens the selected card's link; a card without a link does nothing.
    #[test]
    fn enter_opens_the_selected_card_link() {
        let mut app = App::new(sample());
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Action::None);

        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Open("https://github.com/o/r/pull/1".to_owned())
        );
    }

    /// q, Esc and Ctrl-C quit.
    #[test]
    fn quit_keys() {
        let mut app = App::new(sample());

        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::Quit);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
        assert_eq!(app.handle_key(key(KeyCode::Char('c'))), Action::None);
    }

    /// Space (or d) opens the selected card's details; while they are open,
    /// q and Esc close them instead of quitting, and Enter still opens the
    /// link. Without a selected card nothing opens.
    #[test]
    fn space_opens_card_details_and_esc_or_q_close_them() {
        let mut app = App::new(sample());

        assert_eq!(app.handle_key(key(KeyCode::Char(' '))), Action::None);
        assert_eq!(app.popup(), Some(Popup::Card));
        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::None);
        assert_eq!(app.popup(), None);

        app.handle_key(key(KeyCode::Char('d')));
        assert_eq!(app.popup(), Some(Popup::Card));
        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::None);
        assert_eq!(app.popup(), None);

        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Open("https://github.com/o/r/pull/1".to_owned())
        );
        assert_eq!(app.popup(), Some(Popup::Card), "stays open after Enter");
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.popup(), None, "Space toggles");

        // A refresh leaves the board without cards.
        app.update(snapshot(vec![report("plane", "Work", &["Mine"], vec![])]));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.popup(), None, "no card, no details");
    }

    /// While details are open, j/k, arrows, PageUp/PageDown and Home/End
    /// scroll them, within what the screen last reported as scrollable; the
    /// selection stays put.
    #[test]
    fn details_scroll_within_the_rendered_bounds() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char(' ')));
        app.set_viewport(Viewport {
            popup_page: 5,
            popup_max_scroll: 12,
            ..Viewport::default()
        });

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.popup_scroll(), 1);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.popup_scroll(), 6);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.popup_scroll(), 11);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.popup_scroll(), 12, "stops at the end");
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.popup_scroll(), 7);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.popup_scroll(), 6);
        app.handle_key(key(KeyCode::Home));
        assert_eq!(app.popup_scroll(), 0);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.popup_scroll(), 0);
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.popup_scroll(), 12);
        assert_eq!(selected(&app), Some("p1"), "selection unchanged");

        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.popup_scroll(), 0, "reopening starts at the top");
    }

    /// s opens the sources' details (full status messages); s, q or Esc
    /// close them.
    #[test]
    fn s_opens_source_details() {
        let mut app = App::new(sample());

        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.popup(), Some(Popup::Sources));
        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.popup(), None);

        app.handle_key(key(KeyCode::Char('s')));
        app.handle_key(key(KeyCode::Char('q')));
        assert_eq!(app.popup(), None);
    }

    /// Card details close when a refresh removes every card of the column.
    #[test]
    fn card_details_close_when_the_card_is_gone() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char(' ')));

        app.update(snapshot(Vec::new()));

        assert_eq!(app.popup(), None);
    }

    /// Plane's Mine column with cards c0..c29, and a second, empty column.
    fn long_column() -> App {
        let items = (0..30)
            .map(|i| card("Mine", &format!("c{i}"), None))
            .collect();
        App::new(snapshot(vec![report(
            "plane",
            "Work",
            &["Mine", "Inbox"],
            items,
        )]))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Three columns side by side, 20 cells wide, from row 10: Mine, Inbox
    /// and Review of the sample Work board.
    fn three_columns() -> Viewport {
        Viewport {
            columns: (0..3)
                .map(|index| ColumnArea {
                    index,
                    area: Rect::new(20 * index as u16, 10, 20, 10),
                    offset: 0,
                })
                .collect(),
            card_page: 4,
            ..Viewport::default()
        }
    }

    /// PageUp/PageDown move the selection by the cards that fit in a column;
    /// Home/End (or g/G) jump to the first and last card.
    #[test]
    fn page_keys_move_by_a_page_and_home_end_jump() {
        let mut app = long_column();
        app.set_viewport(Viewport {
            card_page: 5,
            ..Viewport::default()
        });

        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected(&app), Some("c5"));
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected(&app), Some("c10"));
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(selected(&app), Some("c5"));
        app.handle_key(key(KeyCode::End));
        assert_eq!(selected(&app), Some("c29"));
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected(&app), Some("c29"), "stops at the end");
        app.handle_key(key(KeyCode::Home));
        assert_eq!(selected(&app), Some("c0"));
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(selected(&app), Some("c0"), "stops at the top");
        app.handle_key(key(KeyCode::Char('G')));
        assert_eq!(selected(&app), Some("c29"));
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(selected(&app), Some("c0"));
    }

    /// The mouse wheel scrolls the column under the pointer, focusing it
    /// first; away from the columns it scrolls the focused one.
    #[test]
    fn mouse_wheel_scrolls_the_column_under_the_pointer() {
        let mut app = App::new(sample());
        app.set_viewport(three_columns());

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(selected(&app), Some("p2"), "stops at the end");

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 45, 12));
        assert_eq!(app.selected_column(), 2, "focuses the column under it");
        assert_eq!(selected(&app), Some("g1"));

        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 12));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 100, 0));
        assert_eq!(
            selected(&app),
            Some("p2"),
            "outside any column: focused one"
        );
    }

    /// Clicking a card selects it; clicking elsewhere in a column focuses
    /// that column.
    #[test]
    fn clicking_a_card_selects_it() {
        let mut app = App::new(sample());
        app.set_viewport(three_columns());
        let click = |column, row| mouse(MouseEventKind::Down(MouseButton::Left), column, row);

        // Row 10 is the column title; each card takes two rows from row 11.
        app.handle_mouse(click(3, 13));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_mouse(click(3, 11));
        assert_eq!(selected(&app), Some("p1"));

        app.handle_mouse(click(45, 10));
        assert_eq!(selected(&app), Some("g1"), "title focuses the column");
        app.handle_mouse(click(25, 12));
        assert_eq!(selected(&app), Some("p3"), "a card's second row");
        app.handle_mouse(click(3, 18));
        assert_eq!(app.selected_column(), 0, "below the cards: focus only");
        assert_eq!(selected(&app), Some("p1"));
    }

    /// While details are open, the wheel scrolls them and clicks do nothing.
    #[test]
    fn mouse_wheel_scrolls_open_details() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char(' ')));
        app.set_viewport(Viewport {
            popup_page: 5,
            popup_max_scroll: 10,
            ..three_columns()
        });

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 45, 12));
        assert_eq!(app.popup_scroll(), 3);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 45, 12));
        assert_eq!(app.popup_scroll(), 0);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 45, 11));
        assert_eq!(selected(&app), Some("p1"));
        assert_eq!(app.popup(), Some(Popup::Card));
    }

    /// r requests a refresh and says so in the footer; pressing it again right
    /// away doesn't start another round, and says how long to wait.
    #[test]
    fn refresh_is_acknowledged_and_debounced() {
        let mut app = App::new(sample());
        let t0 = std::time::Instant::now();
        let r = key(KeyCode::Char('r'));

        assert_eq!(app.handle_key_at(r, t0), Action::RefreshNow);
        assert!(app.notice().unwrap_or("").contains("refreshing"));

        let soon = t0 + std::time::Duration::from_secs(3);
        assert_eq!(app.handle_key_at(r, soon), Action::None);
        assert!(app.notice().unwrap_or("").contains("wait"));

        assert_eq!(
            app.handle_key_at(r, t0 + REFRESH_COOLDOWN),
            Action::RefreshNow
        );
    }
}
