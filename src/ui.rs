use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    layout::Constraint,
    style::{Color, Style},
    widgets::{Block, Cell, Row, Table, TableState},
};

use crate::tracker::{ConnectionStatus, Protocol, Tracker};

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let exp = (bytes as f64).log2() / 10.0;
    let exp = exp.min(UNITS.len() as f64 - 1.0) as usize;
    let value = bytes as f64 / (1024usize.pow(exp as u32) as f64);
    if exp == 0 {
        format!("{} {}", bytes, UNITS[exp])
    } else {
        format!("{:.2} {}", value, UNITS[exp])
    }
}

pub fn run_ui(tracker: Tracker) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, tracker);
    ratatui::restore();
    result
}

fn run_app(terminal: &mut DefaultTerminal, tracker: Tracker) -> io::Result<()> {
    let mut table_state = TableState::default();

    loop {
        terminal.draw(|frame| draw(frame, &tracker, &mut table_state))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                        KeyCode::Down => {
                            let conns = tracker.with_connections(|conns| conns.len());
                            let i = table_state.selected().unwrap_or(0);
                            if i + 1 < conns {
                                table_state.select(Some(i + 1));
                            }
                        }
                        KeyCode::Up => {
                            let i = table_state.selected().unwrap_or(0);
                            if i > 0 {
                                table_state.select(Some(i - 1));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn draw(frame: &mut Frame, tracker: &Tracker, table_state: &mut TableState) {
    use std::sync::atomic::Ordering;

    let header = Row::new(vec![
        Cell::from("Up Time").style(Style::default().fg(Color::Yellow).bold()),
        Cell::from("Upload").style(Style::default().fg(Color::Yellow).bold()),
        Cell::from("Download").style(Style::default().fg(Color::Yellow).bold()),
        Cell::from("Status").style(Style::default().fg(Color::Yellow).bold()),
        Cell::from("Connection").style(Style::default().fg(Color::Yellow).bold()),
        Cell::from("Details").style(Style::default().fg(Color::Yellow).bold()),
    ]);

    tracker.clean_connections();
    let rows: Vec<Row> = tracker.with_connections(|connections| {
        connections
            .iter()
            .map(|conn| {
                let up_time = conn.start_time.elapsed().as_secs();
                let upload = conn.upload_bytes.load(Ordering::Relaxed);
                let download = conn.download_bytes.load(Ordering::Relaxed);

                let status_text = match conn.status {
                    ConnectionStatus::Waiting => "Waiting",
                    ConnectionStatus::Connected => "Connected",
                    ConnectionStatus::CompletedError => "Error",
                    ConnectionStatus::CompletedNormal => "Done",
                };
                let status_color = match conn.status {
                    ConnectionStatus::Waiting => Color::Yellow,
                    ConnectionStatus::Connected => Color::Cyan,
                    ConnectionStatus::CompletedError => Color::Red,
                    ConnectionStatus::CompletedNormal => Color::Green,
                };

                let protocol_str = match conn.protocol {
                    Protocol::Http => "HTTP",
                    Protocol::Https => "HTTPS",
                    Protocol::Socks5Tcp => "SOCKS5/TCP",
                    Protocol::Socks5Udp => "SOCKS5/UDP",
                };

                let connection_str = format!(
                    "{} → {} [{}]",
                    conn.local_addr, conn.remote_uri, protocol_str
                );

                let mut details = String::new();
                for _ in 0..conn.retries.len() {
                    details.push('🔁');
                }
                if let Some(ref err) = conn.error_details {
                    if !details.is_empty() {
                        details.push(' ');
                    }
                    details.push_str(err);
                } else if !conn.retries.is_empty() {
                    if !details.is_empty() {
                        details.push(' ');
                    }
                    details.push_str(conn.retries.last().unwrap());
                }

                Row::new(vec![
                    Cell::from(format!("{}s", up_time)),
                    Cell::from(format_bytes(upload)),
                    Cell::from(format_bytes(download)),
                    Cell::from(status_text).style(Style::default().fg(status_color)),
                    Cell::from(connection_str),
                    Cell::from(details),
                ])
            })
            .collect()
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Min(30),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default())
    .row_highlight_style(Style::default().bg(Color::DarkGray))
    .highlight_symbol("");

    frame.render_stateful_widget(table, frame.area(), table_state);
}
