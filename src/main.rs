use std::io::{self, Write};
use std::fs;
use std::path::Path;
use std::process::Command;

use crossterm::{
    cursor::MoveTo,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Row, Table, Cell},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};

// ---------------- Configuration ----------------
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    todo_file_path: String,
    cyber_resource_file_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            todo_file_path: "md/TODO.md".into(),
            cyber_resource_file_path: "md/CyberResource.md".into(),
        }
    }
}

fn load_config() -> Config {
    let path = "config.toml";
    if Path::new(path).exists() {
        match fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    } else {
        let cfg = Config::default();
        if let Ok(text) = toml::to_string_pretty(&cfg) { let _ = fs::write(path, text); }
        cfg
    }
}

// ---------------- IO helpers ----------------
fn read_todo(cfg: &Config) -> Result<String, Box<dyn std::error::Error>> {
    // 不再创建默认文件，若不存在则返回 Err
    Ok(fs::read_to_string(&cfg.todo_file_path)?)
}

fn read_cyber(cfg: &Config) -> Result<String, Box<dyn std::error::Error>> {
    // 不再创建默认文件，若不存在则返回 Err
    Ok(fs::read_to_string(&cfg.cyber_resource_file_path)?)
}

// ---------------- Parsing ----------------
fn parse_table(content: &str, min_cols: usize) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('|') && t.ends_with('|') {
            let cells: Vec<String> = t.split('|').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            if cells.len() >= min_cols { out.push(cells); }
        }
    }
    out
}

// ---------------- Table rendering ----------------
fn render_table_generic(
    f: &mut Frame,
    area: Rect,
    rows: &[Vec<String>],
    scroll: usize,
    title: &str,
) {
    if rows.is_empty() {
        let block = Block::default().borders(Borders::ALL).title(title);
        let msg = Paragraph::new("未找到").alignment(Alignment::Center).block(block);
        f.render_widget(msg, area);
        return;
    }

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        let block = Block::default().borders(Borders::ALL).title(title);
        let msg = Paragraph::new("未找到").alignment(Alignment::Center).block(block);
        f.render_widget(msg, area);
        return;
    }

    let mut max_w = vec![0usize; cols];
    for row in rows {
        for i in 0..cols {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            max_w[i] = max_w[i].max(cell.chars().count());
        }
    }
    for w in &mut max_w { if *w < 8 { *w = 8; } if *w > 50 { *w = 50; } }
    let cons: Vec<Constraint> = max_w.iter().map(|&w| Constraint::Length(w as u16)).collect();

    let h = area.height.saturating_sub(2) as usize; // roughly visible rows inside border
    let start = scroll.min(rows.len());
    let mut end = start.saturating_add(h);
    if end > rows.len() { end = rows.len(); }
    let vis = &rows[start..end];

    let table_rows: Vec<Row> = vis.iter().enumerate().map(|(i, row)| {
        let mut cells: Vec<Cell> = Vec::with_capacity(cols);
        for c in 0..cols { cells.push(Cell::from(row.get(c).map(|s| s.as_str()).unwrap_or(""))); }
        let style = if start + i == 0 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else { Style::default() };
        Row::new(cells).style(style)
    }).collect();

    let table = Table::new(table_rows, cons).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(table, area);
}

// ---------------- External editor ----------------
fn open_in_neovim(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    disable_raw_mode()?;
    let status = Command::new("nvim").arg(path).status()?;
    execute!(std::io::stdout(), Clear(ClearType::All), Clear(ClearType::Purge), MoveTo(1, 1))?;
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    std::io::stdout().flush()?;
    if !status.success() { return Err("Failed to open neovim".into()); }
    Ok(())
}

// ---------------- UI & State ----------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppState { MainMenu, TodoView, CyberView }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem { Todo, Bill, Cyber }
impl MenuItem {
    fn all() -> [MenuItem; 3] { [MenuItem::Todo, MenuItem::Bill, MenuItem::Cyber] }
    fn title(&self) -> &'static str { match self { MenuItem::Todo => "TODO", MenuItem::Bill => "BILL", MenuItem::Cyber => "CYBER RESOURCE" } }
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, cfg: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut selected = 0usize;
    let items = MenuItem::all();
    let mut last_msg: Option<String> = None;
    let mut list_state = ListState::default(); list_state.select(Some(0));
    let mut state = AppState::MainMenu;
    let mut force_redraw = false;

    let mut todo: Vec<Vec<String>> = Vec::new();
    let mut todo_scroll = 0usize;

    let mut cyber: Vec<Vec<String>> = Vec::new();
    let mut cyber_scroll = 0usize;

    loop {
        list_state.select(Some(selected));
        if force_redraw { terminal.clear()?; force_redraw = false; }

        terminal.draw(|f| {
            let size = f.size();
            match state {
                AppState::MainMenu => {
                    let chunks = Layout::default().direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)])
                        .split(size);

                    let header = Paragraph::new(Line::from(vec![
                        Span::styled("Jeek!", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                        Span::raw("\t Exist before meaning, feel yourself, embrace imperfection."),
                    ])).alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).title("Hello"));
                    f.render_widget(header, chunks[0]);

                    let list_items: Vec<ListItem> = items.iter().enumerate().map(|(i, item)| {
                        let style = if i == selected { Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD) } else { Style::default() };
                        ListItem::new(Line::from(Span::styled(format!(" {} : {} ", i, item.title()), style)))
                    }).collect();
                    let list = List::new(list_items)
                        .block(Block::default().borders(Borders::ALL).title("Menu"))
                        .highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
                        .highlight_symbol("→ ");
                    f.render_stateful_widget(list, chunks[1], &mut list_state);

                    let help = match &last_msg { Some(m) => m.as_str(), None => "\tjk -- move, Enter -- select, q -- exit" };
                    let footer = Paragraph::new(help).alignment(Alignment::Left).block(Block::default().borders(Borders::ALL).title("Message"));
                    f.render_widget(footer, chunks[2]);
                }
                AppState::TodoView => {
                    let chunks = Layout::default().direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)]).split(size);
                    let header = Paragraph::new("TODO List").alignment(Alignment::Center)
                        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                        .block(Block::default().borders(Borders::ALL).title("TODO"));
                    f.render_widget(header, chunks[0]);

                    render_table_generic(f, chunks[1], &todo, todo_scroll, "Tasks");

                    let help = Paragraph::new("jk -- move | q -- back | e -- edit | r -- refresh").alignment(Alignment::Center).block(Block::default().borders(Borders::ALL).title("Help"));
                    f.render_widget(help, chunks[2]);
                }
                AppState::CyberView => {
                    let chunks = Layout::default().direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(3)]).split(size);
                    let header = Paragraph::new("Cyber Resource List").alignment(Alignment::Center)
                        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                        .block(Block::default().borders(Borders::ALL).title("Cyber Resource"));
                    f.render_widget(header, chunks[0]);

                    render_table_generic(f, chunks[1], &cyber, cyber_scroll, "Resources");

                    let help = Paragraph::new("jk -- move | q -- back | e -- edit | r -- refresh").alignment(Alignment::Center).block(Block::default().borders(Borders::ALL).title("Help"));
                    f.render_widget(help, chunks[2]);
                }
            }
        })?;

        if event::poll(std::time::Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match state {
                        AppState::MainMenu => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Up | KeyCode::Char('k') => { selected = if selected == 0 { items.len()-1 } else { selected-1 }; last_msg = None; }
                            KeyCode::Down | KeyCode::Char('j') => { selected = (selected + 1) % items.len(); last_msg = None; }
                            KeyCode::Enter => match items[selected] {
                                MenuItem::Todo => {
                                    match read_todo(cfg) {
                                        Ok(s) => { todo = parse_table(&s, 1); }
                                        Err(_) => { todo.clear(); }
                                    }
                                    todo_scroll = 0; state = AppState::TodoView; last_msg = None;
                                },
                                MenuItem::Cyber => {
                                    match read_cyber(cfg) {
                                        Ok(s) => { cyber = parse_table(&s, 1); }
                                        Err(_) => { cyber.clear(); }
                                    }
                                    cyber_scroll = 0; state = AppState::CyberView; last_msg = None;
                                },
                                _ => { let title = items[selected].title(); last_msg = Some(format!("已选择 {}（功能未实现）", title)); }
                            },
                            _ => {}
                        },
                        AppState::TodoView => match key.code {
                            KeyCode::Char('q') => { state = AppState::MainMenu; todo_scroll = 0; }
                            KeyCode::Char('e') => {
                                if open_in_neovim(&cfg.todo_file_path).is_ok() {
                                    if let Ok(s) = read_todo(cfg) { todo = parse_table(&s, 1); todo_scroll = 0; force_redraw = true; }
                                    else { todo.clear(); todo_scroll = 0; force_redraw = true; }
                                } else { todo.clear(); todo_scroll = 0; force_redraw = true; }
                            }
                            KeyCode::Char('r') => { if let Ok(s) = read_todo(cfg) { todo = parse_table(&s, 1); todo_scroll = 0; } else { todo.clear(); todo_scroll = 0; } }
                            KeyCode::Char('k') => { if todo_scroll > 0 { todo_scroll -= 1; } }
                            KeyCode::Char('j') => { let h = 10; if todo_scroll + h < todo.len() { todo_scroll += 1; } }
                            _ => {}
                        },
                        AppState::CyberView => match key.code {
                            KeyCode::Char('q') => { state = AppState::MainMenu; cyber_scroll = 0; }
                            KeyCode::Char('e') => {
                                if open_in_neovim(&cfg.cyber_resource_file_path).is_ok() {
                                    if let Ok(s) = read_cyber(cfg) { cyber = parse_table(&s, 1); cyber_scroll = 0; force_redraw = true; }
                                    else { cyber.clear(); cyber_scroll = 0; force_redraw = true; }
                                } else { cyber.clear(); cyber_scroll = 0; force_redraw = true; }
                            }
                            KeyCode::Char('r') => { if let Ok(s) = read_cyber(cfg) { cyber = parse_table(&s, 1); cyber_scroll = 0; } else { cyber.clear(); cyber_scroll = 0; } }
                            KeyCode::Char('k') => { if cyber_scroll > 0 { cyber_scroll -= 1; } }
                            KeyCode::Char('j') => { let h = 10; if cyber_scroll + h < cyber.len() { cyber_scroll += 1; } }
                            _ => {}
                        },
                    }
                }
            }
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = load_config();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_app(&mut terminal, &cfg);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture).ok();
    result
}
