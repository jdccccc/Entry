use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use calamine::{open_workbook, DataType, Reader, Xlsx};
use crossterm::{
    cursor::MoveTo,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};

// ---------------- Configuration ----------------
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    todo_file_path: String,
    cyber_resource_file_path: String,
    bill_dir_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            todo_file_path: "md/TODO.md".into(),
            cyber_resource_file_path: "md/CyberResource.md".into(),
            bill_dir_path: "tmp".into(),
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
        if let Ok(text) = toml::to_string_pretty(&cfg) {
            let _ = fs::write(path, text);
        }
        cfg
    }
}

// ---------------- IO helpers ----------------
fn read_plain(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(fs::read_to_string(path)?)
}

fn read_todo(cfg: &Config) -> Result<String, Box<dyn std::error::Error>> {
    read_plain(&cfg.todo_file_path)
}

fn read_cyber(cfg: &Config) -> Result<String, Box<dyn std::error::Error>> {
    read_plain(&cfg.cyber_resource_file_path)
}

// ---------------- Parsing ----------------
fn parse_table(content: &str, min_cols: usize) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        if t.starts_with('|') && t.ends_with('|') {
            let cells: Vec<String> = t
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if cells.len() >= min_cols {
                out.push(cells);
            }
        }
    }
    out
}

type TableLoader = fn(&Config) -> Result<String, Box<dyn std::error::Error>>;
const SCROLL_WINDOW: usize = 10;

fn load_table(rows: &mut Vec<Vec<String>>, scroll: &mut usize, loader: TableLoader, cfg: &Config) {
    match loader(cfg) {
        Ok(s) => {
            *rows = parse_table(&s, 1);
        }
        Err(_) => rows.clear(),
    }
    *scroll = 0;
}

fn edit_table(
    rows: &mut Vec<Vec<String>>,
    scroll: &mut usize,
    loader: TableLoader,
    path: &str,
    cfg: &Config,
    force_redraw: &mut bool,
) {
    if open_in_neovim(path).is_ok() {
        load_table(rows, scroll, loader, cfg);
    } else {
        rows.clear();
        *scroll = 0;
    }
    *force_redraw = true;
}

fn scroll_up(scroll: &mut usize) {
    if *scroll > 0 {
        *scroll -= 1;
    }
}

fn scroll_down(scroll: &mut usize, len: usize) {
    if *scroll + SCROLL_WINDOW < len {
        *scroll += 1;
    }
}

// ---------------- Bill analysis ----------------
#[derive(Debug, Clone)]
struct BillEntry {
    partner: String,
    product: String,
    amount: f64,
}

#[derive(Debug, Clone)]
struct BillReport {
    nickname: String,
    expenses: Vec<BillEntry>,
    incomes: Vec<BillEntry>,
    total_expense: f64,
    total_income: f64,
    small_expense_total: f64,
}

impl BillReport {
    fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(&mut out, "# {} 微信账单分析\n", self.nickname);

        let _ = writeln!(&mut out, "## 支出");
        if self.expenses.is_empty() {
            out.push_str("无支出记录。\n\n");
        } else {
            out.push_str("| 交易对方 | 商品 | 金额(元) |\n|----------|------|----------|\n");
            for e in &self.expenses {
                let _ = writeln!(
                    &mut out,
                    "| {} | {} | {:.2} |",
                    e.partner, e.product, e.amount
                );
            }
            let _ = writeln!(&mut out, "\n- 总支出：{:.2} 元", self.total_expense);
            let _ = writeln!(
                &mut out,
                "- 小额支出(<100元)：{:.2} 元\n",
                self.small_expense_total
            );
        }

        let _ = writeln!(&mut out, "## 收入");
        if self.incomes.is_empty() {
            out.push_str("无收入记录。\n\n");
        } else {
            out.push_str("| 交易对方 | 商品 | 金额(元) |\n|----------|------|----------|\n");
            for e in &self.incomes {
                let _ = writeln!(
                    &mut out,
                    "| {} | {} | {:.2} |",
                    e.partner, e.product, e.amount
                );
            }
            let _ = writeln!(&mut out, "\n- 总收入：{:.2} 元\n", self.total_income);
        }

        out
    }
}

struct BillState {
    bill_dir: PathBuf,
    files: Vec<PathBuf>,
    processed: HashSet<PathBuf>,
    reports: Vec<BillReport>,
    last_export_dir: PathBuf,
}

impl BillState {
    fn new(cfg: &Config) -> Self {
        let dir = PathBuf::from(&cfg.bill_dir_path);
        Self {
            last_export_dir: dir.clone(),
            bill_dir: dir,
            files: Vec::new(),
            processed: HashSet::new(),
            reports: Vec::new(),
        }
    }

    fn ensure_dir(&self) -> io::Result<()> {
        if !self.bill_dir.exists() {
            fs::create_dir_all(&self.bill_dir)?;
        }
        Ok(())
    }

    fn refresh_files(&mut self) -> io::Result<()> {
        self.ensure_dir()?;
        let mut list: Vec<PathBuf> = fs::read_dir(&self.bill_dir)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("xlsx"))
                    .unwrap_or(false)
            })
            .collect();
        list.sort();
        self.files = list;
        Ok(())
    }

    fn pending_count(&self) -> usize {
        self.files
            .iter()
            .filter(|p| !self.processed.contains(*p))
            .count()
    }

    fn analyze_pending(&mut self) -> Result<usize, String> {
        self.reports.clear();
        let mut success = 0usize;
        let mut found = false;
        for path in &self.files {
            if self.processed.contains(path) {
                continue;
            }
            found = true;
            match analyze_bill_file(path) {
                Ok(report) => {
                    self.processed.insert(path.clone());
                    self.reports.push(report);
                    success += 1;
                }
                Err(e) => return Err(format!("分析失败：{}", e)),
            }
        }
        if !found {
            return Err("没有需要分析的账单".into());
        }
        Ok(success)
    }

    fn export_reports(&self, target_dir: &Path) -> io::Result<usize> {
        if self.reports.is_empty() {
            return Ok(0);
        }
        fs::create_dir_all(target_dir)?;
        for report in &self.reports {
            let file_name = format!("{}.md", sanitize_filename(&report.nickname));
            let path = target_dir.join(file_name);
            fs::write(path, report.to_markdown())?;
        }
        Ok(self.reports.len())
    }

    fn net_summary(&self) -> Option<String> {
        if self.reports.is_empty() {
            return None;
        }
        let total_income: f64 = self.reports.iter().map(|r| r.total_income).sum();
        let total_expense: f64 = self.reports.iter().map(|r| r.total_expense).sum();
        let net = total_income - total_expense;
        let label = if net >= 0.0 { "净收入" } else { "净支出" };
        let value = net.abs();
        Some(format!(
            "{}：{:.2} 元 (收入 {:.2} - 支出 {:.2})",
            label, value, total_income, total_expense
        ))
    }
}

fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() {
            out.push('_');
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "bill".into()
    } else {
        out
    }
}

fn cell_to_string(cell: &DataType) -> String {
    match cell {
        DataType::String(s) => s.clone(),
        DataType::Float(f) => format!("{:.2}", f),
        DataType::Int(i) => i.to_string(),
        DataType::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn parse_amount(raw: &str) -> Option<f64> {
    let mut buf = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' {
            buf.push(ch);
        }
    }
    if buf.is_empty() {
        None
    } else {
        buf.parse().ok()
    }
}

fn analyze_bill_file(path: &Path) -> Result<BillReport, String> {
    let mut workbook: Xlsx<_> =
        open_workbook(path).map_err(|e| format!("无法打开{}: {}", path.display(), e))?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| "账单缺少工作表".to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))?;

    let nickname_raw = range
        .get((1, 0))
        .map(cell_to_string)
        .ok_or_else(|| "无法读取昵称".to_string())?;
    let nickname = nickname_raw
        .split(['[', ']'])
        .nth(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| nickname_raw.clone());

    let header_idx = range
        .rows()
        .enumerate()
        .find(|(_, row)| row.get(0).map(cell_to_string).unwrap_or_default().trim() == "交易时间")
        .map(|(idx, _)| idx)
        .ok_or_else(|| "未找到账单列表".to_string())?;

    let mut expenses = Vec::new();
    let mut incomes = Vec::new();

    for row in range.rows().skip(header_idx + 1) {
        if row.iter().all(|c| matches!(c, DataType::Empty)) {
            continue;
        }
        let category = row.get(4).map(cell_to_string).unwrap_or_default();
        let partner = row.get(2).map(cell_to_string).unwrap_or_default();
        let product = row.get(3).map(cell_to_string).unwrap_or_default();
        let amount_str = row.get(5).map(cell_to_string).unwrap_or_default();
        if category.is_empty() || amount_str.is_empty() {
            continue;
        }
        let amount = match parse_amount(&amount_str) {
            Some(v) => v,
            None => continue,
        };
        let entry = BillEntry {
            partner,
            product,
            amount,
        };
        if category.contains('支') {
            expenses.push(entry);
        } else if category.contains('收') {
            incomes.push(entry);
        }
    }

    expenses.sort_by(|a, b| {
        b.amount
            .partial_cmp(&a.amount)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    incomes.sort_by(|a, b| {
        b.amount
            .partial_cmp(&a.amount)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total_expense: f64 = expenses.iter().map(|e| e.amount).sum();
    let small_expense_total: f64 = expenses
        .iter()
        .filter(|e| e.amount < 100.0)
        .map(|e| e.amount)
        .sum();
    let total_income: f64 = incomes.iter().map(|e| e.amount).sum();

    Ok(BillReport {
        nickname,
        expenses,
        incomes,
        total_expense,
        total_income,
        small_expense_total,
    })
}

fn prompt_export_directory(default: &Path) -> io::Result<PathBuf> {
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    disable_raw_mode()?;

    let result = (|| {
        println!("请输入导出目录 (回车使用默认: {}):", default.display());
        print!("> ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        let mut target = if trimmed.is_empty() {
            default.to_path_buf()
        } else {
            PathBuf::from(trimmed)
        };
        if target.is_relative() {
            target = std::env::current_dir()?.join(target);
        }
        fs::create_dir_all(&target)?;
        Ok(target)
    })();

    enable_raw_mode().ok();
    let _ = execute!(
        std::io::stdout(),
        Clear(ClearType::All),
        Clear(ClearType::Purge),
        MoveTo(1, 1)
    );
    let _ = execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture);
    std::io::stdout().flush().ok();

    result
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
        let msg = Paragraph::new("未找到")
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(msg, area);
        return;
    }

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        let block = Block::default().borders(Borders::ALL).title(title);
        let msg = Paragraph::new("未找到")
            .alignment(Alignment::Center)
            .block(block);
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
    for w in &mut max_w {
        if *w < 8 {
            *w = 8;
        }
        if *w > 50 {
            *w = 50;
        }
    }
    let cons: Vec<Constraint> = max_w
        .iter()
        .map(|&w| Constraint::Length(w as u16))
        .collect();

    let h = area.height.saturating_sub(2) as usize; // roughly visible rows inside border
    let start = scroll.min(rows.len());
    let mut end = start.saturating_add(h);
    if end > rows.len() {
        end = rows.len();
    }
    let vis = &rows[start..end];

    let table_rows: Vec<Row> = vis
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut cells: Vec<Cell> = Vec::with_capacity(cols);
            for c in 0..cols {
                cells.push(Cell::from(row.get(c).map(|s| s.as_str()).unwrap_or("")));
            }
            let style = if start + i == 0 {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(cells).style(style)
        })
        .collect();

    let table =
        Table::new(table_rows, cons).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(table, area);
}

fn render_table_page(
    f: &mut Frame,
    size: Rect,
    header_text: &str,
    block_title: &str,
    table_title: &str,
    rows: &[Vec<String>],
    scroll: usize,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(size);

    let header = Paragraph::new(header_text)
        .alignment(Alignment::Center)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title(block_title));
    f.render_widget(header, chunks[0]);

    render_table_generic(f, chunks[1], rows, scroll, table_title);

    let help = Paragraph::new("jk -- move | q -- back | e -- edit | r -- refresh")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[2]);
}

fn render_bill_view(f: &mut Frame, size: Rect, bill_state: &BillState, last_msg: Option<&str>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(size);

    let header = Paragraph::new("账单分析")
        .alignment(Alignment::Center)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL).title("Bill"));
    f.render_widget(header, chunks[0]);

    let net_line = bill_state
        .net_summary()
        .unwrap_or_else(|| "暂无净收入/净支出".into());
    let info = format!(
        "账单目录: {dir}\n待分析账单: {pending}\n可导出报表: {reports}\n{net}",
        dir = bill_state.bill_dir.display(),
        pending = bill_state.pending_count(),
        reports = bill_state.reports.len(),
        net = net_line,
    );
    let info_block = Paragraph::new(info)
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::ALL).title("状态"));
    f.render_widget(info_block, chunks[1]);

    let mut help_lines = vec![String::from(
        "a -- 分析 | o -- 导出 | r -- 刷新 | q -- 返回",
    )];
    if let Some(msg) = last_msg {
        help_lines.push(msg.to_string());
    }
    let help = Paragraph::new(help_lines.join("\n"))
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::ALL).title("操作"));
    f.render_widget(help, chunks[2]);
}

// ---------------- External editor ----------------
fn open_in_neovim(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    disable_raw_mode()?;
    let status = Command::new("nvim").arg(path).status()?;
    execute!(
        std::io::stdout(),
        Clear(ClearType::All),
        Clear(ClearType::Purge),
        MoveTo(1, 1)
    )?;
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    std::io::stdout().flush()?;
    if !status.success() {
        return Err("Failed to open neovim".into());
    }
    Ok(())
}

// ---------------- UI & State ----------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppState {
    MainMenu,
    TodoView,
    CyberView,
    BillView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItem {
    Todo,
    Bill,
    Cyber,
}
impl MenuItem {
    fn all() -> [MenuItem; 3] {
        [MenuItem::Todo, MenuItem::Bill, MenuItem::Cyber]
    }
    fn title(&self) -> &'static str {
        match self {
            MenuItem::Todo => "TODO",
            MenuItem::Bill => "BILL",
            MenuItem::Cyber => "CYBER RESOURCE",
        }
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    cfg: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut selected = 0usize;
    let items = MenuItem::all();
    let mut last_msg: Option<String> = None;
    let mut list_state = ListState::default();
    list_state.select(Some(0));
    let mut state = AppState::MainMenu;
    let mut force_redraw = false;

    let mut todo: Vec<Vec<String>> = Vec::new();
    let mut todo_scroll = 0usize;

    let mut cyber: Vec<Vec<String>> = Vec::new();
    let mut cyber_scroll = 0usize;

    let mut bill_state = BillState::new(cfg);

    loop {
        list_state.select(Some(selected));
        if force_redraw {
            terminal.clear()?;
            force_redraw = false;
        }

        terminal.draw(|f| {
            let size = f.size();
            match state {
                AppState::MainMenu => {
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),
                            Constraint::Min(5),
                            Constraint::Length(3),
                        ])
                        .split(size);

                    let header = Paragraph::new(Line::from(vec![
                        Span::styled(
                            "Jeek!",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("\t Exist before meaning, feel yourself, embrace imperfection."),
                    ]))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("Hello"));
                    f.render_widget(header, chunks[0]);

                    let list_items: Vec<ListItem> = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| {
                            let style = if i == selected {
                                Style::default()
                                    .bg(Color::Blue)
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            };
                            ListItem::new(Line::from(Span::styled(
                                format!(" {} : {} ", i, item.title()),
                                style,
                            )))
                        })
                        .collect();
                    let list = List::new(list_items)
                        .block(Block::default().borders(Borders::ALL).title("Menu"))
                        .highlight_style(
                            Style::default()
                                .bg(Color::Blue)
                                .add_modifier(Modifier::BOLD),
                        )
                        .highlight_symbol("→ ");
                    f.render_stateful_widget(list, chunks[1], &mut list_state);

                    let help = match &last_msg {
                        Some(m) => m.as_str(),
                        None => "\tjk -- move, Enter -- select, q -- exit",
                    };
                    let footer = Paragraph::new(help)
                        .alignment(Alignment::Left)
                        .block(Block::default().borders(Borders::ALL).title("Message"));
                    f.render_widget(footer, chunks[2]);
                }
                AppState::TodoView => {
                    render_table_page(f, size, "TODO List", "TODO", "Tasks", &todo, todo_scroll);
                }
                AppState::CyberView => {
                    render_table_page(
                        f,
                        size,
                        "Cyber Resource List",
                        "Cyber Resource",
                        "Resources",
                        &cyber,
                        cyber_scroll,
                    );
                }
                AppState::BillView => {
                    render_bill_view(f, size, &bill_state, last_msg.as_deref());
                }
            }
        })?;

        if event::poll(std::time::Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match state {
                        AppState::MainMenu => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Up | KeyCode::Char('k') => {
                                selected = if selected == 0 {
                                    items.len() - 1
                                } else {
                                    selected - 1
                                };
                                last_msg = None;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                selected = (selected + 1) % items.len();
                                last_msg = None;
                            }
                            KeyCode::Enter => match items[selected] {
                                MenuItem::Todo => {
                                    load_table(&mut todo, &mut todo_scroll, read_todo, cfg);
                                    state = AppState::TodoView;
                                    last_msg = None;
                                }
                                MenuItem::Cyber => {
                                    load_table(&mut cyber, &mut cyber_scroll, read_cyber, cfg);
                                    state = AppState::CyberView;
                                    last_msg = None;
                                }
                                MenuItem::Bill => match bill_state.refresh_files() {
                                    Ok(_) => {
                                        state = AppState::BillView;
                                        force_redraw = true;
                                        last_msg = None;
                                    }
                                    Err(e) => last_msg = Some(format!("读取账单失败: {}", e)),
                                },
                            },
                            _ => {}
                        },
                        AppState::TodoView => match key.code {
                            KeyCode::Char('q') => {
                                state = AppState::MainMenu;
                                todo_scroll = 0;
                            }
                            KeyCode::Char('e') => {
                                edit_table(
                                    &mut todo,
                                    &mut todo_scroll,
                                    read_todo,
                                    &cfg.todo_file_path,
                                    cfg,
                                    &mut force_redraw,
                                );
                            }
                            KeyCode::Char('r') => {
                                load_table(&mut todo, &mut todo_scroll, read_todo, cfg);
                            }
                            KeyCode::Char('k') => {
                                scroll_up(&mut todo_scroll);
                            }
                            KeyCode::Char('j') => {
                                scroll_down(&mut todo_scroll, todo.len());
                            }
                            _ => {}
                        },
                        AppState::CyberView => match key.code {
                            KeyCode::Char('q') => {
                                state = AppState::MainMenu;
                                cyber_scroll = 0;
                            }
                            KeyCode::Char('e') => {
                                edit_table(
                                    &mut cyber,
                                    &mut cyber_scroll,
                                    read_cyber,
                                    &cfg.cyber_resource_file_path,
                                    cfg,
                                    &mut force_redraw,
                                );
                            }
                            KeyCode::Char('r') => {
                                load_table(&mut cyber, &mut cyber_scroll, read_cyber, cfg);
                            }
                            KeyCode::Char('k') => {
                                scroll_up(&mut cyber_scroll);
                            }
                            KeyCode::Char('j') => {
                                scroll_down(&mut cyber_scroll, cyber.len());
                            }
                            _ => {}
                        },
                        AppState::BillView => match key.code {
                            KeyCode::Char('q') => {
                                state = AppState::MainMenu;
                                force_redraw = true;
                            }
                            KeyCode::Char('r') => {
                                match bill_state.refresh_files() {
                                    Ok(_) => {
                                        last_msg = Some(format!(
                                            "待分析账单：{}",
                                            bill_state.pending_count()
                                        ));
                                    }
                                    Err(e) => {
                                        last_msg = Some(format!("刷新失败: {}", e));
                                    }
                                }
                                force_redraw = true;
                            }
                            KeyCode::Char('a') => {
                                match bill_state.analyze_pending() {
                                    Ok(n) => {
                                        let net = bill_state
                                            .net_summary()
                                            .unwrap_or_else(|| "暂无统计数据".into());
                                        last_msg = Some(format!("完成 {} 份账单分析 | {}", n, net));
                                    }
                                    Err(e) => last_msg = Some(e),
                                }
                                force_redraw = true;
                            }
                            KeyCode::Char('o') => {
                                if bill_state.reports.is_empty() {
                                    last_msg = Some("请先按a完成分析".into());
                                } else {
                                    match prompt_export_directory(&bill_state.last_export_dir) {
                                        Ok(dir) => match bill_state.export_reports(&dir) {
                                            Ok(count) => {
                                                last_msg = Some(format!(
                                                    "已导出 {} 份报表至 {}",
                                                    count,
                                                    dir.display()
                                                ));
                                                bill_state.last_export_dir = dir;
                                            }
                                            Err(e) => last_msg = Some(format!("导出失败: {}", e)),
                                        },
                                        Err(e) => {
                                            last_msg = Some(format!("输入导出路径失败: {}", e))
                                        }
                                    }
                                }
                                force_redraw = true;
                            }
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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    result
}
