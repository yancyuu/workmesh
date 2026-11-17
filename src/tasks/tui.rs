use std::io::{IsTerminal, stdout};
use std::time::Duration;

use super::{store::Ledger, views};
use anyhow::{Result, bail};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

struct Screen;
impl Drop for Screen {
    fn drop(&mut self) {
        disable_raw_mode().ok();
        execute!(stdout(), LeaveAlternateScreen).ok();
    }
}

pub fn run() -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!("board requires a terminal; use now --json for agents");
    }
    enable_raw_mode()?;
    let _guard = Screen;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let modes = ["now", "inbox", "blocked", "stale", "needs-me", "all"];
    let mut mode = 0;
    let mut search = String::new();
    let mut editing = false;
    let mut selected = 0usize;
    loop {
        let all = {
            let db = Ledger::open()?;
            db.views()?
        };
        let items = views::select(
            all,
            modes[mode],
            None,
            (!search.is_empty()).then_some(search.as_str()),
            7,
            10000,
        );
        selected = selected.min(items.len().saturating_sub(1));
        let mut state = ListState::default();
        if !items.is_empty() {
            state.select(Some(selected));
        }
        terminal.draw(|frame| {
            let areas = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(3), Constraint::Percentage(45), Constraint::Min(4), Constraint::Length(2)]).split(frame.area());
            frame.render_widget(Paragraph::new(format!("{}  | {} tasks | filter: {}{}", modes[mode], items.len(), views::clean(&search), if editing { "_" } else { "" })).block(Block::default().borders(Borders::ALL).title("WorkMesh / last reported state")), areas[0]);
            let rows = items.iter().map(|v| ListItem::new(format!("{} P{} [{}] {} / {}", views::short(&v.task.id), v.task.priority, if v.conflict {"CONFLICT"}else{v.task.status.label()}, views::clean(&v.task.project), views::clean(&v.task.title))).style(if v.conflict {Style::default().fg(Color::Red)} else {Style::default()})).collect::<Vec<_>>();
            frame.render_stateful_widget(List::new(rows).block(Block::default().borders(Borders::ALL).title("Tasks · read only")).highlight_symbol("> ").highlight_style(Style::default().fg(Color::Cyan)), areas[1], &mut state);
            let detail = items.get(selected).map(|v| format!("{}\nWhy: {}\nNext: {}\nCompleted: {}\nBlocker: {}\nLast report: {} on {}\nHarness: {} · Session: {}\nEvidence (reported, not verified): {}",v.task.id,views::clean(&v.reason),views::clean(&v.task.next),views::clean(&v.task.summary),views::clean(&v.task.blocker),v.updated_at,views::clean(&v.machine),views::clean(v.task.harness.as_deref().unwrap_or("-")),views::clean(v.task.session.as_deref().unwrap_or("-")),views::clean(&v.task.evidence.join("; ")))).unwrap_or_else(|| "Nothing here. Tab to Inbox to triage captured work.".into());
            frame.render_widget(Paragraph::new(detail).wrap(Wrap{trim:false}).block(Block::default().borders(Borders::ALL).title("Task detail")),areas[2]);
            frame.render_widget(Paragraph::new("↑/↓ j/k select · Tab/1–6 view · / search · Esc clear · r refresh · q quit"),areas[3]);
        })?;
        if !event::poll(Duration::from_secs(2))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                break;
            }
            if editing {
                match key.code {
                    KeyCode::Esc => {
                        editing = false;
                        search.clear();
                    }
                    KeyCode::Enter => editing = false,
                    KeyCode::Backspace => {
                        search.pop();
                    }
                    KeyCode::Char(c) => search.push(c),
                    _ => {}
                }
                selected = 0;
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = (selected + 1).min(items.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
                KeyCode::Tab => {
                    mode = (mode + 1) % modes.len();
                    selected = 0;
                }
                KeyCode::Char(c @ '1'..='6') => {
                    mode = (c as u8 - b'1') as usize;
                    selected = 0;
                }
                KeyCode::Char('/') => editing = true,
                _ => {}
            }
        }
    }
    Ok(())
}
