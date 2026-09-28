//! Dashboard state and keyboard handling, independent of the terminal.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pending_core::{DashboardSnapshot, PendingCard};

/// Minimum wait between manual refreshes. Each one queries every source, and
/// provider APIs rate-limit (GitHub search: 30 requests per minute).
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

pub struct App {
    snapshot: DashboardSnapshot,
    /// Index into `cards()`; `None` when there are no cards.
    selected: Option<usize>,
    /// Last action feedback, shown until the next key press.
    notice: Option<Notice>,
    last_refresh: Option<Instant>,
}

impl App {
    pub fn new(snapshot: DashboardSnapshot) -> Self {
        let selected = (card_count(&snapshot) > 0).then_some(0);
        Self {
            snapshot,
            selected,
            notice: None,
            last_refresh: None,
        }
    }

    pub fn snapshot(&self) -> &DashboardSnapshot {
        &self.snapshot
    }

    /// Replaces the snapshot, keeping the same card selected when it still
    /// exists, or the same position otherwise.
    pub fn update(&mut self, snapshot: DashboardSnapshot) {
        let previous_id = self.selected_card().map(|card| card.id.clone());
        let previous_index = self.selected;
        self.snapshot = snapshot;

        let count = card_count(&self.snapshot);
        self.selected = if count == 0 {
            None
        } else {
            previous_id
                .and_then(|id| self.cards().position(|card| card.id == id))
                .or(previous_index.map(|index| index.min(count - 1)))
                .or(Some(0))
        };
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

    pub fn selected_card(&self) -> Option<&PendingCard> {
        self.selected.and_then(|index| self.cards().nth(index))
    }

    pub fn is_selected(&self, card: &PendingCard) -> bool {
        self.selected_card()
            .is_some_and(|selected| selected.id == card.id)
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
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                Action::None
            }
            KeyCode::Enter => self
                .selected_card()
                .and_then(|card| card.url.clone())
                .map_or(Action::None, Action::Open),
            _ => Action::None,
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

    fn move_selection(&mut self, delta: isize) {
        let count = card_count(&self.snapshot);
        if let Some(index) = self.selected {
            self.selected = Some(index.saturating_add_signed(delta).min(count - 1));
        }
    }

    /// Cards in display order: lanes, then sections, then cards.
    fn cards(&self) -> impl Iterator<Item = &PendingCard> {
        self.snapshot
            .lanes
            .iter()
            .flat_map(|lane| &lane.sections)
            .flat_map(|section| &section.cards)
    }
}

fn card_count(snapshot: &DashboardSnapshot) -> usize {
    snapshot
        .lanes
        .iter()
        .flat_map(|lane| &lane.sections)
        .map(|section| section.cards.len())
        .sum()
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

    fn card(id: &str, url: Option<&str>) -> SourceItem {
        SourceItem {
            section: "Review".to_owned(),
            card: PendingCard {
                id: id.to_owned(),
                title: id.to_owned(),
                body: String::new(),
                source: "test".to_owned(),
                url: url.map(str::to_owned),
                severity: CardSeverity::Info,
                due_at: None,
                updated_at: Utc.with_ymd_and_hms(2026, 9, 28, 10, 0, 0).unwrap(),
            },
        }
    }

    fn snapshot(items: Vec<SourceItem>) -> DashboardSnapshot {
        let at = Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap();
        build_snapshot(
            at,
            vec![SourceReport {
                name: "test".to_owned(),
                lane: "Work".to_owned(),
                outcome: SourceOutcome::Fresh {
                    batch: SourceBatch {
                        items,
                        warnings: Vec::new(),
                    },
                    refreshed_at: at,
                },
            }],
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn selected(app: &App) -> Option<&str> {
        app.selected_card().map(|c| c.id.as_str())
    }

    /// A seleção começa no primeiro card e anda com j/k ou setas, parando nas
    /// pontas em vez de dar a volta.
    #[test]
    fn selection_moves_through_cards_and_stops_at_the_ends() {
        let mut app = App::new(snapshot(vec![
            card("a", None),
            card("b", None),
            card("c", None),
        ]));
        assert_eq!(selected(&app), Some("a"));

        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(selected(&app), Some("c"));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("c"));

        app.handle_key(key(KeyCode::Char('k')));
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(selected(&app), Some("a"));
    }

    /// Quando o snapshot atualiza, a seleção continua no mesmo card; se ele
    /// sumiu, fica na mesma posição dentro do que restou.
    #[test]
    fn selection_follows_the_card_across_refreshes() {
        let mut app = App::new(snapshot(vec![
            card("a", None),
            card("b", None),
            card("c", None),
        ]));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("b"));

        app.update(snapshot(vec![
            card("new", None),
            card("a", None),
            card("b", None),
        ]));
        assert_eq!(selected(&app), Some("b"));

        app.update(snapshot(vec![card("x", None)]));
        assert_eq!(selected(&app), Some("x"));

        app.update(snapshot(Vec::new()));
        assert_eq!(selected(&app), None);
    }

    /// Enter abre o link do card selecionado; card sem link não faz nada.
    #[test]
    fn enter_opens_the_selected_card_link() {
        let mut app = App::new(snapshot(vec![
            card("linked", Some("https://github.com/o/r/pull/1")),
            card("plain", None),
        ]));

        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            Action::Open("https://github.com/o/r/pull/1".to_owned())
        );
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Action::None);
    }

    /// q, Esc e Ctrl-C saem; r pede atualização imediata.
    #[test]
    fn quit_and_refresh_keys() {
        let mut app = App::new(snapshot(Vec::new()));

        assert_eq!(app.handle_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::Quit);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
        assert_eq!(app.handle_key(key(KeyCode::Char('r'))), Action::RefreshNow);
        assert_eq!(app.handle_key(key(KeyCode::Char('c'))), Action::None);
    }

    /// r pede atualização e avisa no rodapé; apertar de novo logo em seguida
    /// não dispara outra rodada de consultas, para não estourar limites de
    /// taxa das APIs, e diz quanto falta esperar.
    #[test]
    fn refresh_is_acknowledged_and_debounced() {
        let mut app = App::new(snapshot(Vec::new()));
        let t0 = std::time::Instant::now();
        let r = key(KeyCode::Char('r'));

        assert_eq!(app.handle_key_at(r, t0), Action::RefreshNow);
        assert!(
            app.notice().unwrap_or("").contains("refreshing"),
            "{:?}",
            app.notice()
        );

        let soon = t0 + std::time::Duration::from_secs(3);
        assert_eq!(app.handle_key_at(r, soon), Action::None);
        assert!(
            app.notice().unwrap_or("").contains("wait"),
            "{:?}",
            app.notice()
        );

        let later = t0 + REFRESH_COOLDOWN;
        assert_eq!(app.handle_key_at(r, later), Action::RefreshNow);
    }
}
