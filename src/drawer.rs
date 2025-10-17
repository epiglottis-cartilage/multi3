use crossterm::{
    ExecutableCommand,
    event::{self as cross_event, KeyCode, KeyEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{prelude::*, widgets::Paragraph};
use std::{
    io::stdout,
    net::{IpAddr, SocketAddr},
    sync::mpsc,
    time::Duration,
    time::Instant,
};

use crate::event::Protocol;

use super::event::Event;

pub const EXIT_KEY: char = 'q';
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
#[allow(dead_code)]
struct Content {
    time_start: Instant,
    local: IpAddr,
    protocol: Option<Protocol>,
    bind: Option<IpAddr>,
    remote: Option<SocketAddr>,
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
            protocol: None,
            bind: None,
            remote: None,
            uri: None,
            state: State::Waiting,
            upload: 0,
            download: 0,
            addon: String::new(),
        }
    }
    fn to_line(&self) -> Line<'_> {
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
        let protocol: &str = self.protocol.as_ref().map(|p| p.display()).unwrap_or("..");
        res.push(Span::raw(protocol));
        if let Some(uri) = &self.uri {
            res.push(Span::raw(uri).blue().bold());
        }

        res.push(Span::raw(" "));
        res.push(Span::raw(&self.addon).bold());

        res.into()
    }
}

struct Summary(Vec<(usize, Content)>);
impl Summary {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn update(&mut self, id: usize, event: Event) {
        if id != 0 {
            if let Event::Received(ip) = event {
                self.0.push((id, Content::new(ip)));
                self.0.sort_by_key(|(id, _)| *id);
            } else {
                let content = match self.0.binary_search_by_key(&id, |(id, _)| *id) {
                    Ok(index) => &mut self.0[index].1,
                    Err(_) => return,
                };
                match event {
                    Event::Received(_) => {
                        unreachable!()
                    }
                    Event::Recognized(p) => {
                        content.protocol = Some(p);
                    }
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
                    Event::None => {}
                };
            }
        } else {
            self.0.retain(|(_, content)| match content.state {
                State::Done(t) | State::Error(t) => t.elapsed() < KEEP_AFTER_DONE,
                _ => true,
            });
        }
        // self
    }
}

pub fn drawer(recv: mpsc::Receiver<(usize, Event)>) -> std::io::Result<()> {
    stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

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

    let mut summary = Summary::new();
    let mut exit_reason = None;
    for (id, event) in recv {
        if id == 0
            && let Event::Error(reason) = event
        {
            exit_reason = Some(format!("Exiting since {:?}", reason));
            break;
        }

        summary.update(id, event);
        if id == 0 {
            terminal.draw(|frame| {
                // .split(frame.size());
                // let area = frame.size();
                let out_layout = out_layout.split(frame.area());
                frame.render_widget(Paragraph::new(title.clone()), out_layout[0]);
                frame.render_widget(
                    Paragraph::new(
                        summary
                            .0
                            .iter()
                            .rev()
                            .map(|(_, x)| x.to_line())
                            .collect::<Vec<Line>>(),
                    ),
                    out_layout[1],
                );
            })?;
            if cross_event::poll(Duration::new(0, 0))? {
                if let cross_event::Event::Key(key) = cross_event::read()? {
                    if key.kind == KeyEventKind::Press && key.code == KeyCode::Char(EXIT_KEY) {
                        exit_reason = Some(format!("Exiting on user request"));
                        break;
                    }
                }
            }
        }
    }
    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    if let Some(reason) = exit_reason {
        println!("{}", reason);
    }
    Ok(())
}
