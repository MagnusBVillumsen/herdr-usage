mod limits;
mod machines;
mod shell;

use std::io::{IsTerminal, Write, stdout};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use chrono::{Local, TimeZone};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, terminal};

use limits::Window;
use machines::{Machine, Status};

const PLAN_PERIOD: Duration = Duration::from_secs(120);
const MACHINE_PERIOD: Duration = Duration::from_secs(5);

type Plan = Option<Result<Vec<Window>, String>>;

#[derive(Default)]
struct State {
    claude: Plan,
    codex: Plan,
    machines: Vec<(Machine, Option<Status>)>,
    probed: Option<i64>,
}

type Line = Vec<(Option<Color>, String)>;

fn level(p: f64) -> Color {
    match p {
        p if p >= 90.0 => Color::Red,
        p if p >= 70.0 => Color::Yellow,
        _ => Color::Green,
    }
}

fn bar(line: &mut Line, p: f64, width: usize) {
    let filled = ((p / 100.0 * width as f64).round() as usize).min(width);
    line.push((Some(level(p)), "█".repeat(filled)));
    line.push((Some(Color::DarkGrey), "░".repeat(width - filled)));
}

fn until(ts: i64) -> String {
    let d = (ts - limits::now()).max(0);
    let (days, hours, mins) = (d / 86400, d % 86400 / 3600, d % 3600 / 60);
    match (days, hours) {
        (0, 0) => format!("{mins}m"),
        (0, _) => format!("{hours}h {mins:02}m"),
        _ => format!("{days}d {hours}h"),
    }
}

fn clock(ts: i64) -> String {
    let Some(t) = Local.timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    if ts - limits::now() < 20 * 3600 {
        t.format("%H:%M").to_string()
    } else {
        t.format("%a %H:%M").to_string()
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

fn first_line(e: &str, max: usize) -> String {
    let line = e.lines().next().unwrap_or_default();
    match line.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_owned(),
    }
}

fn plan_lines(lines: &mut Vec<Line>, name: &str, plan: &Plan) {
    let head = format!("  {name:<9}");
    match plan {
        None => lines.push(vec![
            (None, head),
            (Some(Color::DarkGrey), "loading…".into()),
        ]),
        Some(Err(e)) => lines.push(vec![(None, head), (Some(Color::Yellow), first_line(e, 70))]),
        Some(Ok(windows)) if windows.is_empty() => lines.push(vec![
            (None, head),
            (Some(Color::DarkGrey), "no limits reported".into()),
        ]),
        Some(Ok(windows)) => {
            for (i, w) in windows.iter().enumerate() {
                let label = if i == 0 { head.clone() } else { " ".repeat(11) };
                let mut line = vec![(None, label), (None, format!("{:<7}", w.label))];
                bar(&mut line, w.used, 24);
                line.push((Some(level(w.used)), format!(" {:>3.0}%", w.used)));
                if let Some(ts) = w.resets {
                    line.push((
                        Some(Color::DarkGrey),
                        format!("   resets in {} · {}", until(ts), clock(ts)),
                    ));
                }
                lines.push(line);
            }
        }
    }
}

fn machine_line(m: &Machine, status: &Option<Status>) -> Line {
    let name = format!("{:<12}", first_line(&m.label, 11));
    let Some(s) = status else {
        return vec![
            (Some(Color::DarkGrey), "  · ".into()),
            (None, name),
            (Some(Color::DarkGrey), "checking…".into()),
        ];
    };
    let l = match &s.load {
        Ok(l) => l,
        Err(e) => {
            return vec![
                (Some(Color::DarkGrey), "  ○ ".into()),
                (Some(Color::DarkGrey), name),
                (
                    Some(Color::DarkGrey),
                    format!("offline  {}", first_line(&e.to_string(), 54)),
                ),
            ];
        }
    };
    let mut line = vec![(Some(Color::Green), "  ● ".into()), (None, name)];
    bar(&mut line, l.cpu, 10);
    line.push((None, format!(" {:>3.0}%   ", l.cpu)));
    let mem = l.mem_used as f64 / l.mem_total as f64 * 100.0;
    bar(&mut line, mem, 10);
    line.push((
        None,
        format!(
            " {:>5.1}/{:<5.0}GB  {:>5.1}/{:<3}",
            gib(l.mem_used),
            gib(l.mem_total),
            l.load,
            l.cores
        ),
    ));
    if let Some(a) = &s.agents {
        let parts = [
            (a.working, "working", Color::Cyan),
            (a.blocked, "waiting", Color::Yellow),
            (a.idle, "idle", Color::DarkGrey),
        ];
        let mut first = true;
        line.push((None, "  ".into()));
        for (n, what, color) in parts.into_iter().filter(|p| p.0 > 0) {
            if !first {
                line.push((Some(Color::DarkGrey), " · ".into()));
            }
            first = false;
            line.push((Some(color), format!("{n} {what}")));
        }
        if first {
            line.push((Some(Color::DarkGrey), "no agents".into()));
        }
    }
    line
}

fn render(state: &State) -> Vec<Line> {
    let dim = |s: &str| vec![(Some(Color::DarkGrey), s.to_owned())];
    let mut lines = vec![dim(" Plan limits"), vec![]];
    plan_lines(&mut lines, "Claude", &state.claude);
    plan_lines(&mut lines, "Codex", &state.codex);
    lines.push(vec![]);
    lines.push(dim(
        " Machines        CPU                RAM                         load     agents",
    ));
    lines.push(vec![]);
    for (m, s) in &state.machines {
        lines.push(machine_line(m, s));
    }
    lines
}

fn draw(out: &mut impl Write, lines: &[Line], eol: &str, color: bool) -> Result<()> {
    for line in lines {
        for (c, text) in line {
            match c.filter(|_| color) {
                Some(c) => queue!(out, SetForegroundColor(c), Print(text), ResetColor)?,
                None => queue!(out, Print(text))?,
            }
        }
        queue!(out, Print(eol))?;
    }
    Ok(out.flush()?)
}

/// Runs `work` now and then every `period`, or sooner when woken.
fn every(period: Duration, wake: mpsc::Receiver<()>, mut work: impl FnMut()) {
    loop {
        work();
        if let Err(mpsc::RecvTimeoutError::Disconnected) = wake.recv_timeout(period) {
            break;
        }
    }
}

fn fetch_plans(state: &Mutex<State>) {
    let (claude, codex) = thread::scope(|s| {
        let claude = s.spawn(limits::claude);
        let codex = s.spawn(limits::codex);
        (claude.join(), codex.join())
    });
    let flat = |r: thread::Result<Result<Vec<Window>>>| match r {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err("crashed".into()),
    };
    let mut st = state.lock().unwrap();
    st.claude = Some(flat(claude));
    st.codex = Some(flat(codex));
}

fn probe_machines(state: &Mutex<State>, machines: &[Machine]) {
    let statuses: Vec<Status> = thread::scope(|s| {
        let handles: Vec<_> = machines
            .iter()
            .map(|m| s.spawn(move || machines::probe(m)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut st = state.lock().unwrap();
    for ((_, slot), status) in st.machines.iter_mut().zip(statuses) {
        *slot = Some(status);
    }
    st.probed = Some(limits::now());
}

fn once() -> Result<()> {
    let machines = machines::list();
    let state = Mutex::new(State {
        machines: machines.iter().map(|m| (m.clone(), None)).collect(),
        ..State::default()
    });
    thread::scope(|s| {
        s.spawn(|| fetch_plans(&state));
        s.spawn(|| probe_machines(&state, &machines));
    });
    let lines = render(&state.lock().unwrap());
    draw(&mut stdout(), &lines, "\n", stdout().is_terminal())
}

struct Screen;

impl Screen {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(Screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--once") {
        return once();
    }
    let machines = machines::list();
    let state = Arc::new(Mutex::new(State {
        machines: machines.iter().map(|m| (m.clone(), None)).collect(),
        ..State::default()
    }));
    let (plan_wake, plan_rx) = mpsc::channel();
    let (machine_wake, machine_rx) = mpsc::channel();
    {
        let state = state.clone();
        thread::spawn(move || every(PLAN_PERIOD, plan_rx, || fetch_plans(&state)));
    }
    {
        let state = state.clone();
        thread::spawn(move || {
            every(MACHINE_PERIOD, machine_rx, || {
                probe_machines(&state, &machines)
            })
        });
    }

    let _screen = Screen::enter()?;
    let mut out = stdout();
    loop {
        let st = state.lock().unwrap();
        let updated = st
            .probed
            .map(|ts| format!("updated {}", clock(ts)))
            .unwrap_or_default();
        let mut lines = vec![vec![]];
        lines.extend(render(&st));
        drop(st);
        lines.push(vec![]);
        lines.push(vec![(
            Some(Color::DarkGrey),
            format!(" r refresh · q close{updated:>58}"),
        )]);
        queue!(
            out,
            cursor::MoveTo(0, 0),
            terminal::Clear(terminal::ClearType::All)
        )?;
        draw(&mut out, &lines, "\r\n", true)?;

        if !event::poll(Duration::from_millis(500))? {
            continue;
        }
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match k.code {
                KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => break,
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('r') => {
                    let _ = plan_wake.send(());
                    let _ = machine_wake.send(());
                }
                _ => {}
            }
        }
    }
    Ok(())
}
