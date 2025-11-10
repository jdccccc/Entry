// 引入标准库的输入/输出模块，用于获取标准输出句柄等
use std::io::{self, Write};
use std::fs;
use std::process::Command;
use std::path::Path;

// 引入 crossterm（跨平台终端控制库）中的若干模块
use crossterm::{
    // 事件处理：键盘、鼠标以及事件枚举
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    // 执行一系列终端命令的宏/函数
    execute,
    // 终端模式相关：启用/禁用原始模式、进入/离开备用屏幕、清屏、光标移动
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, Clear, ClearType},
    // 光标控制
    cursor::MoveTo,
};

// 引入 ratatui（TUI 框架）中的组件
use ratatui::{
    // 终端后端适配器：将 ratatui 绑定到 crossterm 后端
    backend::CrosstermBackend,
    // 布局系统：对齐、约束、方向、布局
    layout::{Alignment, Constraint, Direction, Layout},
    // 样式系统：颜色、修饰（加粗等）、样式组合
    style::{Color, Modifier, Style},
    // 文本组件：行、片段
    text::{Line, Span},
    // 常用小部件：边框块、列表、段落等
    widgets::{Block, Borders, List, ListItem, Paragraph, ListState, Table, Row, Cell},
    // 终端抽象
    Terminal,
};

// 引入序列化相关库
use serde::{Deserialize, Serialize};

// 配置结构体
#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    todo_file_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            todo_file_path: "md/TODO.md".to_string(),
        }
    }
}

// 加载配置文件
fn load_config() -> Config {
    let config_path = "config.toml";
    
    if Path::new(config_path).exists() {
        match fs::read_to_string(config_path) {
            Ok(content) => {
                match toml::from_str(&content) {
                    Ok(config) => config,
                    Err(_) => {
                        eprintln!("Warning: Invalid config file format, using default config");
                        Config::default()
                    }
                }
            }
            Err(_) => {
                eprintln!("Warning: Cannot read config file, using default config");
                Config::default()
            }
        }
    } else {
        // 创建默认配置文件
        let default_config = Config::default();
        if let Ok(content) = toml::to_string_pretty(&default_config) {
            let _ = fs::write(config_path, content);
        }
        default_config
    }
}

// 应用状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppState {
    MainMenu,
    TodoView,
}

// 派生（自动生成）调试、复制语义、相等比较等常用特性
// 它会为 MenuItem 自动实现一组 trait（接口/能力），让这个类型更好用：
// Debug：允许用 {:?} 打印调试信息。
// Clone：提供 .clone() 复制能力。
// Copy：按位复制（赋值/传参不"移动"，而是复制一份）。只有当所有字段都 Copy 时才允许；纯单元枚举天然 Copy。
// PartialEq / Eq：支持 == / !=。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 定义一个菜单项的枚举：包含 Todo 和 Bill 两个分支
enum MenuItem {
    Todo,
    Bill,
    CyberResource,
}

// impl <Type> { ... } 给某个类型实现方法/关联函数/trait 实现等
// 为枚举实现一些辅助方法
impl MenuItem {
    // 没有 self 参数的函数，定义在 impl 块里，就是关联函数。关联函数类似其他语言的静态方法。
    // 返回所有菜单项的固定数组（长度为 2）
    fn all() -> [MenuItem; 3] { [MenuItem::Todo, MenuItem::Bill, MenuItem::CyberResource] }

    // 带有 &self 参数，因此这是方法，可用方法调用语法糖：item.title()（等价 MenuItem::title(&item)）。
    // 返回类型：&'static str
    // &str 是字符串切片（只读视图，不拥有数据）
    // 'static 生命周期表示lived and整个程序一样久。字符串字面量（如 "TODO"）就是 &'static str，因为它们被编译进只读数据段，程序运行期间一直存在。
    // match self { ... } 模式匹配：
    // 必须穷尽所有变体，否则编译不过。
    // 每个分支返回同一类型（这里都是 &'static str），最后整体返回该类型。
    // 返回菜单项的标题文本，&'static str 表示静态生命周期的字符串字面量。
    // str类型是动态大小的，编译器无法在编译阶段确定如何进行str的copy，所以str既没有copy也没有clone，往往通过&str来声明，*来解引用。
    fn title(&self) -> &'static str {
        match self {
            MenuItem::Todo => "TODO",
            MenuItem::Bill => "BILL",
            MenuItem::CyberResource => "CYBER RESOURCE",
        }
    }
}

// 读取TODO文件内容，如果文件不存在则创建默认内容
fn read_todo_file(config: &Config) -> Result<String, Box<dyn std::error::Error>> {
    let todo_path = &config.todo_file_path;
    
    // 获取文件目录
    if let Some(parent) = Path::new(todo_path).parent() {
        // 如果目录不存在，创建它
        if !fs::metadata(parent).is_ok() {
            fs::create_dir_all(parent)?;
        }
    }
    
    // 如果TODO.md文件不存在，创建默认内容
    if !fs::metadata(todo_path).is_ok() {
        // r - 表示这是一个原始字符串（raw string）
        let default_content = r"# TODO List

| 任务 | 状态 | 优先级 |
|------|------|--------|
| 学习 | 进行中 | 高 |
| 完成项目 | 未开始 | 中 |
| 整理文档 | 未开始 | 低 |
";
        fs::write(todo_path, default_content)?;
    }
    
    Ok(fs::read_to_string(todo_path)?)
}

// 解析TODO表格数据
fn parse_todo_table(content: &str) -> Vec<Vec<String>> {
    // 创建一个可变的二维向量来存储表格数据，外层向量表示行，内层向量表示列
    let mut table_data = Vec::new();
    // 将输入内容按行分割，收集到一个向量中，类型为 Vec<&str>
    let lines: Vec<&str> = content.lines().collect();
    
    // 遍历每一行内容
    for line in lines {
        // 去除行首尾的空白字符（空格、制表符等）
        let trimmed = line.trim();
        // 检查是否为有效的表格行：必须以 '|' 开始和结束（Markdown 表格格式）
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            // 解析表格行：按 '|' 分割并处理每个单元格
            let cells: Vec<String> = trimmed
                .split('|')                    // 按 '|' 字符分割字符串
                .map(|s| s.trim().to_string()) // 对每个分割部分：去除空白并转为 String
                .filter(|s| !s.is_empty())     // 过滤掉空字符串（分割产生的边界空元素）
                .collect();                    // 收集结果到 Vec<String>
            
            // 确保至少有3列（任务、状态、优先级）
            if cells.len() >= 3 {
                // 将解析出的单元格数据添加到表格数据中
                table_data.push(cells);
            }
        }
    }
    
    // 返回解析完成的表格数据
    table_data
}

// 在neovim中打开TODO文件
fn open_todo_in_neovim(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // 暂时离开备用屏幕和原始模式
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    disable_raw_mode()?;
    
    // 调用neovim编辑器
    let status = Command::new("nvim")
        .arg(&config.todo_file_path)
        .status()?;
    
    // 完全重置终端状态
    execute!(
        std::io::stdout(),
        Clear(ClearType::All),
        Clear(ClearType::Purge),
        MoveTo(1, 1)
    )?;
    
    // 重新进入备用屏幕和原始模式
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    
    // 强制刷新整个终端
    std::io::stdout().flush()?;
    
    if !status.success() {
        return Err("Failed to open neovim".into());
    }
    
    Ok(())
}

// 主函数
// enum Result<T, E> {
    // Ok(T),
    // Err(E),
// }
/*
Ok 的值类型是 ()
() 是"单位类型"，代表"没有实际值"，类似于其他语言的 void, 读作 unit。
Ok(()) 表示"成功执行，但没有返回任何值"。

Err 的值类型是 Box<dyn std::error::Error>
dyn std::error::Error 表示"某个实现了 std::error::Error 特征的不定具体类型"
默认情况下，Rust 的值都在栈上分配，生命周期和作用域绑定，速度快但容量小。
Box<T> 会把值放到堆上，而自己（一个固定大小的指针）留在栈上。
当 Box 被销毁时，它会自动释放堆上的数据（借助 Drop）。
这样设计可以让 main 返回各种不同错误（I/O 错误、TUI 初始化错误等），而不用统一成一个固定的错误枚举。

Rust 的独特性：类型不靠继承，而靠"实现特征"来表达能力。
*/
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 加载配置
    let config = load_config();
    
    // 开启终端原始模式：禁用行缓冲和回显，便于即时读取键盘事件
    // 以类 Unix 终端为例，Shell 默认是熟（canonical/cooked）模式：内核先把你敲的字存到行缓冲里，直到回车，再一次性交给程序；同时还会回显、行编辑（退格）、把某些组合键当成信号（如 Ctrl+C → SIGINT）等。
    // raw mode（原始模式）就是把这些加工关掉，让程序直接收到即时的原始按键/字节，方便做 TUI/游戏 等交互。
    enable_raw_mode()?; // `?` 操作符：如果出错，直接返回 Err；否则提取 Ok 值

    // 获取标准输出句柄（可变），后续用于附着到 TUI 后端
    let mut stdout = io::stdout();

    // 切换到备用屏幕并启用鼠标捕获（不会影响主屏幕内容）
    // execute! 是 crossterm 的宏，向给定的 writer（这里是 stdout）写入控制序列。 
    // 备用屏幕缓冲区是一个全新的、空白的缓冲区，与主屏幕互不干扰。
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    // 用标准输出创建 Crossterm 后端
    let backend = CrosstermBackend::new(stdout);

    // 基于后端构建 ratatui 的 Terminal 对象
    let mut terminal = Terminal::new(backend)?;

    // 运行应用的主循环；使用可变借用传入 terminal
    // 将结果暂存，确保即使出错也会进行资源清理
    let result = run_app(&mut terminal, &config);

    // 资源清理：恢复原始模式、切回主屏幕、关闭鼠标捕获、显示光标
    // terminal.backend_mut()：拿到底层后端的可变引用，以便再次发控制序列。
    // 使用 .ok() 忽略清理过程中潜在的错误（避免覆盖主错误）
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture).ok();
    // terminal.show_cursor().ok();

    // 返回应用运行结果
    result
}

// 应用主循环：接收一个对终端的可变借用
fn run_app(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // 当前选中的菜单索引（usize 为平台相关的无符号整数类型）
    let mut selected = 0usize;
    // 所有菜单项（固定数组）
    let items = MenuItem::all();
    // 最近一次提示信息：Option 表示"可能有/可能无"
    /*
    enum Option<T> {
        Some(T),
        None,
    }
    Rust 用 Option<T> 来显式地表示"可空值"，让编译器强制在使用前检查。
    */
    let mut last_message: Option<String> = None;
    
    // 创建列表状态来跟踪选中项
    let mut list_state = ListState::default();
    list_state.select(Some(0)); // 初始选中第一项
    
    // 应用状态
    let mut app_state = AppState::MainMenu;
    
    // 强制重绘标志
    let mut force_redraw = false;
    
    // TODO表格数据
    // TODO表格数据
    let mut todo_data: Vec<Vec<String>> = Vec::new();

    // 无限循环，直到通过 break 退出
    loop {
        // 更新 list_state 的选中项
        list_state.select(Some(selected));
        
        // 如果需要强制重绘，先清除终端
        if force_redraw {
            terminal.clear()?;
            force_redraw = false;
        }
        
        terminal.draw(|f| {
            // 获取可用区域大小（整个终端窗口）
            let size = f.size();

            match app_state {
                AppState::MainMenu => {
                    // 顶层布局：垂直方向分成三块：头部、菜单、底部
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),   // 头部高度固定 3 行
                            Constraint::Min(5),      // 菜单区域最少 5 行，剩余空间也给它
                            Constraint::Length(3),   // 底部高度固定 3 行（边框占 2 行，至少留 1 行内容）
                        ])
                        .split(size);

                    // 头部：显示欢迎文本
                    // 声明变量 header，构建一个 Paragraph 小部件（widget）。
                    /*
                        Paragraph::new(...) 接受 <Text>。Line 可被转换为 Text，因此可直接传入。
                        Line::from(vec![ ... ]) 用一组 Span 组成一行文本；vec![] 在堆上存储多个片段。
                    */
                    /*
                        Span::styled(text, style)：把样式与文本绑定成一个片段。
                        Style::default()：从默认样式开始（无颜色、无修饰）。
                        .fg(Color::Cyan)：设置前景色为青色。
                        .add_modifier(Modifier::BOLD)：添加粗体修饰。Modifier 是位标志（bitflags），可叠加多个修饰。 
                    */
                    let header = Paragraph::new(Line::from(vec![
                            // 使用 Span 设置不同片段的样式
                            Span::styled("Jeek!", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                            Span::raw("\t Exist before meaning, feel yourself, embrace imperfection."),
                        ]))
                        .alignment(Alignment::Center) // 居中对齐
                        .block(Block::default().borders(Borders::ALL).title("Hello"));
                    // 将头部控件渲染到第一块区域
                    f.render_widget(header, chunks[0]);

                    // 菜单列表：将枚举项映射为 ListItem
                    /*
                        从一个可迭代容器 items 中生成一组 ListItem。
                        .iter() 产生迭代器，每次产生 &item。
                        .enumerate() 给每个元素附带一个索引 i。
                        .map(|(i, item)| {...}) 把 (索引, 元素) 映射成一个新的类型——这里是 ListItem。
                        .collect() 收集所有 ListItem 成为一个 Vec<ListItem>，最终赋值给变量 list_items。
                    */
                    let list_items: Vec<ListItem> = items.iter().enumerate().map(|(i, item)| {
                        let style = if i == selected {
                            Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        // 生成一个 ListItem（行内使用标题文本）
                        ListItem::new(Line::from(Span::styled(format!(" {} : {} ", i, item.title()), style)))
                    }).collect();

                    // 构建列表控件，并设置外框、标题、高亮样式与符号
                    let list = List::new(list_items)
                        .block(Block::default().borders(Borders::ALL).title("Menu"))
                        .highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
                        .highlight_symbol("→ ");
                    
                    // 使用 render_stateful_widget 而不是 render_widget
                    f.render_stateful_widget(list, chunks[1], &mut list_state);
                    
                    // 底部：帮助提示或最近消息
                    let help_text = match &last_message {
                        // 如果有消息，显示其内容（&String.as_str() -> &str）
                        // 可以用 string 得到 &str
                        // 不能直接从 &str 变成 String，需 to_string() 或 String::from()
                        Some(msg) => msg.as_str(),
                        // 否则显示默认帮助信息（字符串字面量具有 'static 生命周期）
                        None => "\tjk -- move, Enter -- select, q -- exit",
                    };
                    let footer = Paragraph::new(help_text)
                        .alignment(Alignment::Left)
                        .block(Block::default().borders(Borders::ALL).title("Message"));
                    // 渲染到第三块区域
                    f.render_widget(footer, chunks[2]);
                }
                AppState::TodoView => {
                    // TODO视图布局
                    let chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),   // 头部
                            Constraint::Min(5),      // 表格区域
                            Constraint::Length(3),   // 底部帮助
                        ])
                        .split(size);

                    // 头部
                    let header = Paragraph::new("TODO List")
                        .alignment(Alignment::Center)
                        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                        .block(Block::default().borders(Borders::ALL).title("TODO"));
                    f.render_widget(header, chunks[0]);

                    // 表格
                    if !todo_data.is_empty() {
                        let rows: Vec<Row> = todo_data.iter().enumerate().map(|(i, row)| {
                            let cells: Vec<Cell> = row.iter().map(|cell| {
                                Cell::from(cell.as_str())
                            }).collect();
                            
                            let style = if i == 0 {
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            };
                            
                            Row::new(cells).style(style)
                        }).collect();

                        let table = Table::new(rows, [Constraint::Percentage(33), Constraint::Percentage(33), Constraint::Percentage(34)])
                            .block(Block::default().borders(Borders::ALL).title("Tasks"));
                        
                        f.render_widget(table, chunks[1]);
                    } else {
                        let empty_msg = Paragraph::new("No TODO items found")
                            .alignment(Alignment::Center)
                            .block(Block::default().borders(Borders::ALL));
                        f.render_widget(empty_msg, chunks[1]);
                    }

                    // 底部帮助
                    let help = Paragraph::new("q -- back to menu | e -- edit in neovim | r -- refresh")
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).title("Help"));
                    f.render_widget(help, chunks[2]);
                }
            }
        })?; // 如果绘制出错，使用 `?` 传递错误

        // 处理输入事件（带超时轮询，以便界面能周期性刷新）
        if event::poll(std::time::Duration::from_millis(250))? {
            // 读取一个事件, 如果是键盘事件, 则处理, 否则忽略
            if let Event::Key(key) = event::read()? {
                // 只处理按下（而不是释放/重复）的键
                if key.kind == KeyEventKind::Press {
                    match app_state {
                        AppState::MainMenu => {
                            match key.code {
                                // q 或 Esc 退出
                                KeyCode::Char('q') | KeyCode::Esc => break,
                                // 上移选择（支持方向键或 vim 风格 k）
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if selected == 0 { selected = items.len() - 1; } else { selected -= 1; }
                                    last_message = None; // 移动时清空提示
                                }
                                // 下移选择（支持方向键或 vim 风格 j）
                                KeyCode::Down | KeyCode::Char('j') => {
                                    selected = (selected + 1) % items.len();
                                    last_message = None; // 移动时清空提示
                                }
                                // 回车：进入选中项
                                KeyCode::Enter => {
                                    match items[selected] {
                                        MenuItem::Todo => {
                                            // 读取TODO数据
                                            match read_todo_file(config) {
                                                Ok(content) => {
                                                    todo_data = parse_todo_table(&content);
                                                    app_state = AppState::TodoView;
                                                    last_message = None;
                                                }
                                                Err(e) => {
                                                    last_message = Some(format!("Error reading TODO file: {}", e));
                                                }
                                            }
                                        }
                                        _ => {
                                            let title = items[selected].title();
                                            last_message = Some(format!("已选择 {}（功能未实现）", title));
                                        }
                                    }
                                }
                                // 其他按键不处理
                                _ => {}
                            }
                        }
                        AppState::TodoView => {
                            match key.code {
                                // q 返回主菜单
                                KeyCode::Char('q') => {
                                    app_state = AppState::MainMenu;
                                }
                                // e 编辑文件
                                KeyCode::Char('e') => {
                                    match open_todo_in_neovim(config) {
                                        Ok(_) => {
                                            // 重新读取文件
                                            match read_todo_file(config) {
                                                Ok(content) => {
                                                    todo_data = parse_todo_table(&content);
                                                    // 设置强制重绘标志，确保整个界面重新绘制
                                                    force_redraw = true;
                                                }
                                                Err(_) => {
                                                    // 如果读取失败，返回主菜单
                                                    app_state = AppState::MainMenu;
                                                    force_redraw = true;
                                                }
                                            }
                                        }
                                        Err(_) => {
                                            // 如果编辑失败，返回主菜单
                                            app_state = AppState::MainMenu;
                                            force_redraw = true;
                                        }
                                    }
                                }
                                // r 刷新
                                KeyCode::Char('r') => {
                                    match read_todo_file(config) {
                                        Ok(content) => {
                                            todo_data = parse_todo_table(&content);
                                        }
                                        Err(_) => {
                                            // 如果读取失败，返回主菜单
                                            app_state = AppState::MainMenu;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    // 循环正常退出，返回 Ok
    Ok(())
}