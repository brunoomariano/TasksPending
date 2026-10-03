//! Dashboard state and keyboard handling, independent of the terminal.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use pending_core::{Board, Column, DashboardSnapshot, Group, PendingCard, SnoozedCard};
use ratatui::layout::{Position, Rect};

use crate::snooze::{CHOICES, Choice};

/// Minimum wait between manual refreshes. Each one queries every source, and
/// provider APIs rate-limit (GitHub search: 30 requests per minute, and every
/// GitHub stack is one search).
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
    /// Mark the card `id` as in progress, or unmark it.
    SetMark {
        id: String,
        marked: bool,
    },
    /// Hide the card `id` for as long as `choice` says.
    Snooze {
        id: String,
        choice: Choice,
    },
    /// Bring the snoozed card `id` back now.
    Wake {
        id: String,
    },
}

/// One screen row of a group as drawn, before scrolling: the group's stacks
/// one above another, each a header row followed by its cards unless it is
/// collapsed or empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// A stack's header: its name and card count.
    Header { stack: usize },
    /// One of the `CARD_ROWS` rows of a card.
    Card { stack: usize, card: usize },
}

/// A popup over the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    /// The selected card, in full.
    Card,
    /// Every source's status and full message.
    Sources,
    /// The menu of how long to snooze the selected card.
    Snooze,
    /// The snoozed cards, to wake one.
    Snoozed,
}

/// Screen geometry the last render reported, which keys and the mouse need:
/// where the groups are, how many cards fit, how far the popup scrolls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewport {
    /// The groups on screen.
    pub groups: Vec<GroupArea>,
    /// Cards that fit in a group at once.
    pub card_page: usize,
    /// Popup lines that fit on screen at once.
    pub popup_page: u16,
    /// The furthest the popup scrolls, in lines.
    pub popup_max_scroll: u16,
}

/// A group as drawn: `App::rows` from `scroll` on, one per screen row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupArea {
    /// Index into `App::groups()`.
    pub index: usize,
    /// Where the rows are, inside the group's border.
    pub area: Rect,
    /// The first row shown (the group scrolls to keep the selection visible).
    pub scroll: usize,
}

/// Rows a card takes in a stack: title and body.
pub const CARD_ROWS: u16 = 2;
/// Popup lines one mouse wheel step scrolls.
const WHEEL_LINES: i32 = 3;

/// Where the selection is, by name, so it survives refreshes.
struct Anchor {
    board: String,
    source: String,
    stack: String,
    card: Option<String>,
}

/// A stack by name (board, source, stack), so collapsing it survives
/// refreshes and config reorders.
type StackKey = (String, String, String);

pub struct App {
    snapshot: DashboardSnapshot,
    board: usize,
    /// Index into `groups()` of the current board.
    group: usize,
    /// Index into the selected group's stacks.
    stack: usize,
    /// Index into the selected stack's cards. While that stack is collapsed
    /// the selection rests on its header, and this is the card it returns to.
    card: usize,
    collapsed: HashSet<StackKey>,
    /// Last action feedback, shown until the next key press.
    notice: Option<Notice>,
    last_refresh: Option<Instant>,
    popup: Option<Popup>,
    /// First popup line shown.
    popup_scroll: u16,
    /// The selected line of the snooze menu or of the snoozed list.
    popup_selected: usize,
    viewport: Viewport,
}

impl App {
    pub fn new(snapshot: DashboardSnapshot) -> Self {
        let mut app = Self {
            snapshot,
            board: 0,
            group: 0,
            stack: 0,
            card: 0,
            collapsed: HashSet::new(),
            notice: None,
            last_refresh: None,
            popup: None,
            popup_scroll: 0,
            popup_selected: 0,
            viewport: Viewport::default(),
        };
        app.select_first();
        app
    }

    pub fn popup(&self) -> Option<Popup> {
        self.popup
    }

    pub fn popup_scroll(&self) -> u16 {
        self.popup_scroll
    }

    /// The selected line of the snooze menu or of the snoozed list.
    pub fn popup_selected(&self) -> usize {
        self.popup_selected
    }

    /// The cards the user snoozed, as the dashboard lists them.
    pub fn snoozed(&self) -> &[SnoozedCard] {
        &self.snapshot.snoozed
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

    /// Groups (sources) of the current board, in config order.
    pub fn groups(&self) -> &[Group] {
        self.current_board().map_or(&[], |board| &board.groups)
    }

    pub fn selected_group(&self) -> usize {
        self.group
    }

    fn stack_at(&self, group: usize, stack: usize) -> Option<&Column> {
        self.groups().get(group)?.columns.get(stack)
    }

    /// The group and stack the selection is in.
    pub fn selected_stack(&self) -> Option<(&Group, &Column)> {
        let group = self.groups().get(self.group)?;
        Some((group, group.columns.get(self.stack)?))
    }

    fn stack_key(&self, group: usize, stack: usize) -> Option<StackKey> {
        let board = self.current_board()?;
        let group = board.groups.get(group)?;
        let stack = group.columns.get(stack)?;
        Some((board.name.clone(), group.source.clone(), stack.name.clone()))
    }

    /// Whether stack `stack` of group `group` shows only its header. Empty
    /// stacks have nothing to collapse.
    pub fn is_collapsed(&self, group: usize, stack: usize) -> bool {
        self.stack_at(group, stack)
            .is_some_and(|column| !column.cards.is_empty())
            && self
                .stack_key(group, stack)
                .is_some_and(|key| self.collapsed.contains(&key))
    }

    /// The screen rows of group `group`, top to bottom.
    pub fn rows(&self, group: usize) -> Vec<Row> {
        let Some(found) = self.groups().get(group) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (stack, column) in found.columns.iter().enumerate() {
            rows.push(Row::Header { stack });
            if !self.is_collapsed(group, stack) {
                for card in 0..column.cards.len() {
                    rows.extend([Row::Card { stack, card }; CARD_ROWS as usize]);
                }
            }
        }
        rows
    }

    /// Where the selection can rest in group `group`, top to bottom: every
    /// card of an open stack, and the header of a collapsed one.
    fn stops(&self, group: usize) -> Vec<Row> {
        let mut stops = self.rows(group);
        stops.dedup();
        stops.retain(|row| match *row {
            Row::Header { stack } => self.is_collapsed(group, stack),
            Row::Card { .. } => true,
        });
        stops
    }

    /// What the selection rests on in the selected group: a card, or the
    /// header of a collapsed stack. `None` when the group has no cards.
    pub fn selected_row(&self) -> Option<Row> {
        let column = self.stack_at(self.group, self.stack)?;
        if self.is_collapsed(self.group, self.stack) {
            Some(Row::Header { stack: self.stack })
        } else if self.card < column.cards.len() {
            Some(Row::Card {
                stack: self.stack,
                card: self.card,
            })
        } else {
            None
        }
    }

    pub fn selected_card(&self) -> Option<&PendingCard> {
        match self.selected_row()? {
            Row::Card { stack, card } => self.stack_at(self.group, stack)?.cards.get(card),
            Row::Header { .. } => None,
        }
    }

    /// Whether the card `id` is marked as in progress.
    pub fn is_marked(&self, id: &str) -> bool {
        self.snapshot.marked.iter().any(|marked| marked == id)
    }

    /// The marked cards that are on the dashboard, on any board, in marking
    /// order, each with its source.
    pub fn marked_cards(&self) -> Vec<(&PendingCard, &str)> {
        self.snapshot
            .marked
            .iter()
            .filter_map(|id| {
                self.snapshot
                    .boards
                    .iter()
                    .flat_map(|board| &board.groups)
                    .find_map(|group| {
                        group
                            .columns
                            .iter()
                            .flat_map(|stack| &stack.cards)
                            .find(|card| card.id == *id)
                            .map(|card| (card, group.source.as_str()))
                    })
            })
            .collect()
    }

    /// Asks to mark the selected card, or to unmark it when it is marked.
    fn toggle_mark(&self) -> Action {
        self.selected_card()
            .map_or(Action::None, |card| Action::SetMark {
                id: card.id.clone(),
                marked: !self.is_marked(&card.id),
            })
    }

    fn select(&mut self, row: Row) {
        match row {
            Row::Header { stack } => {
                if stack != self.stack {
                    self.stack = stack;
                    self.card = 0;
                }
            }
            Row::Card { stack, card } => {
                self.stack = stack;
                self.card = card;
            }
        }
    }

    /// Moves the selection to the top of the selected group.
    fn select_first(&mut self) {
        self.stack = 0;
        self.card = 0;
        if let Some(first) = self.stops(self.group).first() {
            self.select(*first);
        }
    }

    /// Replaces the snapshot, keeping the selection on the same board, group,
    /// stack and card when they still exist, or the nearest position otherwise.
    pub fn update(&mut self, snapshot: DashboardSnapshot) {
        let anchor = self.anchor();
        self.snapshot = snapshot;
        if let Some(anchor) = anchor {
            self.restore(anchor);
        }
        self.clamp();
        self.close_card_popup_without_card();
    }

    fn restore(&mut self, anchor: Anchor) {
        if let Some(board) = self
            .snapshot
            .boards
            .iter()
            .position(|b| b.name == anchor.board)
        {
            self.board = board;
        }
        let Some(group) = self.groups().iter().position(|g| g.source == anchor.source) else {
            return;
        };
        self.group = group;
        let stacks = &self.groups()[group].columns;
        let Some(stack) = stacks.iter().position(|c| c.name == anchor.stack) else {
            return;
        };
        let card = anchor
            .card
            .and_then(|id| stacks[stack].cards.iter().position(|c| c.id == id));
        self.stack = stack;
        if let Some(card) = card {
            self.card = card;
        }
    }

    /// Closes the popups about the selected card when there is none, and
    /// keeps the snoozed list's selection inside the list.
    fn close_card_popup_without_card(&mut self) {
        match self.popup {
            Some(Popup::Card | Popup::Snooze) if self.selected_card().is_none() => {
                self.popup = None;
            }
            Some(Popup::Snoozed) => self.move_popup_selection(0),
            _ => {}
        }
    }

    fn anchor(&self) -> Option<Anchor> {
        let board = self.current_board()?;
        let (group, stack) = self.selected_stack()?;
        Some(Anchor {
            board: board.name.clone(),
            source: group.source.clone(),
            stack: stack.name.clone(),
            card: stack.cards.get(self.card).map(|c| c.id.clone()),
        })
    }

    /// Brings the selection back inside the snapshot: the same position in
    /// its stack, or else the nearest card or collapsed header after it (or
    /// before it, at the end of the group).
    fn clamp(&mut self) {
        self.board = self.board.min(self.snapshot.boards.len().saturating_sub(1));
        self.group = self.group.min(self.groups().len().saturating_sub(1));
        let stacks = self.groups().get(self.group).map_or(0, |g| g.columns.len());
        self.stack = self.stack.min(stacks.saturating_sub(1));
        let cards = self
            .stack_at(self.group, self.stack)
            .map_or(0, |column| column.cards.len());
        self.card = self.card.min(cards.saturating_sub(1));

        if self.selected_row().is_none() {
            let stops = self.stops(self.group);
            let stack_of = |row: &Row| match *row {
                Row::Header { stack } | Row::Card { stack, .. } => stack,
            };
            let nearest = stops
                .iter()
                .find(|row| stack_of(row) >= self.stack)
                .or(stops.last());
            if let Some(row) = nearest.copied() {
                self.select(row);
            }
        }
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
                self.move_group(1);
                Action::None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.move_group(-1);
                Action::None
            }
            KeyCode::Char('c') => {
                self.toggle_stack(self.stack);
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
                self.move_card(isize::MIN);
                Action::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.move_card(isize::MAX);
                Action::None
            }
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('m') => self.toggle_mark(),
            KeyCode::Char('z') => {
                if self.selected_card().is_some() {
                    self.open_popup(Popup::Snooze);
                }
                Action::None
            }
            KeyCode::Char('Z') => {
                self.open_popup(Popup::Snoozed);
                Action::None
            }
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
            code if matches!(popup, Popup::Snooze | Popup::Snoozed) => {
                return self.handle_list_key(popup, code);
            }
            KeyCode::Char(' ' | 'd') if popup == Popup::Card => self.popup = None,
            KeyCode::Char('s') if popup == Popup::Sources => self.popup = None,
            KeyCode::Enter if popup == Popup::Card => return self.open_selected(),
            KeyCode::Char('m') if popup == Popup::Card => return self.toggle_mark(),
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

    /// Keys of the snooze menu and of the snoozed list: both are a list
    /// with one line selected. In the menu a number key chooses right away
    /// and Enter chooses the selected line; in the list `w` or Enter wakes
    /// the selected card. The key that opened the popup closes it.
    fn handle_list_key(&mut self, popup: Popup, code: KeyCode) -> Action {
        let all = isize::try_from(self.popup_lines()).unwrap_or(isize::MAX);
        match (popup, code) {
            (Popup::Snooze, KeyCode::Char('z')) | (Popup::Snoozed, KeyCode::Char('Z')) => {
                self.popup = None;
            }
            (_, KeyCode::Char('j') | KeyCode::Down) => self.move_popup_selection(1),
            (_, KeyCode::Char('k') | KeyCode::Up) => self.move_popup_selection(-1),
            (_, KeyCode::Home | KeyCode::Char('g')) => self.move_popup_selection(-all),
            (_, KeyCode::End | KeyCode::Char('G')) => self.move_popup_selection(all),
            (Popup::Snooze, KeyCode::Char(digit @ '1'..='9')) => {
                return self.choose_snooze(digit as usize - '1' as usize);
            }
            (Popup::Snooze, KeyCode::Enter) => return self.choose_snooze(self.popup_selected),
            (Popup::Snoozed, KeyCode::Enter | KeyCode::Char('w')) => {
                return self.snapshot.snoozed.get(self.popup_selected).map_or(
                    Action::None,
                    |snoozed| Action::Wake {
                        id: snoozed.card.id.clone(),
                    },
                );
            }
            _ => {}
        }
        Action::None
    }

    /// Lines the open list popup has to select from.
    fn popup_lines(&self) -> usize {
        match self.popup {
            Some(Popup::Snooze) => CHOICES.len(),
            Some(Popup::Snoozed) => self.snapshot.snoozed.len(),
            _ => 0,
        }
    }

    /// Moves the selection of the open list popup by `delta` lines, stopping
    /// at its ends.
    fn move_popup_selection(&mut self, delta: isize) {
        let last = self.popup_lines().saturating_sub(1);
        self.popup_selected = self.popup_selected.saturating_add_signed(delta).min(last);
    }

    /// Asks to snooze the selected card with the menu's choice `index`, and
    /// closes the menu; an index past the choices does nothing.
    fn choose_snooze(&mut self, index: usize) -> Action {
        let (Some((choice, _)), Some(card)) = (CHOICES.get(index), self.selected_card()) else {
            return Action::None;
        };
        let action = Action::Snooze {
            id: card.id.clone(),
            choice: *choice,
        };
        self.popup = None;
        action
    }

    fn open_popup(&mut self, popup: Popup) {
        self.popup = Some(popup);
        self.popup_scroll = 0;
        self.popup_selected = 0;
        // Unknown until the popup is drawn.
        self.viewport.popup_max_scroll = 0;
    }

    /// Scrolls the open popup by `delta` lines, within its text.
    pub fn scroll_popup(&mut self, delta: i32) {
        let scroll = (i32::from(self.popup_scroll) + delta)
            .clamp(0, i32::from(self.viewport.popup_max_scroll));
        self.popup_scroll = scroll as u16;
    }

    /// The wheel scrolls an open popup (or moves the selection of the snooze
    /// menu and the snoozed list), or else moves through the group under
    /// the pointer (focusing it first) or the focused one; a left click
    /// selects the card under it, collapses or expands the stack whose header
    /// is under it, or just focuses the group.
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
        match self.popup {
            Some(Popup::Snooze | Popup::Snoozed) => {
                self.move_popup_selection(delta as isize);
                return;
            }
            Some(Popup::Card | Popup::Sources) => {
                self.scroll_popup(delta * WHEEL_LINES);
                return;
            }
            None => {}
        }
        match self.group_at(mouse.column, mouse.row) {
            Some(group) if group.index != self.group => self.focus_group(group.index),
            _ => self.move_card(delta as isize),
        }
    }

    fn click(&mut self, x: u16, y: u16) {
        let Some(group) = self.group_at(x, y) else {
            return;
        };
        self.notice = None;
        self.focus_group(group.index);
        let row = group.scroll + usize::from(y - group.area.y);
        match self.rows(group.index).get(row).copied() {
            Some(Row::Header { stack }) => self.toggle_stack(stack),
            Some(card) => self.select(card),
            None => {}
        }
    }

    fn group_at(&self, x: u16, y: u16) -> Option<GroupArea> {
        self.viewport
            .groups
            .iter()
            .find(|group| group.area.contains(Position::new(x, y)))
            .filter(|group| group.index < self.groups().len())
            .cloned()
    }

    /// How far group `group` was scrolled when last drawn, in rows.
    pub fn group_scroll(&self, group: usize) -> usize {
        self.viewport
            .groups
            .iter()
            .find(|area| area.index == group)
            .map_or(0, |area| area.scroll)
    }

    fn focus_group(&mut self, group: usize) {
        if group != self.group {
            self.group = group;
            self.select_first();
        }
    }

    /// Collapses or expands stack `stack` of the selected group and puts the
    /// selection on it: on its header when collapsed, or back on its card.
    /// Empty stacks have nothing to collapse.
    fn toggle_stack(&mut self, stack: usize) {
        let has_cards = self
            .stack_at(self.group, stack)
            .is_some_and(|column| !column.cards.is_empty());
        let Some(key) = self.stack_key(self.group, stack).filter(|_| has_cards) else {
            return;
        };
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.select(Row::Header { stack });
    }

    /// Cards PageUp/PageDown move by: as many as fit in a group.
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
            self.group = 0;
            self.select_first();
        }
    }

    fn cycle_board(&mut self, delta: isize) {
        let count = self.snapshot.boards.len();
        if count > 0 {
            let next = (self.board as isize + delta).rem_euclid(count as isize) as usize;
            self.switch_board(next);
        }
    }

    fn move_group(&mut self, delta: isize) {
        let count = self.groups().len();
        if count > 0 {
            self.focus_group(self.group.saturating_add_signed(delta).min(count - 1));
        }
    }

    /// Moves the selection `delta` cards (or collapsed headers) through the
    /// selected group, across its stacks, stopping at the ends.
    fn move_card(&mut self, delta: isize) {
        let stops = self.stops(self.group);
        let Some(current) = self
            .selected_row()
            .and_then(|selected| stops.iter().position(|row| *row == selected))
        else {
            return;
        };
        let next = current.saturating_add_signed(delta).min(stops.len() - 1);
        self.select(stops[next]);
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
        CardSeverity, DashboardSnapshot, PendingCard, SnoozedCard, SourceBatch, SourceItem,
        SourceOutcome, SourceReport, build_snapshot,
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
            sorts: Default::default(),
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

    /// Two boards: Work (plane with two stacks + github) and Personal.
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

    /// h/l move across the board's groups (sources); j/k move through the
    /// cards of the group, flowing from the last card of a stack to the first
    /// card of the next one and back; the ends don't wrap.
    #[test]
    fn keys_move_across_groups_and_through_the_stacks_of_a_group() {
        let mut app = App::new(sample());
        assert_eq!(selected(&app), Some("p1"));

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(selected(&app), Some("p3"), "flows into the next stack");
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p3"), "stops at the end of the group");
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(selected(&app), Some("p2"), "flows back");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.selected_group(), 1);
        assert_eq!(selected(&app), Some("g1"), "next group starts at the top");
        app.handle_key(key(KeyCode::Right));
        assert_eq!(selected(&app), Some("g1"), "stops at the last group");

        app.handle_key(key(KeyCode::Char('h')));
        assert_eq!(selected(&app), Some("p1"));
        app.handle_key(key(KeyCode::Left));
        assert_eq!(selected(&app), Some("p1"), "stops at the first group");
    }

    /// c collapses the stack of the selected card: the selection rests on
    /// its header, which j/k still reach, while its cards are skipped; c
    /// there expands it again, back on the same card (or on the first one,
    /// once the selection has been elsewhere). A header has no details and
    /// no link.
    #[test]
    fn c_collapses_and_expands_the_selected_stack() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"));

        assert_eq!(app.handle_key(key(KeyCode::Char('c'))), Action::None);
        assert!(app.is_collapsed(0, 0));
        app.handle_key(key(KeyCode::Char('c')));
        assert!(!app.is_collapsed(0, 0));
        assert_eq!(selected(&app), Some("p2"), "back on the same card");

        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(selected(&app), None);
        assert_eq!(app.selected_row(), Some(Row::Header { stack: 0 }));
        assert_eq!(
            app.rows(0),
            vec![
                Row::Header { stack: 0 },
                Row::Header { stack: 1 },
                Row::Card { stack: 1, card: 0 },
                Row::Card { stack: 1, card: 0 },
            ]
        );

        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.popup(), None, "a header has no details");
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Action::None);

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p3"), "skips the collapsed cards");
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected_row(), Some(Row::Header { stack: 0 }));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected_row(), Some(Row::Header { stack: 0 }));

        app.handle_key(key(KeyCode::Char('c')));
        assert!(!app.is_collapsed(0, 0));
        assert_eq!(selected(&app), Some("p1"), "its first card");
    }

    /// A collapsed stack stays collapsed across refreshes and board
    /// switches, with the selection still on its header.
    #[test]
    fn collapsed_stacks_survive_refreshes() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char('c')));

        app.update(sample());
        assert!(app.is_collapsed(0, 0));
        assert_eq!(app.selected_row(), Some(Row::Header { stack: 0 }));

        app.handle_key(key(KeyCode::Char('2')));
        assert!(!app.is_collapsed(0, 0), "other boards are untouched");
        assert_eq!(selected(&app), Some("t1"));
        app.handle_key(key(KeyCode::Char('1')));
        assert!(app.is_collapsed(0, 0));
        assert_eq!(app.selected_row(), Some(Row::Header { stack: 0 }));
    }

    /// Empty stacks are skipped: the selection starts on the first card of
    /// the group, and a group with only empty stacks selects nothing (and c
    /// collapses nothing).
    #[test]
    fn empty_stacks_are_skipped_by_the_selection() {
        let mut app = App::new(snapshot(vec![
            report(
                "plane",
                "Work",
                &["Empty", "Mine", "Idle", "Inbox"],
                vec![card("Mine", "p1", None), card("Inbox", "p2", None)],
            ),
            report("github", "Work", &["Review"], vec![]),
        ]));
        assert_eq!(selected(&app), Some("p1"));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(selected(&app), Some("p1"));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"), "jumps over the empty stack");

        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.selected_group(), 1);
        assert_eq!(app.selected_row(), None);
        for code in [KeyCode::Char('j'), KeyCode::Char('c'), KeyCode::End] {
            app.handle_key(key(code));
        }
        assert_eq!(app.selected_row(), None);
        assert!(!app.is_collapsed(1, 0));
    }

    /// m asks to mark the selected card as in progress, or to unmark it when
    /// it is marked, also while its details are open; on a collapsed header
    /// there is no card to mark.
    #[test]
    fn m_asks_to_toggle_the_mark_of_the_selected_card() {
        let mut app = App::new(sample());
        let m = key(KeyCode::Char('m'));
        let set = |marked| Action::SetMark {
            id: "p1".to_owned(),
            marked,
        };

        assert_eq!(app.handle_key(m), set(true));

        let mut marked = sample();
        marked.marked = vec!["p1".to_owned()];
        app.update(marked);
        assert!(app.is_marked("p1"));
        assert!(!app.is_marked("p2"));
        assert_eq!(app.handle_key(m), set(false));

        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.handle_key(m), set(false), "from the details too");
        assert_eq!(app.popup(), Some(Popup::Card), "details stay open");
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.handle_key(m), Action::None, "a header is not a card");
    }

    /// The sample dashboard with `ids` snoozed: `s1` until a time, the
    /// others until they change.
    fn with_snoozed(ids: &[&str]) -> DashboardSnapshot {
        let mut snapshot = sample();
        snapshot.snoozed = ids
            .iter()
            .map(|id| SnoozedCard {
                card: card("Mine", id, None).card,
                source: "plane".to_owned(),
                until: (*id == "s1").then(|| Utc.with_ymd_and_hms(2026, 10, 8, 11, 0, 0).unwrap()),
            })
            .collect();
        snapshot
    }

    /// z opens the snooze menu for the selected card. A number key chooses
    /// right away; j/k move through the four choices (stopping at the ends)
    /// and Enter chooses the selected one. Choosing asks to snooze the card
    /// and closes the menu; digits there never switch boards.
    #[test]
    fn z_opens_the_snooze_menu_and_a_choice_asks_to_snooze() {
        let mut app = App::new(sample());
        let z = key(KeyCode::Char('z'));
        let snooze = |choice| Action::Snooze {
            id: "p1".to_owned(),
            choice,
        };

        assert_eq!(app.handle_key(z), Action::None);
        assert_eq!(app.popup(), Some(Popup::Snooze));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('2'))),
            snooze(Choice::Tomorrow)
        );
        assert_eq!(app.popup(), None, "choosing closes the menu");
        assert_eq!(app.current_board().unwrap().name, "Work", "same board");

        for (digit, choice) in [
            ('1', Choice::Hour),
            ('3', Choice::NextWeek),
            ('4', Choice::UntilChange),
        ] {
            app.handle_key(z);
            assert_eq!(app.handle_key(key(KeyCode::Char(digit))), snooze(choice));
        }

        app.handle_key(z);
        assert_eq!(app.popup_selected(), 0, "starts on the first choice");
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.popup_selected(), 0, "stops at the top");
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            snooze(Choice::NextWeek)
        );
        assert_eq!(selected(&app), Some("p1"), "the board selection stayed");

        app.handle_key(z);
        for _ in 0..6 {
            app.handle_key(key(KeyCode::Char('j')));
        }
        assert_eq!(app.popup_selected(), 3, "stops at the last choice");
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.handle_key(key(KeyCode::Enter)),
            snooze(Choice::UntilChange)
        );

        app.handle_key(z);
        assert_eq!(app.handle_key(key(KeyCode::Char('9'))), Action::None);
        assert_eq!(app.popup(), Some(Popup::Snooze), "not a choice");
    }

    /// Esc, q or z close the snooze menu without snoozing (and without
    /// quitting); on a collapsed header there is no card to snooze, and the
    /// menu closes when a refresh takes the card away.
    #[test]
    fn the_snooze_menu_closes_without_snoozing() {
        let mut app = App::new(sample());
        let z = key(KeyCode::Char('z'));

        for close in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('z')] {
            app.handle_key(z);
            assert_eq!(app.popup(), Some(Popup::Snooze));
            assert_eq!(app.handle_key(key(close)), Action::None);
            assert_eq!(app.popup(), None);
        }

        app.handle_key(z);
        app.update(snapshot(Vec::new()));
        assert_eq!(app.popup(), None, "the card is gone");

        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char('c')));
        app.handle_key(z);
        assert_eq!(app.popup(), None, "a header is not a card");
    }

    /// Z opens the list of snoozed cards: j/k select one (stopping at the
    /// ends), w or Enter asks to wake it and the list stays open; Esc, q or
    /// Z close it without quitting. The selection stays inside the list when
    /// it shrinks, and with nothing snoozed there is nothing to wake.
    #[test]
    fn capital_z_lists_the_snoozed_cards_and_w_wakes_one() {
        let mut app = App::new(with_snoozed(&["s1", "s2", "s3"]));
        let list = key(KeyCode::Char('Z'));
        let wake = |id: &str| Action::Wake { id: id.to_owned() };

        assert_eq!(app.handle_key(list), Action::None);
        assert_eq!(app.popup(), Some(Popup::Snoozed));
        assert_eq!(app.handle_key(key(KeyCode::Char('w'))), wake("s1"));
        assert_eq!(app.popup(), Some(Popup::Snoozed), "stays open");

        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.popup_selected(), 0, "stops at the top");
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), wake("s2"));
        for _ in 0..5 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.handle_key(key(KeyCode::Char('w'))), wake("s3"));

        // s3 woke up: the selection moves to the new last card.
        app.update(with_snoozed(&["s1", "s2"]));
        assert_eq!(app.handle_key(key(KeyCode::Char('w'))), wake("s2"));
        assert_eq!(selected(&app), Some("p1"), "the board selection stayed");

        for close in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('Z')] {
            assert_eq!(app.handle_key(key(close)), Action::None);
            assert_eq!(app.popup(), None);
            app.handle_key(list);
        }
        assert_eq!(app.popup_selected(), 0, "reopening starts at the top");

        app.update(with_snoozed(&[]));
        assert_eq!(app.popup(), Some(Popup::Snoozed), "an empty list stays");
        assert_eq!(app.handle_key(key(KeyCode::Char('w'))), Action::None);
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Action::None);
    }

    /// While the snooze menu or the snoozed list is open, the wheel moves
    /// its selection instead of the board's.
    #[test]
    fn mouse_wheel_moves_through_the_snooze_popups() {
        let mut app = App::new(with_snoozed(&["s1", "s2"]));
        app.set_viewport(two_groups());

        app.handle_key(key(KeyCode::Char('z')));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(app.popup_selected(), 1);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 12));
        assert_eq!(app.popup_selected(), 0);
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(key(KeyCode::Char('Z')));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(app.popup_selected(), 1, "stops at the last snoozed card");
        assert_eq!(selected(&app), Some("p1"), "the board selection stayed");
    }

    /// The marked cards are listed in marking order with their source,
    /// whatever the board, once each; marks whose card is not on the
    /// dashboard are left out.
    #[test]
    fn marked_cards_are_listed_in_marking_order_across_boards() {
        let mut snapshot = sample();
        snapshot.marked = ["t1", "gone", "g1", "p2"].map(str::to_owned).to_vec();
        let app = App::new(snapshot);

        let listed: Vec<(&str, &str)> = app
            .marked_cards()
            .into_iter()
            .map(|(card, source)| (card.id.as_str(), source))
            .collect();

        assert_eq!(
            listed,
            [("t1", "todoist"), ("g1", "github"), ("p2", "plane")]
        );
    }

    /// Digits and Tab switch boards (tabs); the selection starts at the first group.
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

    /// When the snapshot updates, the selection stays on the same board, group,
    /// stack and card; if the card is gone, it keeps the same position in the
    /// stack, or moves to the nearest card when the stack emptied.
    #[test]
    fn selection_follows_board_stack_and_card_across_refreshes() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(selected(&app), Some("p2"));

        // A new card above p2 in the same stack.
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

        // Mine emptied: the nearest card is in the next stack.
        app.update(snapshot(vec![report(
            "plane",
            "Work",
            &["Mine", "Inbox"],
            vec![card("Inbox", "p3", None)],
        )]));
        assert_eq!(selected(&app), Some("p3"));

        app.update(snapshot(Vec::new()));
        assert_eq!(selected(&app), None);
    }

    /// Enter opens the selected card's link; a card without a link does nothing.
    #[test]
    fn enter_opens_the_selected_card_link() {
        let mut app = App::new(sample());
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Action::None);

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

    /// Card details close when a refresh removes every card of the group.
    #[test]
    fn card_details_close_when_the_card_is_gone() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char(' ')));

        app.update(snapshot(Vec::new()));

        assert_eq!(app.popup(), None);
    }

    /// Plane's Mine stack with cards c0..c29, an empty stack, and a last
    /// stack with the card `last`.
    fn long_group() -> App {
        let items = (0..30)
            .map(|i| card("Mine", &format!("c{i}"), None))
            .chain([card("Later", "last", None)])
            .collect();
        App::new(snapshot(vec![report(
            "plane",
            "Work",
            &["Mine", "Inbox", "Later"],
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

    /// The two groups of the sample Work board side by side, 30 cells wide,
    /// from row 10: plane (Mine's header on row 10, p1 on 11-12, p2 on
    /// 13-14, Inbox's header on 15, p3 on 16-17) and github (Review's header
    /// on row 10, g1 on 11-12).
    fn two_groups() -> Viewport {
        Viewport {
            groups: (0..2)
                .map(|index| GroupArea {
                    index,
                    area: Rect::new(30 * index as u16, 10, 30, 10),
                    scroll: 0,
                })
                .collect(),
            card_page: 4,
            ..Viewport::default()
        }
    }

    /// PageUp/PageDown move the selection by the cards that fit in a group;
    /// Home/End (or g/G) jump to the first and last card of the group.
    #[test]
    fn page_keys_move_by_a_page_and_home_end_jump() {
        let mut app = long_group();
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
        assert_eq!(selected(&app), Some("last"), "the group's last card");
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(selected(&app), Some("last"), "stops at the end");
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(selected(&app), Some("c25"), "pages across stacks");
        app.handle_key(key(KeyCode::Home));
        assert_eq!(selected(&app), Some("c0"));
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(selected(&app), Some("c0"), "stops at the top");
        app.handle_key(key(KeyCode::Char('G')));
        assert_eq!(selected(&app), Some("last"));
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(selected(&app), Some("c0"));
    }

    /// The mouse wheel moves through the group under the pointer, focusing
    /// it first; away from the groups it moves through the focused one.
    #[test]
    fn mouse_wheel_scrolls_the_group_under_the_pointer() {
        let mut app = App::new(sample());
        app.set_viewport(two_groups());

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(selected(&app), Some("p3"), "flows into the next stack");
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 12));
        assert_eq!(selected(&app), Some("p3"), "stops at the end");

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 45, 12));
        assert_eq!(app.selected_group(), 1, "focuses the group under it");
        assert_eq!(selected(&app), Some("g1"));

        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 12));
        assert_eq!(selected(&app), Some("p1"), "focused at its top");
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 100, 0));
        assert_eq!(selected(&app), Some("p2"), "outside any group: focused one");
    }

    /// Clicking a card selects it; clicking a stack's header collapses or
    /// expands it; clicking elsewhere in a group focuses that group.
    #[test]
    fn clicking_selects_a_card_or_toggles_a_stack() {
        let mut app = App::new(sample());
        app.set_viewport(two_groups());
        let click = |column, row| mouse(MouseEventKind::Down(MouseButton::Left), column, row);

        app.handle_mouse(click(3, 13));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_mouse(click(3, 11));
        assert_eq!(selected(&app), Some("p1"));
        app.handle_mouse(click(3, 17));
        assert_eq!(selected(&app), Some("p3"), "a card's second row");
        app.handle_mouse(click(3, 19));
        assert_eq!(selected(&app), Some("p3"), "below the cards: nothing");

        app.handle_mouse(click(45, 18));
        assert_eq!(app.selected_group(), 1, "below the cards: focus only");
        assert_eq!(selected(&app), Some("g1"));

        // Mine's header: its cards go away and Inbox moves up to row 11.
        app.handle_mouse(click(3, 10));
        assert_eq!(app.selected_group(), 0);
        assert!(app.is_collapsed(0, 0));
        assert_eq!(app.selected_row(), Some(Row::Header { stack: 0 }));
        app.handle_mouse(click(3, 13));
        assert_eq!(selected(&app), Some("p3"));
        app.handle_mouse(click(3, 10));
        assert!(!app.is_collapsed(0, 0));
        assert_eq!(selected(&app), Some("p1"), "expanded: its first card");
    }

    /// A click in a scrolled group lands on the row as drawn.
    #[test]
    fn clicks_account_for_the_group_scroll() {
        let mut app = App::new(sample());
        let mut viewport = two_groups();
        // Rows 0-3 (Mine's header, p1, half of p2) are above the area.
        viewport.groups[0].scroll = 4;
        app.set_viewport(viewport);
        assert_eq!(app.group_scroll(0), 4);
        assert_eq!(app.group_scroll(1), 0);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 10));
        assert_eq!(selected(&app), Some("p2"));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 12));
        assert_eq!(selected(&app), Some("p3"));
    }

    /// While details are open, the wheel scrolls them and clicks do nothing.
    #[test]
    fn mouse_wheel_scrolls_open_details() {
        let mut app = App::new(sample());
        app.handle_key(key(KeyCode::Char(' ')));
        app.set_viewport(Viewport {
            popup_page: 5,
            popup_max_scroll: 10,
            ..two_groups()
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
