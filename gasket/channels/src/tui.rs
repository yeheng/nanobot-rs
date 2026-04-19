//! TUI (Terminal User Interface) channel for gasket.
//!
//! Provides a local terminal-based chat interface using `ratatui` and `crossterm`.
//! This channel operates as a first-class citizen in the channel system:
//! - Inbound: user keyboard input → `InboundMessage` → broker
//! - Outbound: agent responses from broker → TUI display
//!
//! The TUI loop runs synchronously inside a `spawn_blocking` task.  Agent
//! responses are pushed into a shared queue by the async `send()` method and
//! drained by the synchronous render loop on every frame.

use std::io;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;
use ratatui::Terminal;
use tracing::error;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::adapter::ImAdapter;
use crate::events::{ChannelType, InboundMessage, OutboundMessage};
use crate::middleware::InboundSender;

// ── Message Types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
struct Message {
    role: MessageRole,
    content: String,
    thinking: Option<String>,
}

impl Message {
    fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            thinking: None,
        }
    }
    fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            thinking: None,
        }
    }
    fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            thinking: None,
        }
    }
    fn thinking(&mut self, text: impl Into<String>) {
        self.thinking = Some(text.into());
    }
    fn append(&mut self, text: &str) {
        self.content.push_str(text);
    }
}

// ── App State ──────────────────────────────────────────────────────────────

struct App {
    messages: Vec<Message>,
    input: String,
    cursor_pos: usize,
    scroll: usize,
    auto_scroll: bool,
    status: String,
    token_info: String,
    show_help: bool,
    waiting: bool,
    last_assistant_idx: Option<usize>,
}

impl App {
    fn new() -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll: 0,
            auto_scroll: true,
            status: "Ready".to_string(),
            token_info: String::new(),
            show_help: false,
            waiting: false,
            last_assistant_idx: None,
        }
    }

    fn push_user(&mut self, content: &str) {
        self.messages.push(Message::user(content));
        self.last_assistant_idx = None;
        self.scroll_to_bottom();
    }
    fn start_assistant(&mut self) {
        let idx = self.messages.len();
        self.messages.push(Message::assistant(""));
        self.last_assistant_idx = Some(idx);
        self.scroll_to_bottom();
    }
    fn append_to_last(&mut self, text: &str) {
        if let Some(idx) = self.last_assistant_idx {
            self.messages[idx].append(text);
            if self.auto_scroll {
                self.scroll = self.messages.len().saturating_sub(1);
            }
        }
    }
    fn set_thinking(&mut self, text: &str) {
        if let Some(idx) = self.last_assistant_idx {
            self.messages[idx].thinking(text);
        }
    }
    fn push_system(&mut self, content: &str) {
        self.messages.push(Message::system(content));
        self.scroll_to_bottom();
    }
    fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
        self.auto_scroll = false;
    }
    fn scroll_down(&mut self, n: usize) {
        self.scroll = (self.scroll + n).min(self.messages.len().saturating_sub(1));
        if self.scroll >= self.messages.len().saturating_sub(1) {
            self.auto_scroll = true;
        }
    }
    fn scroll_to_bottom(&mut self) {
        if !self.messages.is_empty() {
            self.scroll = self.messages.len() - 1;
        }
        self.auto_scroll = true;
    }
    fn clear_messages(&mut self) {
        self.messages.clear();
        self.last_assistant_idx = None;
        self.scroll = 0;
        self.auto_scroll = true;
    }
    fn move_cursor_left(&mut self) {
        let before = &self.input[..self.cursor_pos];
        if let Some((idx, _)) = before.char_indices().next_back() {
            self.cursor_pos = idx;
        } else {
            self.cursor_pos = 0;
        }
    }
    fn move_cursor_right(&mut self) {
        let after = &self.input[self.cursor_pos..];
        if let Some((idx, c)) = after.char_indices().next() {
            self.cursor_pos = idx + c.len_utf8();
        }
    }
    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }
    fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let before = &self.input[..self.cursor_pos];
            if let Some((idx, _)) = before.char_indices().next_back() {
                self.input.drain(idx..self.cursor_pos);
                self.cursor_pos = idx;
            }
        }
    }
    fn delete_char(&mut self) {
        if self.cursor_pos < self.input.len() {
            let after = &self.input[self.cursor_pos..];
            if let Some((idx, c)) = after.char_indices().next() {
                let end = self.cursor_pos + idx + c.len_utf8();
                self.input.drain(self.cursor_pos..end);
            }
        }
    }
    fn move_cursor_home(&mut self) {
        self.cursor_pos = 0;
    }
    fn move_cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }
    fn take_input(&mut self) -> String {
        let text = std::mem::take(&mut self.input);
        self.cursor_pos = 0;
        text
    }
}

// ── UI Rendering ───────────────────────────────────────────────────────────

fn draw_ui(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);

    // Status bar
    let status_style = Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let status_text = if app.token_info.is_empty() {
        format!(" 🐈 gasket TUI | {} ", app.status)
    } else {
        format!(" 🐈 gasket TUI | {} | {} ", app.status, app.token_info)
    };
    frame.render_widget(
        Paragraph::new(status_text)
            .style(status_style)
            .alignment(Alignment::Left),
        chunks[0],
    );

    // Messages area
    let messages_block = Block::default().borders(Borders::ALL).title(" Chat ");
    let messages_area = messages_block.inner(chunks[1]);
    frame.render_widget(messages_block, chunks[1]);
    render_messages(frame, app, messages_area);

    // Input area
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(if app.waiting {
            " Input (waiting...) "
        } else {
            " Input "
        });
    let input_style = if app.waiting {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };
    let input_text = format!("> {}", app.input);
    frame.render_widget(
        Paragraph::new(input_text)
            .block(input_block)
            .style(input_style),
        chunks[2],
    );

    if !app.waiting {
        let cursor_x = chunks[2].x + 2 + app.input[..app.cursor_pos].width() as u16;
        let cursor_y = chunks[2].y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    if app.show_help {
        render_help(frame, area);
    }
}

fn render_messages(frame: &mut Frame, app: &App, area: Rect) {
    if app.messages.is_empty() {
        let hint = Paragraph::new("Type a message and press Enter to chat. Press '?' for help.")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(hint, area);
        return;
    }

    let visible_height = area.height as usize;
    let total = app.messages.len();
    let start = app.scroll;
    let end = (start + visible_height).min(total);

    let items: Vec<Line> = app.messages[start..end]
        .iter()
        .flat_map(|msg| message_to_lines(msg, area.width as usize))
        .collect();

    let para = Paragraph::new(Text::from(items)).wrap(Wrap { trim: true });
    frame.render_widget(para, area);

    if total > visible_height {
        let mut scrollbar_state = ScrollbarState::new(total)
            .position(app.scroll)
            .viewport_content_length(visible_height);
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin {
                horizontal: 0,
                vertical: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

fn message_to_lines(msg: &Message, max_width: usize) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    match msg.role {
        MessageRole::User => {
            lines.push(Line::from(Span::styled(
                "You",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            for l in wrap_text(&msg.content, max_width.saturating_sub(2)) {
                lines.push(Line::from(Span::raw(l)));
            }
        }
        MessageRole::Assistant => {
            lines.push(Line::from(Span::styled(
                "Agent",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            if let Some(ref thinking) = msg.thinking {
                if !thinking.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "┌─ Thinking ──",
                        Style::default().fg(Color::DarkGray),
                    )));
                    for l in wrap_text(thinking, max_width.saturating_sub(4)) {
                        lines.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                l,
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                    }
                    lines.push(Line::from(Span::styled(
                        "└─────────────",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            for l in wrap_text(&msg.content, max_width.saturating_sub(2)) {
                lines.push(Line::from(Span::raw(l)));
            }
        }
        MessageRole::System => {
            let style = if msg.content.starts_with("Error") {
                Style::default().fg(Color::Red)
            } else if msg.content.starts_with("Tool") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Blue)
            };
            for l in wrap_text(&msg.content, max_width.saturating_sub(2)) {
                lines.push(Line::from(Span::styled(l, style)));
            }
        }
    }
    lines.push(Line::from(""));
    lines
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return text.lines().map(|s| s.to_string()).collect();
    }
    let mut result = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }
        let mut current = String::new();
        for ch in line.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if current.width() + ch_width > width && !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            current.push(ch);
        }
        if !current.is_empty() {
            result.push(current);
        }
    }
    result
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help_text = r#"
    🐈 gasket TUI Help

    Commands:
      /new    Start a new conversation
      /help   Show this help
      /exit   Exit the TUI

    Keys:
      Enter        Send message
      Ctrl+C       Exit
      ↑ / ↓        Scroll messages
      PageUp       Scroll up faster
      PageDown     Scroll down faster
      Home         Move cursor to start
      End          Move cursor to end
      ?            Toggle this help
    "#;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(Style::default().bg(Color::Black));
    let para = Paragraph::new(help_text)
        .block(block)
        .wrap(Wrap { trim: true });
    let popup_area = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup_area);
    frame.render_widget(para, popup_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ── Outbound Queue ─────────────────────────────────────────────────────────

/// Thread-safe queue for outbound messages.
/// The async `send()` pushes into this queue; the sync TUI loop drains it.
#[derive(Clone)]
pub struct OutboundQueue {
    inner: Arc<Mutex<Vec<OutboundMessage>>>,
}

impl Default for OutboundQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl OutboundQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn push(&self, msg: OutboundMessage) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.push(msg);
        }
    }
    pub fn drain(&self) -> Vec<OutboundMessage> {
        self.inner
            .lock()
            .map_or_else(|_| Vec::new(), |mut guard| std::mem::take(&mut *guard))
    }
}

// ── TuiAdapter ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TuiAdapter {
    queue: OutboundQueue,
}

impl Default for TuiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiAdapter {
    pub fn new() -> Self {
        Self {
            queue: OutboundQueue::new(),
        }
    }

    pub fn from_config(_cfg: &crate::config::TuiConfig) -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImAdapter for TuiAdapter {
    fn name(&self) -> &str {
        "tui"
    }

    async fn start(&self, inbound: InboundSender) -> anyhow::Result<()> {
        let queue = self.queue.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = run_tui_loop(queue, inbound) {
                error!("TUI loop error: {}", e);
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("TUI task failed: {}", e))?;

        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> anyhow::Result<()> {
        self.queue.push(msg.clone());
        Ok(())
    }
}

// ── Synchronous TUI Loop ───────────────────────────────────────────────────

fn run_tui_loop(queue: OutboundQueue, inbound: InboundSender) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(crossterm::event::EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let mut _last_outbound_count = 0usize;

    // Crossterm event channel (sync)
    let (event_tx, event_rx) = std::sync::mpsc::channel::<CrosstermEvent>();
    std::thread::spawn(move || {
        while let Ok(evt) = event::read() {
            if event_tx.send(evt).is_err() {
                break;
            }
        }
    });

    // Initial draw
    terminal.draw(|f| draw_ui(f, &app))?;

    let runtime = tokio::runtime::Handle::current();
    let result = 'main: loop {
        // Poll outbound queue
        let outbound = queue.drain();
        for msg in outbound {
            apply_outbound_message(&mut app, &msg);
        }

        // Poll crossterm events (non-blocking)
        match event_rx.try_recv() {
            Ok(CrosstermEvent::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break 'main Ok(())
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break 'main Ok(())
                }
                KeyCode::Char('?') | KeyCode::Char('/') if !app.waiting && app.input.is_empty() => {
                    app.show_help = !app.show_help;
                }
                KeyCode::Esc => app.show_help = false,
                KeyCode::Up => app.scroll_up(1),
                KeyCode::Down => app.scroll_down(1),
                KeyCode::PageUp => app.scroll_up(5),
                KeyCode::PageDown => app.scroll_down(5),
                KeyCode::Home => app.move_cursor_home(),
                KeyCode::End => app.move_cursor_end(),
                KeyCode::Left => app.move_cursor_left(),
                KeyCode::Right => app.move_cursor_right(),
                KeyCode::Enter if !app.waiting && !app.show_help => {
                    let line = app.take_input();
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let cmd = trimmed.to_lowercase();
                    if cmd == "/exit" || cmd == "/quit" || cmd == ":q" {
                        break 'main Ok(());
                    }
                    if cmd == "/help" {
                        app.show_help = true;
                        continue;
                    }
                    if cmd == "/new" {
                        app.clear_messages();
                        app.status = "New session started".to_string();
                        app.token_info.clear();
                        continue;
                    }

                    app.push_user(trimmed);
                    app.waiting = true;
                    app.status = "Thinking...".to_string();
                    app.scroll_to_bottom();

                    let inbound_msg = InboundMessage {
                        channel: ChannelType::Custom("tui".to_string()),
                        sender_id: "tui".to_string(),
                        chat_id: "tui".to_string(),
                        content: trimmed.to_string(),
                        media: None,
                        metadata: None,
                        timestamp: chrono::Utc::now(),
                        trace_id: None,
                    };
                    if let Err(e) = runtime.block_on(inbound.send(inbound_msg)) {
                        app.push_system(&format!("Error sending message: {}", e));
                        app.waiting = false;
                        app.status = "Error".to_string();
                    }
                }
                KeyCode::Backspace if !app.waiting && !app.show_help => app.backspace(),
                KeyCode::Delete if !app.waiting && !app.show_help => app.delete_char(),
                KeyCode::Char(c) if !app.waiting && !app.show_help => app.insert_char(c),
                _ => {}
            },
            Ok(CrosstermEvent::Mouse(mouse)) => match mouse.kind {
                MouseEventKind::ScrollUp => app.scroll_up(3),
                MouseEventKind::ScrollDown => app.scroll_down(3),
                _ => {}
            },
            Ok(CrosstermEvent::Resize(_, _)) => {}
            Ok(_) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // No event, sleep a bit to avoid busy-waiting
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'main Ok(()),
        }

        // Redraw
        if let Err(e) = terminal.draw(|f| draw_ui(f, &app)) {
            error!("Failed to draw TUI: {}", e);
        }
    };

    // Cleanup
    let stdout = terminal.backend_mut();
    let _ = stdout.execute(crossterm::event::DisableMouseCapture);
    let _ = stdout.execute(LeaveAlternateScreen);
    let _ = disable_raw_mode();

    result
}

fn apply_outbound_message(app: &mut App, msg: &OutboundMessage) {
    // Try to parse as ChatEvent JSON for structured content
    if let Some(ref ws_msg) = msg.ws_message {
        apply_chat_event(app, ws_msg);
        return;
    }

    // Plain text fallback
    if !msg.content.is_empty() {
        // Check if this is a new message or continuation
        // For simplicity, we treat each outbound text as a new assistant message
        // unless we already have an empty assistant message at the end
        let needs_new = match app.last_assistant_idx {
            Some(idx) => !app.messages[idx].content.is_empty(),
            None => true,
        };
        if needs_new {
            app.start_assistant();
        }
        app.append_to_last(&msg.content);
        app.status = "Ready".to_string();
        app.waiting = false;
    }
}

fn apply_chat_event(app: &mut App, event: &crate::events::WebSocketMessage) {
    use crate::events::ChatEvent;
    match event {
        ChatEvent::Content { content } => {
            // Ensure we have an active assistant message
            if app.last_assistant_idx.is_none() {
                app.start_assistant();
            }
            app.append_to_last(content);
            app.status = "Responding...".to_string();
        }
        ChatEvent::Thinking { content } => {
            if app.last_assistant_idx.is_none() {
                app.start_assistant();
            }
            app.set_thinking(content);
        }
        ChatEvent::ToolStart { name, arguments } => {
            let text = if let Some(args) = arguments {
                format!("Tool: {} {}", name, args)
            } else {
                format!("Tool: {}", name)
            };
            app.push_system(&text);
        }
        ChatEvent::ToolEnd { name, output } => {
            let text = if let Some(out) = output {
                format!("Tool {} done: {}", name, out)
            } else {
                format!("Tool {} done", name)
            };
            app.push_system(&text);
        }
        ChatEvent::Done => {
            app.status = "Ready".to_string();
            app.waiting = false;
        }
        ChatEvent::Error { message } => {
            app.push_system(&format!("Error: {}", message));
            app.status = "Error".to_string();
            app.waiting = false;
        }
        ChatEvent::Text { content } => {
            if app.last_assistant_idx.is_none() {
                app.start_assistant();
            }
            app.append_to_last(content);
        }
        _ => {}
    }
}
