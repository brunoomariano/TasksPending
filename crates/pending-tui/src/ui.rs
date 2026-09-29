//! Rendering of the dashboard as a kanban: board tabs, then columns grouped
//! by source.

use chrono::{DateTime, Local, Utc};
use pending_core::{CardSeverity, PendingCard, SourceHealth, SourceStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap,
};

use crate::app::{App, ColumnRef, Popup, Viewport};
use crate::clock;

const HELP: &str = "1-9/tab boards · h/l columns · j/k cards · enter open · space details · s sources · r refresh · q quit";
/// Largest details popup, so long lines stay readable on wide terminals.
const POPUP_MAX_WIDTH: u16 = 110;
const POPUP_MAX_HEIGHT: u16 = 30;
/// Narrowest useful column; with less room the board scrolls sideways.
const MIN_COLUMN_WIDTH: u16 = 26;
/// Fewest rows the board keeps (about three cards per column); the clock
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

    render_board(layout[3], buf, app);

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
    if let Some(column) = app.columns().get(app.selected_column()) {
        lines.push(field("Source", column.source.to_owned()));
        if let Some(board) = app.current_board() {
            lines.push(field("Board", board.name.clone()));
        }
        lines.push(field("Column", column.column.name.clone()));
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

/// The first visible column and how many fit, keeping `selected` in view.
fn visible_window(total: usize, selected: usize, width: u16) -> (usize, usize) {
    let fit = usize::from((width / MIN_COLUMN_WIDTH).max(1)).min(total);
    let start = selected.saturating_sub(fit - 1).min(total - fit);
    (start, fit)
}

fn render_board(area: Rect, buf: &mut Buffer, app: &App) {
    let columns = app.columns();
    if columns.is_empty() {
        Paragraph::new("No columns on this board.")
            .block(Block::default().borders(Borders::ALL))
            .render(area, buf);
        return;
    }

    let (start, fit) = visible_window(columns.len(), app.selected_column(), area.width);
    let visible = &columns[start..start + fit];

    // Consecutive columns of the same source share one titled group box.
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (offset, column) in visible.iter().enumerate() {
        match groups.last_mut() {
            Some((source, indexes)) if *source == column.source => indexes.push(start + offset),
            _ => groups.push((column.source, vec![start + offset])),
        }
    }

    let group_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            groups
                .iter()
                .map(|(_, indexes)| Constraint::Ratio(indexes.len() as u32, fit as u32))
                .collect::<Vec<_>>(),
        )
        .split(area);

    for ((source, indexes), group_area) in groups.iter().zip(group_areas.iter()) {
        let block = Block::default()
            .title(Span::styled(
                format!(" {source} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL);
        let inner = block.inner(*group_area);
        block.render(*group_area, buf);

        let column_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Ratio(1, indexes.len() as u32);
                indexes.len()
            ])
            .split(inner);
        for (&index, column_area) in indexes.iter().zip(column_areas.iter()) {
            render_column(*column_area, buf, app, index, &columns[index]);
        }
    }
}

fn render_column(area: Rect, buf: &mut Buffer, app: &App, index: usize, column: &ColumnRef<'_>) {
    let selected_column = index == app.selected_column();
    let title_style = if selected_column {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(Color::Yellow)
    };
    let block = Block::default()
        .title(Span::styled(
            format!("{} ({})", column.column.name, column.column.cards.len()),
            title_style,
        ))
        .borders(Borders::TOP);

    if column.column.cards.is_empty() {
        Paragraph::new(Span::styled("—", Style::default().fg(Color::DarkGray)))
            .block(block)
            .render(area, buf);
        return;
    }

    let mut state = ListState::default();
    let items: Vec<ListItem> = column
        .column
        .cards
        .iter()
        .enumerate()
        .map(|(position, card)| {
            let marker = match card.severity {
                CardSeverity::Info => Style::default().fg(Color::Blue),
                CardSeverity::Warning => Style::default().fg(Color::Yellow),
                CardSeverity::Critical => Style::default().fg(Color::Red),
            };
            let title_style = if app.is_selected(index, position) {
                state.select(Some(position));
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled("▍", marker),
                    Span::styled(card.title.as_str(), title_style),
                ]),
                Line::from(Span::styled(
                    format!(" {}", card.body),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    StatefulWidget::render(List::new(items).block(block), area, buf, &mut state);
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent};
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
                outcome: SourceOutcome::Failed(SourceError::new("feed returned 404")),
            },
        ])
    }

    /// The screen shows boards as tabs, each tool as a group of columns with
    /// their card counts, and the health of the sources.
    #[test]
    fn boards_are_tabs_and_sources_are_groups_of_columns() {
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
            "h/l columns",
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

    /// With more columns than fit the width, the board scrolls sideways to
    /// keep the selected column visible.
    #[test]
    fn wide_boards_scroll_to_the_selected_column() {
        let names: Vec<String> = (0..8).map(|i| format!("Col{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut app = app(vec![fresh(
            "plane",
            "Work",
            &names,
            vec![card("Col7", "last", "Last column card")],
        )]);
        for _ in 0..7 {
            app.handle_key(KeyEvent::from(KeyCode::Char('l')));
        }

        let screen = screen(&app, 80, 24);

        assert!(screen.contains("Col7 (1)"), "{screen}");
        assert!(screen.contains("Last column card"), "{screen}");
        assert!(!screen.contains("Col0"), "{screen}");
    }

    /// With more cards than fit the height, the column scrolls to keep the
    /// selected card visible.
    #[test]
    fn long_columns_keep_the_selected_card_visible() {
        let items = (0..30)
            .map(|i| card("Mine", &format!("c{i}"), &format!("Card number {i}")))
            .collect();
        let mut app = app(vec![fresh("plane", "Work", &["Mine"], items)]);
        for _ in 0..29 {
            app.handle_key(KeyEvent::from(KeyCode::Char('j')));
        }

        let screen = screen(&app, 80, 24);

        assert!(screen.contains("Card number 29"), "{screen}");
        assert!(!screen.contains("Card number 0 "), "{screen}");
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
    /// column, due date, link and the full body, over the board.
    #[test]
    fn card_details_show_the_whole_card() {
        let mut app = detailed(3);
        app.handle_key(KeyEvent::from(KeyCode::Char(' ')));
        let screen = screen(&app, 120, 40);

        let due = local(Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap());
        for expected in [
            "Migrate the billing service to the new queue and retire the old worker",
            "plane",
            "Work",
            "Mine",
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

    /// The Sources panel clips long failure messages; s shows them whole.
    #[test]
    fn source_details_show_the_full_failure_message() {
        let message = format!("feed returned 404 {} END", "x".repeat(150));
        let mut app = app(vec![SourceReport {
            name: "calendar".to_owned(),
            board: "Personal".to_owned(),
            columns: vec!["Today".to_owned()],
            icon: None,
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
