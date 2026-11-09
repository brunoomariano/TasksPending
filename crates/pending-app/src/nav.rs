//! Moving the keyboard focus between cards: the page as columns of cards
//! (the stages of the Activity, the groups of the Board, top to bottom),
//! and where each key takes the focus.

use std::time::{Duration, Instant};

/// A card's place on the page: its column, and its row in it.
pub type Spot = (usize, usize);

/// Where a key moves the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    /// The first card of the column.
    First,
    /// The last card of the column.
    Last,
}

/// Where `step` takes the focus from `from` on a page whose columns hold
/// `columns` cards each. With no card in focus, any step goes to the
/// first card on the page; with no card on it, nowhere.
pub fn step(columns: &[usize], from: Option<Spot>, step: Move) -> Option<Spot> {
    let on_page = from.filter(|&(column, row)| columns.get(column).is_some_and(|&n| row < n));
    let Some((column, row)) = on_page else {
        return columns
            .iter()
            .position(|&n| n > 0)
            .map(|column| (column, 0));
    };
    let last = columns[column] - 1;
    let across = |to: Option<usize>| match to {
        Some(to) => (to, row.min(columns[to] - 1)),
        None => (column, row),
    };
    Some(match step {
        Move::Up => (column, row.saturating_sub(1)),
        Move::Down => (column, (row + 1).min(last)),
        Move::First => (column, 0),
        Move::Last => (column, last),
        Move::Left => across((0..column).rev().find(|&to| columns[to] > 0)),
        Move::Right => across((column + 1..columns.len()).find(|&to| columns[to] > 0)),
    })
}

/// Where the card `id` is on a page of `columns`. A card can sit in two
/// stacks of a group, or in two groups: the copy in the column of `was`,
/// nearest its row, wins.
pub fn find(columns: &[Vec<String>], id: &str, was: Option<Spot>) -> Option<Spot> {
    let rows = |column: usize| {
        columns.get(column).into_iter().flat_map(move |cards| {
            cards
                .iter()
                .enumerate()
                .filter(move |(_, card)| *card == id)
                .map(move |(row, _)| (column, row))
        })
    };
    was.and_then(|(column, row)| rows(column).min_by_key(|(_, at)| at.abs_diff(row)))
        .or_else(|| (0..columns.len()).find_map(|column| rows(column).next()))
}

/// The card nearest to `spot` on a page whose columns hold `columns`
/// cards each: the one at the same place, the one above when the column
/// got shorter, or the nearest column with cards when it emptied.
pub fn nearest(columns: &[usize], spot: Spot) -> Option<Spot> {
    let (column, row) = spot;
    let column = column.min(columns.len().checked_sub(1)?);
    let at = |column: usize| (columns[column] > 0).then(|| (column, row.min(columns[column] - 1)));
    at(column).or_else(|| {
        (1..columns.len()).find_map(|distance| {
            let left = column.checked_sub(distance).and_then(at);
            let right = Some(column + distance)
                .filter(|&to| to < columns.len())
                .and_then(at);
            left.or(right)
        })
    })
}

/// The keys held down, by hardware code, to tell a press from the repeats
/// GDK sends while a key stays down.
#[derive(Debug, Default)]
pub struct Held {
    down: std::collections::HashSet<u32>,
}

impl Held {
    /// The key `code` went down: whether it is a fresh press, not a
    /// repeat.
    pub fn press(&mut self, code: u32) -> bool {
        self.down.insert(code)
    }

    pub fn release(&mut self, code: u32) {
        self.down.remove(&code);
    }

    /// The window lost the keys, and their releases go elsewhere.
    pub fn clear(&mut self) {
        self.down.clear();
    }
}

/// How long the first press of Delete waits for the second: as long as
/// the note that asks for it shows.
pub const DELETE_WINDOW: Duration = Duration::from_secs(6);

/// Delete, which acts on the second press on the same card within
/// [`DELETE_WINDOW`] of the first.
#[derive(Debug, Default)]
pub struct Armed {
    pressed: Option<(String, Instant)>,
}

impl Armed {
    /// Delete pressed on `id` at `now`: whether this press deletes it.
    pub fn press(&mut self, id: &str, now: Instant) -> bool {
        let fires = matches!(
            &self.pressed,
            Some((armed, at)) if armed == id && now.duration_since(*at) < DELETE_WINDOW
        );
        self.pressed = (!fires).then(|| (id.to_owned(), now));
        fires
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Up and down stay in the column and stop at its ends; first and last
    /// go to its ends.
    #[test]
    fn moves_up_and_down_a_column() {
        let columns = [3, 2];
        assert_eq!(step(&columns, Some((0, 1)), Move::Up), Some((0, 0)));
        assert_eq!(step(&columns, Some((0, 0)), Move::Up), Some((0, 0)));
        assert_eq!(step(&columns, Some((0, 1)), Move::Down), Some((0, 2)));
        assert_eq!(step(&columns, Some((0, 2)), Move::Down), Some((0, 2)));
        assert_eq!(step(&columns, Some((0, 1)), Move::Last), Some((0, 2)));
        assert_eq!(step(&columns, Some((1, 1)), Move::First), Some((1, 0)));
    }

    /// Left and right go to the next column that has cards, at the same
    /// row or the last one it has, and stop at the page's edge.
    #[test]
    fn moves_across_columns() {
        let columns = [3, 0, 1, 4];
        assert_eq!(
            step(&columns, Some((0, 2)), Move::Right),
            Some((2, 0)),
            "past the empty column, to its only card"
        );
        assert_eq!(step(&columns, Some((2, 0)), Move::Right), Some((3, 0)));
        assert_eq!(step(&columns, Some((3, 3)), Move::Right), Some((3, 3)));
        assert_eq!(step(&columns, Some((3, 3)), Move::Left), Some((2, 0)));
        assert_eq!(step(&columns, Some((2, 0)), Move::Left), Some((0, 0)));
        assert_eq!(step(&columns, Some((0, 1)), Move::Left), Some((0, 1)));
        assert_eq!(
            step(&[2, 3], Some((0, 1)), Move::Right),
            Some((1, 1)),
            "the same row"
        );
    }

    /// With no card in focus, or one no longer on the page, any key goes
    /// to the first card; a page without cards has nowhere to go.
    #[test]
    fn starts_from_the_first_card() {
        assert_eq!(step(&[0, 2], None, Move::Down), Some((1, 0)));
        assert_eq!(step(&[0, 2], None, Move::Last), Some((1, 0)));
        assert_eq!(step(&[0, 2], Some((5, 0)), Move::Up), Some((1, 0)));
        assert_eq!(step(&[0, 2], Some((1, 7)), Move::Up), Some((1, 0)));
        assert_eq!(step(&[0, 0], None, Move::Down), None);
        assert_eq!(step(&[], Some((0, 0)), Move::Down), None);
    }

    fn page(columns: &[&[&str]]) -> Vec<Vec<String>> {
        columns
            .iter()
            .map(|column| column.iter().map(|id| (*id).to_owned()).collect())
            .collect()
    }

    /// A card is found where it is, in the column asked for when it sits
    /// in more than one.
    #[test]
    fn finds_a_card() {
        let columns = page(&[&["a", "b"], &["c", "a"]]);
        assert_eq!(find(&columns, "b", None), Some((0, 1)));
        assert_eq!(find(&columns, "a", None), Some((0, 0)));
        assert_eq!(find(&columns, "a", Some((1, 0))), Some((1, 1)));
        assert_eq!(
            find(&columns, "c", Some((0, 0))),
            Some((1, 0)),
            "or wherever it went"
        );
        assert_eq!(find(&columns, "z", Some((0, 0))), None);
        assert_eq!(find(&columns, "a", Some((9, 0))), Some((0, 0)));

        // The same card in two stacks of a group: the copy that had the
        // focus keeps it.
        let twice = page(&[&["a", "b", "a", "c"]]);
        assert_eq!(find(&twice, "a", Some((0, 2))), Some((0, 2)));
        assert_eq!(find(&twice, "a", Some((0, 3))), Some((0, 2)));
        assert_eq!(find(&twice, "a", Some((0, 0))), Some((0, 0)));
    }

    /// A key held down repeats: the first press is fresh, the repeats are
    /// not, until the key comes up or the window loses the keys.
    #[test]
    fn tells_a_press_from_a_repeat() {
        let mut held = Held::default();
        assert!(held.press(119));
        assert!(!held.press(119), "a repeat");
        assert!(held.press(36), "another key");
        held.release(119);
        assert!(held.press(119), "pressed again");
        held.clear();
        assert!(held.press(36), "after the window lost the keys");
    }

    /// When the card in focus left the page, the focus goes to the card
    /// that took its place, the one above it, or the nearest column with
    /// cards.
    #[test]
    fn finds_the_nearest_card() {
        assert_eq!(nearest(&[3], (0, 1)), Some((0, 1)));
        assert_eq!(nearest(&[1], (0, 1)), Some((0, 0)));
        assert_eq!(nearest(&[2, 0, 1], (1, 1)), Some((0, 1)), "left first");
        assert_eq!(nearest(&[0, 0, 4], (1, 3)), Some((2, 3)));
        assert_eq!(nearest(&[0, 0], (1, 0)), None);
        assert_eq!(nearest(&[2], (4, 0)), Some((0, 0)));
    }

    /// Delete acts on the second press on the same card in time; a press
    /// on another card, or too late, starts over.
    #[test]
    fn deletes_on_the_second_press() {
        let now = Instant::now();
        let mut armed = Armed::default();
        assert!(!armed.press("todo:1", now));
        assert!(armed.press("todo:1", now + Duration::from_secs(1)));
        assert!(!armed.press("todo:1", now + Duration::from_secs(2)), "once");

        assert!(!armed.press("todo:2", now));
        assert!(!armed.press("todo:3", now + Duration::from_millis(10)));
        assert!(armed.press("todo:3", now + Duration::from_millis(20)));

        assert!(!armed.press("todo:4", now));
        assert!(!armed.press("todo:4", now + DELETE_WINDOW));
        assert!(armed.press("todo:4", now + DELETE_WINDOW + Duration::from_secs(1)));
    }
}
