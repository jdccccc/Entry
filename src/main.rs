use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
use csv::ReaderBuilder;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, Wrap},
    Frame, Terminal,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

// ---------------- Configuration ----------------
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    todo_file_path: String,
    cyber_resource_file_path: String,
    bill_dir_path: String,
    #[serde(default)]
    weather_api_key: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            todo_file_path: "md/TODO.md".into(),
            cyber_resource_file_path: "md/CyberResource.md".into(),
            bill_dir_path: "tmp".into(),
            weather_api_key: String::new(),
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

// ---------------- Weather ----------------
#[derive(Clone, Copy)]
struct WeatherLocation {
    query: &'static str,
    label: &'static str,
}

const WEATHER_LOCATIONS: [WeatherLocation; 2] = [
    WeatherLocation {
        query: "beijing",
        label: "北京",
    },
    WeatherLocation {
        query: "shijiazhuang",
        label: "石家庄",
    },
];

const WEATHER_ENDPOINT: &str = "http://api.weatherapi.com/v1/forecast.json";

#[derive(Debug, Clone)]
enum WeatherCard {
    Success {
        name: String,
        condition: String,
        temperature: String,
    },
    Error {
        label: String,
        message: String,
    },
}

#[derive(Debug, Deserialize)]
struct WeatherApiResponse {
    location: WeatherApiLocation,
    current: WeatherApiCurrent,
    forecast: WeatherApiForecast,
}

#[derive(Debug, Deserialize)]
struct WeatherApiLocation {
    name: String,
}

#[derive(Debug, Deserialize)]
struct WeatherApiCurrent {
    condition: WeatherApiCondition,
    temp_c: f64,
}

#[derive(Debug, Deserialize)]
struct WeatherApiForecast {
    forecastday: Vec<WeatherApiForecastDay>,
}

#[derive(Debug, Deserialize)]
struct WeatherApiForecastDay {
    day: WeatherApiDay,
}

#[derive(Debug, Deserialize)]
struct WeatherApiDay {
    maxtemp_c: f64,
    mintemp_c: f64,
}

#[derive(Debug, Deserialize)]
struct WeatherApiCondition {
    text: String,
}

fn fetch_weather_board(cfg: Config, sender: mpsc::Sender<Vec<WeatherCard>>) {
    thread::spawn(move || {
        let key = cfg.weather_api_key.trim();
        if key.is_empty() {
            let cards = WEATHER_LOCATIONS
                .iter()
                .map(|loc| WeatherCard::Error {
                    label: loc.label.to_string(),
                    message: "请在config.toml中配置weather_api_key".to_string(),
                })
                .collect();
            
            let _ = sender.send(cards);
            return;
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| Client::new());

        // 使用并行请求提高性能
        let client = Arc::new(client);
        let key = Arc::new(key.to_string());
        
        let cards: Vec<WeatherCard> = WEATHER_LOCATIONS
            .iter()
            .map(|loc| {
                let client = Arc::clone(&client);
                let key = Arc::clone(&key);
                let loc = loc.clone();
                
                std::thread::spawn(move || {
                    fetch_city_weather(&client, &key, &loc)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        let _ = sender.send(cards);
    });
}

fn fetch_city_weather(client: &Client, api_key: &str, loc: &WeatherLocation) -> WeatherCard {
    let response = match client
        .get(WEATHER_ENDPOINT)
        .query(&[
            ("key", api_key),
            ("q", loc.query),
            ("lang", "zh"),
            ("aqi", "no"),
            ("days", "1"),
        ])
        .send()
    {
        Ok(resp) => resp,
        Err(_) => {
            return WeatherCard::Error {
                label: loc.label.to_string(),
                message: "网络请求失败".to_string(),
            };
        }
    };

    if !response.status().is_success() {
        let error_msg = match response.status().as_u16() {
            401 => "API密钥无效或已过期".to_string(),
            403 => "API访问被拒绝，请检查密钥权限".to_string(),
            400 => "请求参数错误".to_string(),
            404 => "城市未找到".to_string(),
            429 => "API请求频率超限".to_string(),
            500..=599 => "天气服务器内部错误".to_string(),
            _ => format!("天气API错误 (HTTP {})", response.status()),
        };
        
        return WeatherCard::Error {
            label: loc.label.to_string(),
            message: error_msg,
        };
    }

    let bytes = match response.bytes() {
        Ok(bytes) => bytes,
        Err(_) => {
            return WeatherCard::Error {
                label: loc.label.to_string(),
                message: "读取响应失败".to_string(),
            };
        }
    };

    match serde_json::from_slice::<WeatherApiResponse>(&bytes) {
        Ok(data) => {
            // 获取当天的预报数据
            let forecast_day = data.forecast.forecastday.first();
            
            match forecast_day {
                Some(day) => {
                    WeatherCard::Success {
                        name: data.location.name,
                        condition: data.current.condition.text,
                        temperature: format!("{:.1}C~{:.1}C", day.day.mintemp_c, day.day.maxtemp_c),
                    }
                }
                None => {
                    WeatherCard::Error {
                        label: loc.label.to_string(),
                        message: "无法获取预报数据".to_string(),
                    }
                }
            }
        }
        Err(_) => WeatherCard::Error {
            label: loc.label.to_string(),
            message: "解析天气数据失败".to_string(),
        },
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

#[derive(Debug, Clone, Default)]
struct BillAggregate {
    incomes: Vec<BillEntry>,
    expenses: Vec<BillEntry>,
}

impl BillAggregate {
    fn from_entries(expenses: Vec<BillEntry>, incomes: Vec<BillEntry>) -> Self {
        Self { incomes, expenses }
    }

    fn extend(&mut self, mut other: BillAggregate) {
        self.incomes.append(&mut other.incomes);
        self.expenses.append(&mut other.expenses);
    }

    fn is_empty(&self) -> bool {
        self.incomes.is_empty() && self.expenses.is_empty()
    }

    fn total_income(&self) -> f64 {
        self.incomes.iter().map(|e| e.amount).sum()
    }

    fn total_expense(&self) -> f64 {
        self.expenses.iter().map(|e| e.amount).sum()
    }

    fn net(&self) -> f64 {
        self.total_income() - self.total_expense()
    }

    fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(&mut out, "# 账单分析\n");
        
        let _ = writeln!(out, "## 支出");
        if self.expenses.is_empty() {
            out.push_str("无支出记录。\n");
        } else {
            out.push_str("| 交易对方 | 商品 | 金额(元) |\n|----------|------|----------|\n");
            for e in &self.expenses {
                let _ = writeln!(out, "| {} | {} | {:.2} |", e.partner, e.product, e.amount);
            }
            let _ = writeln!(out, "\n总支出：{:.2} 元\n", self.total_expense());
        }
        
        let _ = writeln!(out, "## 收入");
        if self.incomes.is_empty() {
            out.push_str("无收入记录。\n");
        } else {
            out.push_str("| 交易对方 | 商品 | 金额(元) |\n|----------|------|----------|\n");
            for e in &self.incomes {
                let _ = writeln!(out, "| {} | {} | {:.2} |", e.partner, e.product, e.amount);
            }
            let _ = writeln!(out, "\n总收入：{:.2} 元\n", self.total_income());
        }
        
        let net = self.net();
        let label = if net >= 0.0 { "净收入" } else { "净支出" };
        let _ = writeln!(out, "{}：{:.2} 元", label, net.abs());
        
        out
    }
}

struct BillState {
    bill_dir: PathBuf,
    files: Vec<PathBuf>,
    processed: HashSet<PathBuf>,
    aggregate: BillAggregate,
}

impl BillState {
    fn new(cfg: &Config) -> Self {
        let dir = PathBuf::from(&cfg.bill_dir_path);
        Self {
            bill_dir: dir,
            files: Vec::new(),
            processed: HashSet::new(),
            aggregate: BillAggregate::default(),
        }
    }

    fn refresh_files(&mut self) -> io::Result<()> {
        if !self.bill_dir.exists() {
            fs::create_dir_all(&self.bill_dir)?;
        }
        
        let mut list: Vec<PathBuf> = fs::read_dir(&self.bill_dir)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| {
                        let ext_lc = ext.to_ascii_lowercase();
                        ext_lc == "xlsx" || ext_lc == "csv"
                    })
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
        let mut success = 0usize;
        for path in &self.files {
            if self.processed.contains(path) {
                continue;
            }
            match analyze_bill_file(path) {
                Ok(report) => {
                    self.processed.insert(path.clone());
                    self.aggregate.extend(report);
                    success += 1;
                }
                Err(_) => return Err("分析失败".into()),
            }
        }
        if success == 0 {
            return Err("没有需要分析的账单".into());
        }
        Ok(success)
    }

    fn export_reports(&self, target_dir: &Path) -> io::Result<usize> {
        if self.aggregate.is_empty() {
            return Ok(0);
        }
        fs::create_dir_all(target_dir)?;
        let path = target_dir.join("bill_summary.md");
        fs::write(path, self.aggregate.to_markdown())?;
        Ok(1)
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

fn analyze_bill_file(path: &Path) -> Result<BillAggregate, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| "无法识别文件类型".to_string())?;
    
    match ext.as_str() {
        "xlsx" => parse_wechat_bill(path),
        "csv" => parse_alipay_bill(path),
        _ => Err("不支持的账单格式".to_string()),
    }
}

fn parse_wechat_bill(path: &Path) -> Result<BillAggregate, String> {
    let mut workbook: Xlsx<_> = open_workbook(path).map_err(|_| "无法打开文件".to_string())?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| "账单缺少工作表".to_string())
        .and_then(|r| r.map_err(|_| "读取工作表失败".to_string()))?;

    let header_idx = range
        .rows()
        .enumerate()
        .find(|(_, row)| row.first().map(cell_to_string).unwrap_or_default().trim() == "交易时间")
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

    Ok(BillAggregate::from_entries(expenses, incomes))
}

fn parse_alipay_bill(path: &Path) -> Result<BillAggregate, String> {
    let mut content = fs::read_to_string(path).map_err(|_| "无法读取文件".to_string())?;
    if let Some(stripped) = content.strip_prefix('\u{feff}') {
        content = stripped.to_string();
    }
    
    let start = content
        .find("交易时间,")
        .ok_or_else(|| "未找到账单数据表头".to_string())?;
    let data = &content[start..];
    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .from_reader(data.as_bytes());

    let mut expenses = Vec::new();
    let mut incomes = Vec::new();

    for result in reader.records() {
        let record = result.map_err(|_| "解析CSV失败".to_string())?;
        if record.len() < 7 {
            continue;
        }
        
        let flow = record.get(5).unwrap_or("").trim();
        let amount_text = record.get(6).unwrap_or("").trim();
        
        if flow.is_empty() || amount_text.is_empty() {
            continue;
        }
        
        let amount = match parse_amount(amount_text) {
            Some(v) => v,
            None => continue,
        };
        
        let entry = BillEntry {
            partner: record.get(2).unwrap_or("").trim().to_string(),
            product: record.get(4).unwrap_or("").trim().to_string(),
            amount,
        };

        if flow.contains('支') {
            expenses.push(entry);
        } else if flow.contains('收') {
            incomes.push(entry);
        }
    }

    Ok(BillAggregate::from_entries(expenses, incomes))
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

    let mut max_w = vec![8usize; cols]; // 默认最小宽度
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < cols {
                let cell_str = cell.as_str();
                max_w[i] = max_w[i].max(cell_str.chars().count());
            }
        }
    }
    for w in &mut max_w {
        *w = (*w).clamp(8, 30); // 减小最大宽度限制
    }
    let cons: Vec<Constraint> = max_w
        .iter()
        .map(|&w| Constraint::Length(w as u16))
        .collect();

    let h = area.height.saturating_sub(2) as usize;
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

    let help = Paragraph::new("\tjk -- move | q -- back | e -- edit | r -- refresh")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title("Help"));
    f.render_widget(help, chunks[2]);
}

fn render_weather_panel(f: &mut Frame, area: Rect, cards: &[WeatherCard]) {
    let block = Block::default().borders(Borders::ALL).title("Weather");
    if cards.is_empty() {
        let placeholder = Paragraph::new("暂无天气数据")
            .alignment(Alignment::Center)
            .block(block);
        f.render_widget(placeholder, area);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (idx, card) in cards.iter().enumerate() {
        match card {
            WeatherCard::Success {
                name,
                condition,
                temperature,
            } => {
                lines.push(Line::from(vec![
                    Span::styled(
                        name.as_str(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::raw(format!("{}  {}", condition, temperature)),
                ]));
            }
            WeatherCard::Error { label, message } => {
                lines.push(Line::from(vec![
                    Span::styled(
                        label.as_str(),
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        message.as_str(),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
        }
        if idx + 1 < cards.len() {
            lines.push(Line::default());
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
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

    if bill_state.files.is_empty() {
        let info_block = Paragraph::new("暂无账单")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title("状态"));
        f.render_widget(info_block, chunks[1]);
    } else {
        let mut info_lines = vec![
            format!("账单目录: {}", bill_state.bill_dir.display()),
            format!("待分析账单: {}", bill_state.pending_count()),
            format!("已分析账单: {}", bill_state.processed.len()),
        ];
        
        if !bill_state.aggregate.is_empty() {
            let net = bill_state.aggregate.net();
            let label = if net >= 0.0 { "净收入" } else { "净支出" };
            info_lines.push(format!("{}：{:.2} 元", label, net.abs()));
        }
        
        let info_block = Paragraph::new(info_lines.join("\n"))
            .alignment(Alignment::Left)
            .block(Block::default().borders(Borders::ALL).title("状态"));
        f.render_widget(info_block, chunks[1]);
    }

    let help_text = if let Some(msg) = last_msg {
        format!("a -- 分析 | o -- 导出 | r -- 刷新 | q -- 返回\n{}", msg)
    } else {
        "a -- 分析 | o -- 导出 | r -- 刷新 | q -- 返回".to_string()
    };
    
    let help = Paragraph::new(help_text)
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
    let mut weather_cards = Vec::new(); // 初始化为空，按w再加载
    
    // 创建通道用于接收天气数据
    let (weather_tx, weather_rx) = mpsc::channel::<Vec<WeatherCard>>();
    let weather_rx = Arc::new(Mutex::new(weather_rx));
    
    // 检查天气API密钥状态
    if cfg.weather_api_key.trim().is_empty() {
        last_msg = Some("警告: 未配置天气API密钥，请在config.toml中设置weather_api_key".to_string());
    }

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

                    let body = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
                        .split(chunks[1]);
                    f.render_stateful_widget(list, body[0], &mut list_state);
                    render_weather_panel(f, body[1], &weather_cards);

                    let help = match &last_msg {
                        Some(m) => m.as_str(),
                        None => "jk -- move, Enter -- select, w -- load weather, q -- exit",
                    };
                    let footer = Paragraph::new(help)
                        .alignment(Alignment::Left)
                        .block(Block::default().borders(Borders::ALL).title("Help"));
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

        // 检查是否有新的天气数据
        if let Ok(rx) = weather_rx.try_lock() {
            if let Ok(cards) = rx.try_recv() {
                weather_cards = cards;
                force_redraw = true;
            }
        }

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
                                MenuItem::Bill => {
                                    let _ = bill_state.refresh_files();
                                    state = AppState::BillView;
                                    force_redraw = true;
                                    last_msg = None;
                                }
                            },
                            KeyCode::Char('w') => {
                                // 启动后台线程加载天气数据
                                fetch_weather_board(cfg.clone(), weather_tx.clone());
                            }
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
                                        let net = if bill_state.aggregate.is_empty() {
                                            "暂无统计数据".to_string()
                                        } else {
                                            let net_val = bill_state.aggregate.net();
                                            let label = if net_val >= 0.0 { "净收入" } else { "净支出" };
                                            format!("{}：{:.2} 元", label, net_val.abs())
                                        };
                                        last_msg = Some(format!("完成 {} 份账单分析 | {}", n, net));
                                    }
                                    Err(e) => last_msg = Some(e),
                                }
                                force_redraw = true;
                            }
                            KeyCode::Char('o') => {
                                if bill_state.aggregate.is_empty() {
                                    last_msg = Some("请先按a完成分析".into());
                                } else {
                                    let default_dir = &bill_state.bill_dir;
                                    match prompt_export_directory(default_dir) {
                                        Ok(dir) => match bill_state.export_reports(&dir) {
                                            Ok(count) => {
                                                last_msg = Some(format!(
                                                    "已导出 {} 份报表至 {}",
                                                    count,
                                                    dir.display()
                                                ));
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