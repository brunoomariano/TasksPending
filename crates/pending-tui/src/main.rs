use std::io;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use crossterm::ExecutableCommand;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pending_core::{CardSeverity, DashboardSnapshot, sample_snapshot};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Widget, Wrap};

#[derive(Debug, Parser)]
#[command(
    name = "pending-tui",
    about = "TasksPending terminal dashboard",
    version
)]
struct Cli {}

const TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M UTC";
const INPUT_POLL: Duration = Duration::from_millis(250);

struct TerminalSession;

impl TerminalSession {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        io::stdout()
            .execute(EnterAlternateScreen)
            .context("entering alternate screen")?;
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = io::stdout().execute(Show);
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
    }
}

fn main() -> anyhow::Result<()> {
    Cli::parse();
    let _session = TerminalSession::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("opening terminal")?;
    let snapshot = sample_snapshot();

    loop {
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &snapshot))
            .context("drawing TUI")?;

        if event::poll(INPUT_POLL)?
            && matches!(event::read()?, Event::Key(key) if key.code == KeyCode::Char('q'))
        {
            break;
        }
    }

    terminal.show_cursor().context("showing cursor")?;
    Ok(())
}

fn render(area: Rect, buf: &mut ratatui::buffer::Buffer, snapshot: &DashboardSnapshot) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let title = Line::from(vec![
        Span::styled(
            "TasksPending",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  generated {}",
            snapshot.generated_at.format(TIMESTAMP_FORMAT)
        )),
        Span::styled("  press q to quit", Style::default().fg(Color::DarkGray)),
    ]);
    Paragraph::new(title)
        .block(Block::default().borders(Borders::ALL))
        .render(layout[0], buf);

    let lane_count = snapshot.lanes.len().max(1);
    let constraints = vec![Constraint::Ratio(1, lane_count as u32); lane_count];
    let lanes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(layout[1]);

    for (lane, area) in snapshot.lanes.iter().zip(lanes.iter()) {
        let mut items = Vec::new();
        for section in &lane.sections {
            items.push(ListItem::new(Line::from(Span::styled(
                section.name.as_str(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))));

            for card in &section.cards {
                let severity = match card.severity {
                    CardSeverity::Info => Style::default().fg(Color::Blue),
                    CardSeverity::Warning => Style::default().fg(Color::Yellow),
                    CardSeverity::Critical => Style::default().fg(Color::Red),
                };
                items.push(ListItem::new(vec![
                    Line::from(vec![
                        Span::styled("  ", severity),
                        Span::styled(card.title.as_str(), Style::default().fg(Color::White)),
                    ]),
                    Line::from(Span::styled(
                        format!(
                            "    {} · {}",
                            card.source,
                            card.updated_at.format(TIMESTAMP_FORMAT)
                        ),
                        Style::default().fg(Color::DarkGray),
                    )),
                    Line::from(Span::raw(format!("    {}", card.body))),
                ]));
            }
        }

        List::new(items)
            .block(
                Block::default()
                    .title(lane.name.as_str())
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().add_modifier(Modifier::BOLD))
            .render(*area, buf);
    }

    if snapshot.lanes.is_empty() {
        Paragraph::new("No pending lanes yet.")
            .wrap(Wrap { trim: true })
            .render(layout[1], buf);
    }
}
