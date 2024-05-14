use crossterm::{
    event::{self, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{prelude::*, widgets::Paragraph};
use std::{io::stdout, time::Instant};
use std::{net::IpAddr, sync::mpsc, time::Duration};

use super::event::Event;

pub const FRAME_INTERVAL: Duration = Duration::from_millis(200);
const WIDGETS_TIME_LEN: usize = 5;
const WIDGETS_SPEED_LEN: usize = 10;
const KEEP_AFTER_DONE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum State {
    Waiting,
    Connected,
    Done(Instant),
    Error(Instant),
}
impl From<State> for &str {
    fn from(val: State) -> Self {
        match val {
            State::Waiting => "⏳",
            State::Connected => "🔗",
            State::Done(_) => "✅",
            State::Error(_) => "❎",
        }
    }
}

struct Content {
    time_start: Instant,
    local: IpAddr,
    bind: Option<IpAddr>,
    remote: Option<IpAddr>,
    uri: Option<String>,
    state: State,
    upload: usize,
    download: usize,
    addon: String,
}
impl Content {
    fn new(local: IpAddr) -> Self {
        Self {
            time_start: Instant::now(),
            local,
            bind: None,
            remote: None,
            uri: None,
            state: State::Waiting,
            upload: 0,
            download: 0,
            addon: String::new(),
        }
    }
    fn to_line(&self) -> Line {
        let mut res = Vec::with_capacity(6);
        res.push(
            Span::raw(format!(
                "{:>width$}",
                self.time_start.elapsed().as_secs(),
                width = WIDGETS_TIME_LEN
            ))
            .cyan(),
        );
        res.push(
            //🔼🔽
            Span::raw(format!(
                "{:>width$.1} {:>width$.1}",
                self.upload as f32 / 1024f32,
                self.download as f32 / 1024f32,
                width = WIDGETS_SPEED_LEN
            ))
            .light_magenta(),
        );

        let icon: &str = self.state.into();
        res.push(Span::raw(icon));

        // res.push(Span::raw(self.local.to_string()).light_blue());
        if let Some(ip) = &self.bind {
            res.push(Span::raw(ip.to_string()).cyan());
            res.push(Span::raw(" "));
        }
        if let Some(uri) = &self.uri {
            res.push(Span::raw(uri).blue().bold());
        }

        res.push(Span::raw(" "));
        res.push(Span::raw(&self.addon).bold());

        res.into()
    }
}

pub fn drawer(recv: mpsc::Receiver<(usize, Event)>) -> std::io::Result<()> {
    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    let mut jobs = Vec::new();
    let title: Line = vec![
        Span::raw(format!("{:>width$}", "time", width = WIDGETS_TIME_LEN)).cyan(),
        Span::raw(format!(
            "{:>width$} {:>width$}",
            "⇧KB",
            "⇩KB",
            width = WIDGETS_SPEED_LEN
        ))
        .light_magenta(),
        Span::raw("🔰").blue().bold(),
    ]
    .into();

    let out_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(1), Constraint::Fill(1)]);

    for (id, event) in recv {
        if id != 0 {
            if let Event::Received(ip) = event {
                jobs.push((id, Content::new(ip)));
            } else {
                let index = match jobs.binary_search_by_key(&id, |x| x.0) {
                    Ok(x) => x,
                    Err(_) => continue,
                };
                let content = &mut jobs.get_mut(index).unwrap().1;
                match event {
                    Event::Resolved(uri) => {
                        content.uri = Some(uri);
                    }
                    Event::Connected(bind, remote) => {
                        content.bind = Some(bind);
                        content.remote = Some(remote);
                        content.state = State::Connected;
                    }
                    Event::Done() => {
                        content.state = State::Done(Instant::now());
                    }
                    Event::Upload(n) => {
                        content.upload += n;
                    }
                    Event::Download(n) => {
                        content.download += n;
                    }
                    Event::Retry() => {
                        content.addon.push('🔁');
                    }
                    Event::Error(e) => {
                        content.state = State::Error(Instant::now());
                        content.addon += &e;
                    }
                    _ => {
                        unreachable!()
                    }
                };
            }
        } else {
            jobs.retain(|(_id, content)| match content.state {
                State::Done(t) | State::Error(t) => t.elapsed() < KEEP_AFTER_DONE,
                _ => true,
            });
            terminal.draw(|frame| {
                // .split(frame.size());
                // let area = frame.size();
                let out_layout = out_layout.split(frame.size());
                frame.render_widget(Paragraph::new(title.clone()), out_layout[0]);
                frame.render_widget(
                    Paragraph::new(jobs.iter().map(|(_, x)| x.to_line()).collect::<Vec<Line>>()),
                    out_layout[1],
                );
            })?;
            if event::poll(FRAME_INTERVAL)? {
                if let event::Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                        break;
                    }
                }
            }
        }
    }
    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
