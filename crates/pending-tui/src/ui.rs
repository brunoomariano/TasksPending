//! Rendering of the dashboard as a kanban: board tabs, then columns grouped
//! by source.

use chrono::{DateTime, Local, Utc};
use pending_core::{CardSeverity, SourceHealth, SourceStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};

use crate::app::{App, ColumnRef};

const HELP: &str = "1-9/tab boards · h/l columns · j/k cards · enter open · r refresh · q quit";
/// Narrowest useful column; with less room the board scrolls sideways.
const MIN_COLUMN_WIDTH: u16 = 26;

fn local(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// `header` describes where the config came from.
pub fn render(area: Rect, buf: &mut Buffer, app: &App, header: &str) {
    let snapshot = app.snapshot();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(
                snapshot.sources.len().max(1) as u16
                    + u16::from(snapshot.config_error.is_some())
                    + 2,
            ),
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

fn source_line(source: &SourceHealth) -> Line<'_> {
    let (label, color) = match source.status {
        SourceStatus::Ready => ("ready", Color::Green),
        SourceStatus::Refreshing => ("refreshing", Color::Blue),
        SourceStatus::Degraded => ("degraded", Color::Yellow),
        SourceStatus::Failed => ("failed", Color::Red),
    };
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
    use chrono::{TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent};
    use pending_core::{
        CardSeverity, PendingCard, SourceBatch, SourceError, SourceItem, SourceOutcome,
        SourceReport, build_snapshot,
    };
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;
    use crate::app::App;

    fn screen(app: &App, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        render(
            area,
            &mut buf,
            app,
            "config: ~/.config/tasks-pending/config.toml",
        );
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
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
        render(area, &mut buf, &app, "");
        assert_ne!(buf[(0, footer_y)].fg, ratatui::style::Color::Red);

        app.set_notice("could not open link: xdg-open not found");
        let mut buf = Buffer::empty(area);
        render(area, &mut buf, &app, "");
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
}
