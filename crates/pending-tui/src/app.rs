//! Dashboard state and keyboard handling, independent of the terminal.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pending_core::{Board, Column, DashboardSnapshot, PendingCard};

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
        }
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
            KeyCode::Enter => self
                .selected_card()
                .and_then(|card| card.url.clone())
                .map_or(Action::None, Action::Open),
            _ => Action::None,
        }
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

    /// Duas áreas: Work (plane com duas colunas + github) e Personal.
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

    /// h/l andam entre as colunas da área, passando de uma ferramenta para a
    /// outra; j/k andam dentro da coluna; as pontas não dão a volta.
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

    /// Números e Tab trocam de área (aba); a seleção começa na primeira coluna.
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

    /// Quando o snapshot atualiza, a seleção continua na mesma área, coluna e
    /// card; se o card sumiu, fica na mesma posição dentro da coluna.
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

    /// Enter abre o link do card selecionado; card sem link não faz nada.
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

    /// q, Esc e Ctrl-C saem.
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

    /// r pede atualização e avisa no rodapé; apertar de novo logo em seguida
    /// não dispara outra rodada, e diz quanto falta esperar.
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
