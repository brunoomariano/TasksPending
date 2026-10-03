//! Rendering of the dashboard as on the web page: board tabs, then the
//! board's groups (sources) side by side, each with its stacks one above
//! another.

use chrono::{DateTime, Local, Utc};
use pending_core::{CardSeverity, Column, Group, PendingCard, SourceHealth, SourceStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Widget, Wrap,
};

use crate::app::{App, CARD_ROWS, GroupArea, Popup, Row, Viewport};
use crate::clock;

const HELP: &str = "1-9/tab boards · h/l groups · j/k cards · c collapse · enter open · space details · s sources · r refresh · q quit";
/// Largest details popup, so long lines stay readable on wide terminals.
const POPUP_MAX_WIDTH: u16 = 110;
const POPUP_MAX_HEIGHT: u16 = 30;
/// Narrowest useful group, border included; with less room the board scrolls
/// sideways.
const MIN_GROUP_WIDTH: u16 = 30;
/// Fewest rows the board keeps (about three cards per group); the clock
/// hides rather than take them.
const MIN_BOARD_ROWS: u16 = 10;
/// Glyphs in `HH:MM:SS`.
const CLOCK_TEXT_LEN: usize = 8;
/// The same calm accent as the source titles and the selected tab.
const CLOCK_COLOR: Color = Color::Cyan;

fn local(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// `header` describes where the config came from.
/// `now` is the local time the clock shows. Returns the geometry the keys
/// need (see `App::set_viewport`).
pub fn render(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    header: &str,
    now: DateTime<Local>,
) -> Viewport {
    let screen = area;
    let mut viewport = Viewport::default();
    let snapshot = app.snapshot();
    let sources_height =
        snapshot.sources.len().max(1) as u16 + u16::from(snapshot.config_error.is_some()) + 2;
    // Header, tabs, Sources panel and footer.
    let chrome = 1 + 1 + sources_height + 1;
    let clock_size = clock_size(area, chrome);
    let clock_height = clock_size.map_or(0, |size| 1 + clock::GLYPH_ROWS * size);

    let [clock_area, area] =
        Layout::vertical([Constraint::Length(clock_height), Constraint::Min(0)]).areas(area);
    if let Some(size) = clock_size {
        render_clock(clock_area, buf, size, now);
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(sources_height),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    Paragraph::new(Line::from(vec![
        Span::styled(
            "TasksPending",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {}  ", local(snapshot.generated_at))),
        Span::styled(header, Style::default().fg(Color::DarkGray)),
    ]))
    .render(layout[0], buf);

    render_tabs(layout[1], buf, app);

    // One line per source, clipped, so a long failure message never pushes
    // the other sources out of the panel.
    let mut sources: Vec<Line> = if snapshot.sources.is_empty() {
        vec![Line::from(Span::styled(
            "No sources configured: add [[sources]] to the config file.",
            Style::default().fg(Color::Yellow),
        ))]
    } else {
        snapshot.sources.iter().map(source_line).collect()
    };
    if let Some(error) = &snapshot.config_error {
        sources.insert(
            0,
            Line::from(Span::styled(
                format!("config not reloaded (previous one still running): {error}"),
                Style::default().fg(Color::Red),
            )),
        );
    }
    Paragraph::new(sources)
        .block(Block::default().title("Sources").borders(Borders::ALL))
        .render(layout[2], buf);

    render_board(layout[3], buf, app, &mut viewport);

    let footer = match app.notice() {
        Some(notice) if app.notice_is_error() => {
            Span::styled(notice, Style::default().fg(Color::Red))
        }
        Some(notice) => Span::styled(notice, Style::default().fg(Color::Cyan)),
        None => Span::styled(HELP, Style::default().fg(Color::DarkGray)),
    };
    Paragraph::new(footer).render(layout[4], buf);

    if let Some(popup) = app.popup() {
        render_popup(screen, buf, app, popup, &mut viewport);
    }
    viewport
}

/// The popup's area: centered, at most `POPUP_MAX_WIDTH` × `POPUP_MAX_HEIGHT`,
/// with a margin around it on smaller terminals.
fn popup_area(screen: Rect) -> Rect {
    let width = POPUP_MAX_WIDTH.min(screen.width.saturating_sub(4));
    let height = POPUP_MAX_HEIGHT.min(screen.height.saturating_sub(2));
    Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    )
}

/// Draws `popup` over the screen, scrolled as far as the app asks within its
/// text, and records its page size and scroll limit in `viewport`.
fn render_popup(screen: Rect, buf: &mut Buffer, app: &App, popup: Popup, viewport: &mut Viewport) {
    let (title, lines, hint) = match popup {
        Popup::Card => {
            let Some(card) = app.selected_card() else {
                return;
            };
            (
                " Details ",
                card_details(app, card),
                " j/k scroll · enter open · esc close ",
            )
        }
        Popup::Sources => (" Sources ", source_details(app), " j/k scroll · esc close "),
    };

    let area = popup_area(screen);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))).right_aligned(),
        );
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    let total = u16::try_from(paragraph.line_count(inner.width)).unwrap_or(u16::MAX);
    let max_scroll = total.saturating_sub(inner.height);
    let scroll = app.popup_scroll().min(max_scroll);

    Clear.render(area, buf);
    paragraph.block(block).scroll((scroll, 0)).render(area, buf);
    if max_scroll > 0 {
        let mut state = ScrollbarState::new(usize::from(max_scroll)).position(usize::from(scroll));
        Scrollbar::new(ScrollbarOrientation::VerticalRight).render(
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            buf,
            &mut state,
        );
    }

    viewport.popup_page = inner.height;
    viewport.popup_max_scroll = max_scroll;
}

/// The selected card in full: title, where it comes from, dates, link and
/// the whole body.
fn card_details<'a>(app: &App, card: &'a PendingCard) -> Vec<Line<'a>> {
    let field = |name: &str, value: String| {
        Line::from(vec![
            Span::styled(format!("{name:<9}"), Style::default().fg(Color::DarkGray)),
            Span::raw(value),
        ])
    };
    let mut lines = vec![
        Line::from(Span::styled(
            card.title.as_str(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    if let Some((group, stack)) = app.selected_stack() {
        lines.push(field("Source", group.source.clone()));
        if let Some(board) = app.current_board() {
            lines.push(field("Board", board.name.clone()));
        }
        lines.push(field("Stack", stack.name.clone()));
    }
    if let Some(due) = card.due_at {
        lines.push(field("Due", local(due)));
    }
    lines.push(field("Updated", local(card.updated_at)));
    if let Some(url) = &card.url {
        lines.push(field("Link", url.clone()));
    }
    if !card.body.is_empty() {
        lines.push(Line::default());
        lines.extend(card.body.lines().map(Line::raw));
    }
    lines
}

/// Every source with its status, last refresh and full message, after the
/// config error if there is one.
fn source_details(app: &App) -> Vec<Line<'_>> {
    let snapshot = app.snapshot();
    let mut lines = Vec::new();
    if let Some(error) = &snapshot.config_error {
        lines.push(Line::from(Span::styled(
            format!("config not reloaded (previous one still running): {error}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::default());
    }
    if snapshot.sources.is_empty() {
        lines.push(Line::raw(
            "No sources configured: add [[sources]] to the config file.",
        ));
    }
    for source in &snapshot.sources {
        let (label, color) = status_label(source.status);
        let mut header = vec![
            Span::styled(
                source.name.as_str(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(label, Style::default().fg(color)),
        ];
        if let Some(at) = source.last_refresh_at {
            header.push(Span::styled(
                format!("  last refresh {}", local(at)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(header));
        if let Some(message) = &source.message {
            lines.extend(message.lines().map(|line| Line::raw(format!("  {line}"))));
        }
        lines.push(Line::default());
    }
    lines
}

/// The clock's glyph size for a terminal of `area` whose other panels (all
/// but the board) take `chrome` rows, or `None` to hide it. The clock (the
/// date line plus the digits) takes at most a third of the height and never
/// leaves the board fewer than `MIN_BOARD_ROWS`.
fn clock_size(area: Rect, chrome: u16) -> Option<u16> {
    let budget = (area.height / 3).min(
        area.height
            .saturating_sub(chrome)
            .saturating_sub(MIN_BOARD_ROWS),
    );
    clock::fit(CLOCK_TEXT_LEN, area.width, budget.saturating_sub(1))
}

/// The date on one line, then `HH:MM:SS` in block digits, centered.
fn render_clock(area: Rect, buf: &mut Buffer, size: u16, now: DateTime<Local>) {
    let [date, digits] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    Paragraph::new(now.format("%A, %B %-d, %Y").to_string())
        .style(Style::default().fg(Color::Gray))
        .alignment(Alignment::Center)
        .render(date, buf);
    clock::draw(
        &now.format("%H:%M:%S").to_string(),
        size,
        digits,
        Style::default().fg(CLOCK_COLOR),
        buf,
    );
}

fn render_tabs(area: Rect, buf: &mut Buffer, app: &App) {
    let mut spans = Vec::new();
    for (index, board) in app.snapshot().boards.iter().enumerate() {
        let label = format!(" {} {} ", index + 1, board.name);
        let style = if index == app.board_index() {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn status_label(status: SourceStatus) -> (&'static str, Color) {
    match status {
        SourceStatus::Ready => ("ready", Color::Green),
        SourceStatus::Refreshing => ("refreshing", Color::Blue),
        SourceStatus::Degraded => ("degraded", Color::Yellow),
        SourceStatus::Failed => ("failed", Color::Red),
    }
}

fn source_line(source: &SourceHealth) -> Line<'_> {
    let (label, color) = status_label(source.status);
    let mut spans = vec![
        Span::raw(format!("{} ", source.name)),
        Span::styled(label, Style::default().fg(color)),
    ];
    if let Some(message) = &source.message {
        spans.push(Span::raw(format!(": {message}")));
    }
    if let Some(at) = source.last_refresh_at {
        spans.push(Span::styled(
            format!("  ({})", local(at)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// The first visible group and how many fit, keeping `selected` in view.
fn visible_window(total: usize, selected: usize, width: u16) -> (usize, usize) {
    let fit = usize::from((width / MIN_GROUP_WIDTH).max(1)).min(total);
    let start = selected.saturating_sub(fit - 1).min(total - fit);
    (start, fit)
}

/// Draws the current board, its groups side by side, and records where they
/// landed.
fn render_board(area: Rect, buf: &mut Buffer, app: &App, viewport: &mut Viewport) {
    let groups = app.groups();
    if groups.is_empty() {
        Paragraph::new("No stacks on this board.")
            .block(Block::default().borders(Borders::ALL))
            .render(area, buf);
        return;
    }

    let (start, fit) = visible_window(groups.len(), app.selected_group(), area.width);
    let areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, fit as u32); fit])
        .split(area);
    for (offset, group_area) in areas.iter().enumerate() {
        let index = start + offset;
        let drawn = render_group(*group_area, buf, app, index, &groups[index]);
        // Below a stack's header row.
        viewport.card_page = usize::from(drawn.area.height.saturating_sub(1) / CARD_ROWS);
        viewport.groups.push(drawn);
    }
}

/// How far group `index` scrolls, in rows, to show `height` of its `rows`.
/// The focused group moves from where it was last drawn only as far as the
/// selection needs to stay visible (the first card of a stack brings the
/// stack's header along); the others rest at their top, where focusing them
/// puts the selection.
fn group_scroll(app: &App, index: usize, rows: &[Row], height: usize) -> usize {
    if index != app.selected_group() || height == 0 {
        return 0;
    }
    let Some(selected) = app.selected_row() else {
        return 0;
    };
    let mut scroll = app
        .group_scroll(index)
        .min(rows.len().saturating_sub(height));
    let Some(first) = rows.iter().position(|row| *row == selected) else {
        return scroll;
    };
    let last = rows
        .iter()
        .rposition(|row| *row == selected)
        .unwrap_or(first);
    let top = match selected {
        Row::Card { card: 0, .. } => first.saturating_sub(1),
        _ => first,
    };
    if top < scroll {
        scroll = top;
    }
    if last >= scroll + height {
        scroll = last + 1 - height;
    }
    scroll
}

/// Draws a group: a titled box with its stacks one above another, scrolled
/// to keep the selection visible. Returns where its rows landed.
fn render_group(area: Rect, buf: &mut Buffer, app: &App, index: usize, group: &Group) -> GroupArea {
    let focused = index == app.selected_group();
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", group.source),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }));
    let inner = block.inner(area);
    block.render(area, buf);

    let rows = app.rows(index);
    let height = usize::from(inner.height);
    let scroll = group_scroll(app, index, &rows, height);
    let selected = app.selected_row().filter(|_| focused);

    let visible = rows.iter().enumerate().skip(scroll);
    for (y, (position, row)) in (inner.y..inner.bottom()).zip(visible) {
        let is_selected = selected == Some(*row);
        let line = match *row {
            Row::Header { stack } => {
                let holds_selection = matches!(
                    selected,
                    Some(Row::Card { stack: holder, .. }) if holder == stack
                );
                stack_header(
                    &group.columns[stack],
                    app.is_collapsed(index, stack),
                    is_selected,
                    holds_selection,
                )
            }
            Row::Card { stack, card } => {
                let card = &group.columns[stack].cards[card];
                // A card's rows are equal: the second one is its body.
                if position > 0 && rows[position - 1] == *row {
                    card_body(card)
                } else {
                    card_title(card, is_selected)
                }
            }
        };
        Paragraph::new(line).render(Rect::new(inner.x, y, inner.width, 1), buf);
    }

    if rows.len() > height && height > 0 {
        let mut state = ScrollbarState::new(rows.len() - height).position(scroll);
        Scrollbar::new(ScrollbarOrientation::VerticalRight).render(
            area.inner(Margin {
                vertical: 1,
                horizontal: 0,
            }),
            buf,
            &mut state,
        );
    }

    GroupArea {
        index,
        area: inner,
        scroll,
    }
}

/// A stack's header: an open/closed marker, its name and card count. An
/// empty stack is dimmed and has no marker; the selection rests on a
/// collapsed stack's header, and the stack holding it stands out.
fn stack_header(
    stack: &Column,
    collapsed: bool,
    selected: bool,
    holds_selection: bool,
) -> Line<'_> {
    let label = format!("{} ({})", stack.name, stack.cards.len());
    if stack.cards.is_empty() {
        return Line::from(Span::styled(
            format!("  {label}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let style = Style::default().fg(Color::Yellow);
    let label_style = if selected {
        style.add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else if holds_selection {
        style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        style
    };
    Line::from(vec![
        Span::styled(if collapsed { "▸ " } else { "▾ " }, style),
        Span::styled(label, label_style),
    ])
}

fn card_title(card: &PendingCard, selected: bool) -> Line<'_> {
    let marker = match card.severity {
        CardSeverity::Info => Style::default().fg(Color::Blue),
        CardSeverity::Warning => Style::default().fg(Color::Yellow),
        CardSeverity::Critical => Style::default().fg(Color::Red),
    };
    let title = if selected {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    Line::from(vec![
        Span::styled("▍", marker),
        Span::styled(card.title.as_str(), title),
    ])
}

fn card_body(card: &PendingCard) -> Line<'_> {
    Line::from(Span::styled(
        format!(" {}", body_summary(&card.body)),
        Style::default().fg(Color::DarkGray),
    ))
}

/// A card's body on one line: its non-blank lines, trimmed and separated by
/// " · " (the separator sources already use between details).
fn body_summary(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone, Utc};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use pending_core::{
        CardSeverity, PendingCard, SourceBatch, SourceError, SourceItem, SourceOutcome,
        SourceReport, build_snapshot,
    };
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;
    use crate::app::App;

    /// Tuesday, 2026-09-29 14:05:09, local time.
    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 29, 14, 5, 9).unwrap()
    }

    fn screen(app: &App, width: u16, height: u16) -> String {
        render_screen(app, width, height).0
    }

    /// The screen as text, and the geometry `render` reports back.
    fn render_screen(app: &App, width: u16, height: u16) -> (String, Viewport) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let viewport = render(
            area,
            &mut buf,
            app,
            "config: ~/.config/tasks-pending/config.toml",
            now(),
        );
        let text = (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        (text, viewport)
    }

    fn card(column: &str, id: &str, title: &str) -> SourceItem {
        SourceItem {
            column: column.to_owned(),
            card: PendingCard {
                id: id.to_owned(),
                title: title.to_owned(),
                body: format!("{id} · body"),
                source: "test".to_owned(),
                url: None,
                due_at: None,
                severity: CardSeverity::Info,
                updated_at: Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap(),
            },
        }
    }

    fn fresh(name: &str, board: &str, columns: &[&str], items: Vec<SourceItem>) -> SourceReport {
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
                refreshed_at: Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap(),
            },
        }
    }

    fn app(reports: Vec<SourceReport>) -> App {
        App::new(build_snapshot(
            Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap(),
            reports,
        ))
    }

    fn work() -> App {
        app(vec![
            fresh(
                "plane",
                "Work",
                &["Mine", "Inbox"],
                vec![card("Mine", "API-1", "Fix login")],
            ),
            fresh(
                "github",
                "Work",
                &["Review"],
                vec![card("Review", "gh-1", "Add cache")],
            ),
            SourceReport {
                name: "calendar".to_owned(),
                board: "Personal".to_owned(),
                columns: vec!["Today".to_owned()],
                icon: None,
                sorts: Default::default(),
                outcome: SourceOutcome::Failed(SourceError::new("feed returned 404")),
            },
        ])
    }

    /// The screen shows boards as tabs, each source as a group with its
    /// stacks and their card counts, and the health of the sources.
    #[test]
    fn boards_are_tabs_and_sources_are_groups_of_stacks() {
        let screen = screen(&work(), 120, 24);

        for expected in [
            "1 Work",
            "2 Personal",
            " plane ",
            " github ",
            "Mine (1)",
            "Inbox (0)",
            "Review (1)",
            "Fix login",
            "Add cache",
            "calendar failed: feed returned 404",
            "h/l groups",
            "c collapse",
            "q quit",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
        assert!(
            !screen.contains("Today"),
            "other boards stay hidden\n{screen}"
        );
    }

    /// Groups sit side by side in config order; inside a group the stacks
    /// sit one above another, each a header line (name and count) followed
    /// by its cards.
    #[test]
    fn groups_sit_side_by_side_and_their_stacks_one_above_another() {
        let app = app(vec![
            fresh(
                "plane",
                "Work",
                &["Mine", "Inbox"],
                vec![
                    card("Mine", "API-1", "Fix login"),
                    card("Inbox", "API-2", "Triage report"),
                ],
            ),
            fresh(
                "github",
                "Work",
                &["Review"],
                vec![card("Review", "gh-1", "Add cache")],
            ),
        ]);
        let screen = screen(&app, 120, 24);

        let (plane_x, group_row) = position(&screen, " plane ");
        let (github_x, github_row) = position(&screen, " github ");
        assert_eq!(group_row, github_row, "{screen}");
        assert!(plane_x < github_x, "{screen}");

        let (mine_x, mine_row) = position(&screen, "▾ Mine (1)");
        assert_eq!(mine_row, group_row + 1, "{screen}");
        assert_eq!(position(&screen, "Fix login").1, mine_row + 1, "{screen}");
        assert_eq!(position(&screen, "API-1 · body").1, mine_row + 2);
        // The next stack follows right below, in the same group.
        assert_eq!(
            position(&screen, "▾ Inbox (1)"),
            (mine_x, mine_row + 3),
            "{screen}"
        );
        assert_eq!(position(&screen, "Triage report").1, mine_row + 4);

        let (review_x, review_row) = position(&screen, "▾ Review (1)");
        assert_eq!(review_row, mine_row, "{screen}");
        assert_eq!(review_x, github_x, "inside the github group\n{screen}");
    }

    /// An empty stack is only its header, dimmed, with (0): the next stack
    /// follows on the next row.
    #[test]
    fn empty_stacks_are_a_dimmed_header_only() {
        let app = app(vec![fresh(
            "plane",
            "Work",
            &["Mine", "Idle", "Inbox"],
            vec![
                card("Mine", "API-1", "Fix login"),
                card("Inbox", "API-2", "Triage report"),
            ],
        )]);
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, "", now());
        let screen = screen(&app, 100, 24);

        let (x, y) = position(&screen, "Idle (0)");
        assert_eq!(y, position(&screen, "Fix login").1 + 2, "{screen}");
        assert_eq!(position(&screen, "Inbox (1)").1, y + 1, "{screen}");
        assert_eq!(buf[(x, y)].fg, Color::DarkGray);
        let (x, y) = position(&screen, "Inbox (1)");
        assert_ne!(buf[(x, y)].fg, Color::DarkGray);
    }

    /// A collapsed stack shows only its header, marked as closed, and the
    /// next stack moves up; expanding it brings the cards back.
    #[test]
    fn collapsed_stacks_show_only_their_header() {
        let mut app = app(vec![fresh(
            "plane",
            "Work",
            &["Mine", "Inbox"],
            vec![
                card("Mine", "API-1", "Fix login"),
                card("Inbox", "API-2", "Triage report"),
            ],
        )]);
        app.handle_key(KeyEvent::from(KeyCode::Char('c')));

        let collapsed = screen(&app, 100, 24);
        assert!(!collapsed.contains("Fix login"), "{collapsed}");
        let (_, y) = position(&collapsed, "▸ Mine (1)");
        assert_eq!(position(&collapsed, "▾ Inbox (1)").1, y + 1, "{collapsed}");
        assert!(collapsed.contains("Triage report"), "{collapsed}");

        app.handle_key(KeyEvent::from(KeyCode::Char('c')));
        let expanded = screen(&app, 100, 24);
        assert!(expanded.contains("▾ Mine (1)"), "{expanded}");
        assert!(expanded.contains("Fix login"), "{expanded}");
    }

    /// With more groups than fit the width, the board scrolls sideways to
    /// keep the selected group visible, and no group gets narrower than the
    /// minimum.
    #[test]
    fn wide_boards_scroll_to_the_selected_group() {
        let reports = (0..8)
            .map(|i| {
                fresh(
                    &format!("src{i}"),
                    "Work",
                    &["Mine"],
                    vec![card(
                        "Mine",
                        &format!("c{i}"),
                        &format!("Card of group {i}"),
                    )],
                )
            })
            .collect();
        let mut app = app(reports);

        let (start, viewport) = render_screen(&app, 80, 24);
        assert!(start.contains(" src0 "), "{start}");
        assert!(start.contains(" src1 "), "{start}");
        assert!(!start.contains(" src2 "), "{start}");
        assert_eq!(viewport.groups.len(), 2, "{viewport:?}");
        assert!(viewport.groups.iter().all(|g| g.area.width >= 28));

        for _ in 0..7 {
            app.handle_key(KeyEvent::from(KeyCode::Char('l')));
        }
        let end = screen(&app, 80, 24);
        assert!(end.contains(" src7 "), "{end}");
        assert!(end.contains("Card of group 7"), "{end}");
        assert!(!end.contains(" src0 "), "{end}");
    }

    /// With more cards than fit the height, the group scrolls to keep the
    /// selected card visible, through its stacks, and back up to the first
    /// stack's header.
    #[test]
    fn long_groups_keep_the_selected_card_visible() {
        let items = (0..30)
            .map(|i| card("Mine", &format!("c{i}"), &format!("Card number {i}")))
            .chain([card("Later", "last", "Final card")])
            .collect();
        let mut app = app(vec![fresh("plane", "Work", &["Mine", "Later"], items)]);
        // What the event loop does: draw, then record the geometry.
        let draw = |app: &mut App| {
            let (screen, viewport) = render_screen(app, 80, 24);
            app.set_viewport(viewport);
            screen
        };

        let top = draw(&mut app);
        assert!(top.contains("Mine (30)"), "{top}");
        assert!(top.contains("Card number 0 "), "{top}");
        assert!(!top.contains("Later (1)"), "{top}");

        for _ in 0..29 {
            app.handle_key(KeyEvent::from(KeyCode::Char('j')));
        }
        let middle = draw(&mut app);
        assert!(middle.contains("Card number 29"), "{middle}");
        assert!(!middle.contains("Card number 0 "), "{middle}");

        app.handle_key(KeyEvent::from(KeyCode::Char('j')));
        let bottom = draw(&mut app);
        assert!(bottom.contains("Later (1)"), "{bottom}");
        assert!(bottom.contains("Final card"), "{bottom}");

        // Moving up one card doesn't jump the view: the last one stays.
        app.handle_key(KeyEvent::from(KeyCode::Char('k')));
        let still = draw(&mut app);
        assert!(still.contains("Final card"), "{still}");

        app.handle_key(KeyEvent::from(KeyCode::Home));
        let back = draw(&mut app);
        assert!(back.contains("Mine (30)"), "header comes back\n{back}");
        assert!(back.contains("Card number 0 "), "{back}");
    }

    /// A body with line breaks shows on the card's single body row with the
    /// lines separated by " · " (blank ones dropped), not run together; the
    /// details popup still shows each line on its own.
    #[test]
    fn multi_line_bodies_are_joined_on_the_board() {
        let mut item = card("Mine", "API-1", "Fix login");
        item.card.body = "first line\r\n\n  second line\nthird".to_owned();
        let mut app = app(vec![fresh("plane", "Work", &["Mine"], vec![item])]);

        let board = screen(&app, 100, 24);
        assert!(
            board.contains("first line · second line · third"),
            "{board}"
        );

        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        let details = screen(&app, 100, 24);
        let popup_line = |text: &str| {
            details
                .lines()
                .position(|line| line.contains(text))
                .unwrap_or_else(|| panic!("{text:?} not in the popup\n{details}"))
        };
        assert_eq!(popup_line("second line"), popup_line("first line") + 2);
        assert_eq!(popup_line("third"), popup_line("first line") + 3);
    }

    /// With no sources, the screen explains that none are configured.
    #[test]
    fn no_sources_is_explained() {
        let screen = screen(&app(Vec::new()), 80, 20);

        assert!(screen.contains("No sources configured"), "{screen}");
    }

    /// Informational notices aren't red; failures are.
    #[test]
    fn footer_notices_use_error_color_only_for_failures() {
        let area = Rect::new(0, 0, 100, 20);
        let footer_y = area.height - 1;
        let mut app = work();

        app.set_info("refreshing all sources…");
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, "", now());
        assert_ne!(buf[(0, footer_y)].fg, ratatui::style::Color::Red);

        app.set_notice("could not open link: xdg-open not found");
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, "", now());
        assert_eq!(buf[(0, footer_y)].fg, ratatui::style::Color::Red);
    }

    /// An error in the saved config shows in the panel, saying the previous
    /// one is still in effect.
    #[test]
    fn config_errors_are_shown() {
        let mut snapshot = build_snapshot(
            Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap(),
            Vec::new(),
        );
        snapshot.config_error = Some("invalid config config.toml: unknown kind".to_owned());

        let screen = screen(&App::new(snapshot), 120, 20);

        assert!(screen.contains("config not reloaded"), "{screen}");
        assert!(screen.contains("unknown kind"), "{screen}");
    }

    fn clock_rows(screen: &str) -> usize {
        screen.lines().filter(|line| line.contains('█')).count()
    }

    /// A big clock tops the screen: the date on one line, then the local time
    /// in block digits, centered.
    #[test]
    fn big_clock_shows_the_date_and_time_centered() {
        let screen = screen(&work(), 100, 30);
        let lines: Vec<&str> = screen.lines().collect();

        assert_eq!(lines[0].trim(), "Tuesday, September 29, 2026", "{screen}");
        assert_eq!(
            lines[1].trim(),
            "████    ██  ██          ██████  ██████          ██████  ██████",
            "{screen}"
        );
        // 62 cells wide in 100: 19 blank cells on the left.
        assert!(
            lines[1].starts_with(&format!("{}█", " ".repeat(19))),
            "{screen}"
        );
        assert_eq!(clock_rows(&screen), 5, "{screen}");
        assert!(lines[6].contains("TasksPending"), "{screen}");
    }

    /// The digits grow with the terminal, taking at most a third of its height.
    #[test]
    fn clock_grows_with_the_terminal() {
        assert_eq!(clock_rows(&screen(&work(), 100, 30)), 5);
        assert_eq!(clock_rows(&screen(&work(), 120, 40)), 10);
        assert_eq!(clock_rows(&screen(&work(), 180, 60)), 15);
    }

    /// The clock disappears when the board would get fewer than ten rows, or
    /// when the digits don't fit the width.
    #[test]
    fn clock_hides_when_the_board_would_be_squeezed() {
        // Chrome with three sources: header, tabs, a 5-row Sources panel and
        // the footer take 8 rows; the clock takes 6 at the smallest size.
        let tall_enough = screen(&work(), 100, 24);
        assert_eq!(clock_rows(&tall_enough), 5, "{tall_enough}");

        let too_short = screen(&work(), 100, 23);
        assert_eq!(clock_rows(&too_short), 0, "{too_short}");
        assert!(too_short.lines().next().unwrap().contains("TasksPending"));

        let too_narrow = screen(&work(), 61, 40);
        assert_eq!(clock_rows(&too_narrow), 0, "{too_narrow}");
    }

    /// One Work card with a long title, a body of `body_lines` lines, a due
    /// date and a link.
    fn detailed(body_lines: usize) -> App {
        let body = (0..body_lines)
            .map(|i| format!("body line {i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut item = card("Mine", "API-7", "");
        item.card.title =
            "Migrate the billing service to the new queue and retire the old worker".to_owned();
        item.card.body = body;
        item.card.url = Some("https://plane.example/api/7".to_owned());
        item.card.due_at = Some(Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap());
        app(vec![fresh("plane", "Work", &["Mine"], vec![item])])
    }

    /// Space opens a popup with the whole card: title, source, board and
    /// stack, due date, link and the full body, over the board.
    #[test]
    fn card_details_show_the_whole_card() {
        let mut app = detailed(3);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        let screen = screen(&app, 120, 40);

        let due = local(Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap());
        for expected in [
            "Migrate the billing service to the new queue and retire the old worker",
            "Source   plane",
            "Board    Work",
            "Stack    Mine",
            &due,
            "https://plane.example/api/7",
            "body line 00",
            "body line 01",
            "body line 02",
            "esc close",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in\n{screen}"
            );
        }
    }

    /// The popup is centered and at most 110 × 30, however big the terminal.
    #[test]
    fn card_details_are_centered_and_capped() {
        let mut app = detailed(3);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        let area = Rect::new(0, 0, 200, 60);
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, "", now());

        // (200 - 110) / 2 = 45 and (60 - 30) / 2 = 15.
        assert_eq!(buf[(45, 15)].symbol(), "┌");
        assert_eq!(buf[(154, 15)].symbol(), "┐");
        assert_eq!(buf[(45, 44)].symbol(), "└");
        assert_eq!(buf[(154, 44)].symbol(), "┘");
    }

    /// A long body scrolls: the screen reports how far, and End shows the
    /// last line.
    #[test]
    fn long_card_details_scroll() {
        let mut app = detailed(50);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));

        let (top, viewport) = render_screen(&app, 100, 30);
        assert!(top.contains("body line 00"), "{top}");
        assert!(!top.contains("body line 49"), "{top}");
        assert!(viewport.popup_max_scroll > 0, "{viewport:?}");
        assert!(viewport.popup_page > 0, "{viewport:?}");

        app.set_viewport(viewport);
        app.handle_key(KeyEvent::from(KeyCode::End));
        let bottom = screen(&app, 100, 30);
        assert!(bottom.contains("body line 49"), "{bottom}");
        assert!(!bottom.contains("body line 00"), "{bottom}");
    }

    /// Where `text` starts on the screen, as (column, row).
    fn position(screen: &str, text: &str) -> (u16, u16) {
        screen
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                line.find(text)
                    .map(|byte| (line[..byte].chars().count() as u16, row as u16))
            })
            .unwrap_or_else(|| panic!("{text:?} not on screen\n{screen}"))
    }

    /// The screen reports where each group is, how far it scrolled and how
    /// many cards fit, so a click on a card as drawn selects it, a click on a
    /// stack header as drawn collapses it, and PageDown moves by a page.
    #[test]
    fn clicks_land_on_the_cards_as_drawn() {
        let items = (0..30)
            .map(|i| card("Mine", &format!("c{i}"), &format!("Card number {i}")))
            .collect();
        let mut app = app(vec![
            fresh("plane", "Work", &["Mine"], items),
            fresh(
                "github",
                "Work",
                &["Review"],
                vec![card("Review", "gh-1", "Add cache")],
            ),
        ]);

        let (screen, viewport) = render_screen(&app, 120, 40);
        assert_eq!(viewport.groups.len(), 2, "{viewport:?}");
        assert!(viewport.card_page > 0, "{viewport:?}");
        app.set_viewport(viewport.clone());

        let (x, y) = position(&screen, "Add cache");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected_card().unwrap().id, "gh-1");

        app.handle_key(KeyEvent::from(KeyCode::Char('h')));
        app.handle_key(KeyEvent::from(KeyCode::PageDown));
        let expected = format!("c{}", viewport.card_page);
        assert_eq!(app.selected_card().unwrap().id, expected);

        // Scrolled down, the group reports how far.
        app.handle_key(KeyEvent::from(KeyCode::End));
        let (screen, viewport) = render_screen(&app, 120, 40);
        assert!(viewport.groups[0].scroll > 0, "{viewport:?}");
        app.set_viewport(viewport);
        let (x, y) = position(&screen, "Card number 28");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected_card().unwrap().id, "c28");

        app.handle_key(KeyEvent::from(KeyCode::Home));
        let (screen, viewport) = render_screen(&app, 120, 40);
        app.set_viewport(viewport);
        let (x, y) = position(&screen, "Mine (30)");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.is_collapsed(0, 0));
        assert!(app.selected_card().is_none());
    }

    /// The Sources panel clips long failure messages; s shows them whole.
    #[test]
    fn source_details_show_the_full_failure_message() {
        let message = format!("feed returned 404 {} END", "x".repeat(150));
        let mut app = app(vec![SourceReport {
            name: "calendar".to_owned(),
            board: "Personal".to_owned(),
            columns: vec!["Today".to_owned()],
            icon: None,
            sorts: Default::default(),
            outcome: SourceOutcome::Failed(SourceError::new(message)),
        }]);
        let panel = screen(&app, 100, 30);
        assert!(!panel.contains("END"), "{panel}");

        app.handle_key(KeyEvent::from(KeyCode::Char('s')));
        let screen = screen(&app, 100, 30);

        assert!(screen.contains("END"), "{screen}");
        assert!(screen.contains("calendar"), "{screen}");
        assert!(screen.contains("failed"), "{screen}");
    }
}
