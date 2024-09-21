use crossterm::{
    event::{self, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{prelude::*, widgets::Paragraph};
use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
    thread,
};
use std::{io::stdout, time::Instant};
use std::{net::IpAddr, sync::mpsc, time::Duration};

use super::event::Event;

pub const FRAME_INTERVAL: Duration = Duration::from_millis(400);
const WIDGETS_TIME_LEN: usize = 5;
const WIDGETS_SPEED_LEN: usize = 10;
const DISPLAY_AFTER_DONE: Duration = Duration::from_secs(2);
const KEEP_AFTER_DONE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy)]
enum State {
    Waiting,
    Connected,
    Done,
    Error,
}
impl From<State> for &str {
    fn from(val: State) -> Self {
        match val {
            State::Waiting => "⏳",
            State::Connected => "🔗",
            State::Done => "✅",
            State::Error => "❎",
        }
    }
}

struct Content {
    group: u64,
    time_start: Instant,
    last_update: Instant,
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
    fn new(group: u64, local: IpAddr) -> Self {
        Self {
            group,
            time_start: Instant::now(),
            last_update: Instant::now(),
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

pub struct Summary {
    jobs: Option<BTreeMap<usize, Content>>,
    bit_counter: BTreeMap<u64, (u64, u64)>,
    pub stopped: bool,
}
impl Summary {
    pub fn new() -> Self {
        Self {
            jobs: Some(BTreeMap::new()),
            bit_counter: BTreeMap::new(),
            stopped: false,
        }
    }
    pub fn update(&mut self, id: usize, group: u64, event: Event) {
        match event {
            Event::Upload(n) => {
                self.bit_counter.entry(group).or_default().0 += n as u64;
            }
            Event::Download(n) => {
                self.bit_counter.entry(group).or_default().1 += n as u64;
            }
            _ => {}
        }
        if id != 0 {
            if let Event::Received(ip) = event {
                self.jobs
                    .as_mut()
                    .unwrap()
                    .insert(id, Content::new(group, ip));
            } else {
                let mut index = match self.jobs.as_mut().unwrap().entry(id) {
                    std::collections::btree_map::Entry::Vacant(_) => return,
                    std::collections::btree_map::Entry::Occupied(x) => x,
                };
                let content = index.get_mut();
                content.last_update = Instant::now();
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
                        content.state = State::Done;
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
                        content.state = State::Error;
                        content.addon += &e;
                    }
                    _ => {
                        unreachable!()
                    }
                };
            }
        } else {
            // if self.last_update.elapsed() > FRAME_INTERVAL {
            self.jobs = Some(
                self.jobs
                    .take()
                    .unwrap()
                    .into_iter()
                    .filter(|(_id, content)| match content.state {
                        State::Done | State::Error => {
                            content.last_update.elapsed() < KEEP_AFTER_DONE
                        }
                        _ => true,
                    })
                    .collect(),
            );
        }
        // self
    }
    fn jobs(&self) -> &BTreeMap<usize, Content> {
        self.jobs.as_ref().unwrap()
    }
    pub fn lookup_group(&self, group: u64) -> Option<(u64, u64)> {
        self.bit_counter.get(&group).copied()
    }
}

pub static SUMMARY: LazyLock<Mutex<Summary>> = LazyLock::new(|| Mutex::new(Summary::new()));

pub fn init(recv: mpsc::Receiver<(usize, u64, Event)>, tui: bool) -> std::io::Result<()> {
    thread::spawn(|| {
        for (id, group, event) in recv {
            let mut summary = SUMMARY.lock().unwrap();
            summary.update(id, group, event);
            if summary.stopped {
                break;
            }
        }
    });

    if tui {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || drawer(rx));
        thread::spawn(move || {
            while tx.send(()).is_ok() {
                thread::sleep(FRAME_INTERVAL)
            }
        });
    }

    Ok(())
}

pub fn drawer(recv: mpsc::Receiver<()>) -> std::io::Result<()> {
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

    for () in recv {
        terminal.draw(|frame| {
            // .split(frame.size());
            // let area = frame.size();
            let out_layout = out_layout.split(frame.area());
            frame.render_widget(Paragraph::new(title.clone()), out_layout[0]);
            frame.render_widget(
                Paragraph::new(
                    SUMMARY
                        .lock()
                        .unwrap()
                        .jobs()
                        .iter()
                        .filter(|(_, x)| match x.state {
                            State::Done | State::Error => {
                                x.last_update.elapsed() < DISPLAY_AFTER_DONE
                            }
                            _ => true,
                        })
                        .map(|(_, x)| x.to_line())
                        .collect::<Vec<Line>>(),
                ),
                out_layout[1],
            );
        })?;
        if event::poll(FRAME_INTERVAL)? {
            if let event::Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    // NOT exit in this mode.
                    // SUMMARY.lock().unwrap().stopped = true;
                    // break;
                }
            }
        }
    }
    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
