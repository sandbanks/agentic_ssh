use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
    }
}

#[derive(Debug, Clone)]
pub struct HostStatus {
    pub name: String,
    pub status: String, // "Running", "Success", "Failed"
    pub exit_code: Option<u32>,
    pub log_path: std::path::PathBuf,
    pub log_lines: Vec<String>,
}

pub struct WatchState {
    pub hosts: Vec<HostStatus>,
    pub selected_index: usize,
    pub is_teardown: bool,
    pub first_ctrl_c: bool,
}

struct LineBuffer {
    buffer: String,
}

impl LineBuffer {
    fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    fn push(&mut self, new_str: &str, log_lines: &mut Vec<String>, prefix: &str) {
        self.buffer.push_str(new_str);
        if self.buffer.contains('\n') {
            let mut parts: Vec<&str> = self.buffer.split('\n').collect();
            let last = parts.pop().unwrap_or("");
            for part in parts {
                log_lines.push(format!("{}{}", prefix, part));
            }
            self.buffer = last.to_string();
        }
    }

    fn flush_remaining(&mut self, log_lines: &mut Vec<String>, prefix: &str) {
        if !self.buffer.is_empty() {
            log_lines.push(format!("{}{}", prefix, self.buffer));
            self.buffer.clear();
        }
    }
}

async fn run_worker(
    host: String,
    command: String,
    pool: Arc<crate::ssh_pool::ConnectionPool>,
    state: Arc<Mutex<WatchState>>,
    channel_slot: Arc<Mutex<Option<russh::ChannelWriteHalf<russh::client::Msg>>>>,
    host_index: usize,
) {
    let sessions_dir = match home::home_dir() {
        Some(hd) => hd.join(".agentic_ssh").join("sessions"),
        None => std::path::PathBuf::from(".")
            .join(".agentic_ssh")
            .join("sessions"),
    };
    let _ = std::fs::create_dir_all(&sessions_dir);
    let now_str = chrono::Local::now().format("%Y%m%d_%H%M").to_string();
    let rand_hex = format!(
        "{:04x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            & 0xffff
    );
    let log_file_name = format!("watch_{}_{}_{}.log", now_str, host, rand_hex);
    let log_path = sessions_dir.join(log_file_name);

    {
        let mut s = state.lock().unwrap();
        if let Some(h) = s.hosts.get_mut(host_index) {
            h.log_path = log_path.clone();
        }
    }

    let mut log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .await
        .ok();

    use tokio::io::AsyncWriteExt;
    if let Some(ref mut f) = log_file {
        let header = format!(
            "--- Executing watch command on {}: {:?} ---\n",
            host, command
        );
        let _ = f.write_all(header.as_bytes()).await;
        let _ = f.flush().await;
    }

    let handle = match pool.get_connection(&host).await {
        Ok(h) => h,
        Err(e) => {
            let err_msg = format!("Connection failed: {}\n", e);
            if let Some(ref mut f) = log_file {
                let _ = f.write_all(err_msg.as_bytes()).await;
            }
            let mut s = state.lock().unwrap();
            if let Some(h) = s.hosts.get_mut(host_index) {
                h.status = "Failed".to_string();
                h.log_lines.push(err_msg);
            }
            return;
        }
    };

    let channel = match handle.channel_open_session().await {
        Ok(c) => c,
        Err(e) => {
            let err_msg = format!("Failed to open SSH channel: {}\n", e);
            if let Some(ref mut f) = log_file {
                let _ = f.write_all(err_msg.as_bytes()).await;
            }
            let mut s = state.lock().unwrap();
            if let Some(h) = s.hosts.get_mut(host_index) {
                h.status = "Failed".to_string();
                h.log_lines.push(err_msg);
            }
            return;
        }
    };

    if let Err(e) = channel.exec(true, command.as_str()).await {
        let err_msg = format!("Failed to request command execution: {}\n", e);
        if let Some(ref mut f) = log_file {
            let _ = f.write_all(err_msg.as_bytes()).await;
        }
        let mut s = state.lock().unwrap();
        if let Some(h) = s.hosts.get_mut(host_index) {
            h.status = "Failed".to_string();
            h.log_lines.push(err_msg);
        }
        return;
    }

    let (mut read_half, write_half) = channel.split();

    {
        let mut slot = channel_slot.lock().unwrap();
        *slot = Some(write_half);
    }

    let mut exit_code = None;
    let mut stdout_buf = LineBuffer::new();
    let mut stderr_buf = LineBuffer::new();

    loop {
        tokio::select! {
            msg = read_half.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { data }) => {
                        if let Some(ref mut f) = log_file {
                            let _ = f.write_all(&data).await;
                            let _ = f.flush().await;
                        }
                        let parsed = String::from_utf8_lossy(&data);
                        let mut s = state.lock().unwrap();
                        if let Some(h) = s.hosts.get_mut(host_index) {
                            stdout_buf.push(&parsed, &mut h.log_lines, "");
                            if h.log_lines.len() > 1000 {
                                h.log_lines.drain(0..(h.log_lines.len() - 1000));
                            }
                        }
                    }
                    Some(russh::ChannelMsg::ExtendedData { data, ext }) => {
                        if ext == 1 {
                            if let Some(ref mut f) = log_file {
                                let _ = f.write_all(&data).await;
                                let _ = f.flush().await;
                            }
                            let parsed = String::from_utf8_lossy(&data);
                            let mut s = state.lock().unwrap();
                            if let Some(h) = s.hosts.get_mut(host_index) {
                                stderr_buf.push(&parsed, &mut h.log_lines, "[STDERR] ");
                                if h.log_lines.len() > 1000 {
                                    h.log_lines.drain(0..(h.log_lines.len() - 1000));
                                }
                            }
                        }
                    }
                    Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                        exit_code = Some(exit_status);
                    }
                    None => break,
                    _ => {}
                }
            }
        }
    }

    {
        let mut s = state.lock().unwrap();
        if let Some(h) = s.hosts.get_mut(host_index) {
            stdout_buf.flush_remaining(&mut h.log_lines, "");
            stderr_buf.flush_remaining(&mut h.log_lines, "[STDERR] ");
            h.exit_code = exit_code;
            if let Some(code) = exit_code {
                if code == 0 {
                    h.status = "Success".to_string();
                } else {
                    h.status = "Failed".to_string();
                }
            } else {
                h.status = "Failed".to_string();
            }
        }
    }
}

fn get_grid_layout(area: ratatui::layout::Rect, num_hosts: usize) -> Vec<ratatui::layout::Rect> {
    use ratatui::layout::{Constraint, Direction, Layout};
    if num_hosts == 0 {
        return vec![];
    }
    if num_hosts == 1 {
        return vec![area];
    }
    if num_hosts == 2 {
        return Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area)
            .to_vec();
    }
    if num_hosts <= 4 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        let mut cells = Vec::new();
        for row in rows.iter() {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(*row);
            cells.extend(cols.to_vec());
        }
        cells.truncate(num_hosts);
        return cells;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let mut cells = Vec::new();
    let mid = num_hosts.div_ceil(2);

    let r0_cols = mid;
    let r1_cols = num_hosts - mid;

    let c0 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, r0_cols as u32); r0_cols])
        .split(rows[0]);
    cells.extend(c0.to_vec());

    let c1 = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, r1_cols as u32); r1_cols])
        .split(rows[1]);
    cells.extend(c1.to_vec());

    cells
}

fn draw_teardown_overlay(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    use ratatui::layout::{Alignment, Constraint, Direction, Layout};
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

    let msg = "\nDisconnecting from hosts and terminating remote processes...\n\n[Press Ctrl+C again to force quit]";
    let paragraph = Paragraph::new(msg).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Teardown in Progress ")
            .title_alignment(Alignment::Center)
            .style(Style::default().fg(Color::Yellow)),
    );

    let vertical_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(area);

    let horizontal_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(vertical_layout[1]);

    f.render_widget(Clear, horizontal_layout[1]);
    f.render_widget(paragraph, horizontal_layout[1]);
}

fn draw_ui(f: &mut ratatui::Frame, state: &WatchState) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

    let full_area = f.area();

    let is_complete = state.hosts.iter().all(|h| h.status != "Running");

    let (size, status_area) = if is_complete {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(full_area);
        (chunks[0], Some(chunks[1]))
    } else {
        (full_area, None)
    };

    let threshold = 6;
    if state.hosts.len() > threshold {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
            .split(size);

        let items: Vec<ListItem> = state
            .hosts
            .iter()
            .enumerate()
            .map(|(idx, host)| {
                let emoji = match host.status.as_str() {
                    "Running" => "🟡",
                    "Success" => "✅",
                    "Failed" => "🔴",
                    _ => "⚪",
                };
                let style = if idx == state.selected_index {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{} {} - {}", emoji, host.name, host.status)).style(style)
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Target Fleet "),
        );
        f.render_widget(list, chunks[0]);

        if let Some(host) = state.hosts.get(state.selected_index) {
            let height = chunks[1].height as usize;
            let log_len = host.log_lines.len();
            let start = log_len.saturating_sub(height.saturating_sub(2));
            let visible_lines = &host.log_lines[start..];

            let text = visible_lines.join("\n");
            let paragraph = Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Logs: {} ({}) ", host.name, host.status)),
            );
            f.render_widget(paragraph, chunks[1]);
        }
    } else {
        let grid_cells = get_grid_layout(size, state.hosts.len());
        for (idx, host) in state.hosts.iter().enumerate() {
            if idx >= grid_cells.len() {
                break;
            }
            let cell = grid_cells[idx];
            let emoji = match host.status.as_str() {
                "Running" => "🟡",
                "Success" => "✅",
                "Failed" => "🔴",
                _ => "⚪",
            };

            let height = cell.height as usize;
            let log_len = host.log_lines.len();
            let start = log_len.saturating_sub(height.saturating_sub(2));
            let visible_lines = &host.log_lines[start..];
            let text = visible_lines.join("\n");

            let border_color = match host.status.as_str() {
                "Success" => Color::Green,
                "Failed" => Color::Red,
                _ => Color::Yellow,
            };

            let paragraph = Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color))
                    .title(format!(" {} {} ({}) ", emoji, host.name, host.status)),
            );
            f.render_widget(paragraph, cell);
        }
    }

    if let Some(area) = status_area {
        let total = state.hosts.len();
        let succeeded = state.hosts.iter().filter(|h| h.status == "Success").count();
        let failed = state.hosts.iter().filter(|h| h.status == "Failed").count();

        let (status_text, style) = if failed > 0 {
            (
                format!(
                    " ✘ Execution complete. Succeeded: {}/{}, Failed: {}. Press Esc or Ctrl+C to exit. ",
                    succeeded, total, failed
                ),
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                format!(
                    " ✔ Execution complete. All {} hosts succeeded. Press Esc or Ctrl+C to exit. ",
                    total
                ),
                Style::default()
                    .bg(Color::Green)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
        };

        let status_bar = Paragraph::new(status_text).style(style);
        f.render_widget(status_bar, area);
    }

    if state.is_teardown {
        draw_teardown_overlay(f, size);
    }
}

fn propagate_signals(
    channel_slots: Vec<Arc<Mutex<Option<russh::ChannelWriteHalf<russh::client::Msg>>>>>,
) {
    tokio::spawn(async move {
        for slot in channel_slots {
            let mut chan_opt = {
                let mut guard = slot.lock().unwrap();
                guard.take()
            };
            if let Some(ref mut channel) = chan_opt {
                let _ = channel.signal(russh::Sig::INT).await;
                let _ = channel.signal(russh::Sig::TERM).await;
            }
        }
    });
}

#[allow(clippy::collapsible_if)]
pub async fn run_watch(target: &str, command: &str) -> Result<()> {
    crate::ssh_pool::SILENT_CONNECTION_LOGS.store(true, std::sync::atomic::Ordering::Relaxed);
    let config = crate::ssh_pool::load_config();
    let groups = &config.groups;
    let mut resolved_targets = Vec::new();
    if target.contains(',') {
        for t in target.split(',') {
            let t = t.trim();
            if !t.is_empty() {
                resolved_targets.push(t.to_string());
            }
        }
    } else {
        resolved_targets.push(target.to_string());
    }
    let final_hosts = crate::mcp_server::resolve_hosts(&resolved_targets, groups);
    if final_hosts.is_empty() {
        anyhow::bail!(
            "Configuration Error: No valid target hosts resolved from '{}'",
            target
        );
    }

    let mut hosts_status = Vec::new();
    for host in &final_hosts {
        hosts_status.push(HostStatus {
            name: host.clone(),
            status: "Running".to_string(),
            exit_code: None,
            log_path: std::path::PathBuf::new(),
            log_lines: Vec::new(),
        });
    }

    let state = Arc::new(Mutex::new(WatchState {
        hosts: hosts_status,
        selected_index: 0,
        is_teardown: false,
        first_ctrl_c: false,
    }));

    let pool = Arc::new(crate::ssh_pool::ConnectionPool::new(Duration::from_secs(
        300,
    )));

    let mut channel_slots = Vec::new();
    let mut join_handles = Vec::new();

    for (idx, host) in final_hosts.iter().enumerate() {
        let slot = Arc::new(Mutex::new(None));
        channel_slots.push(slot.clone());

        let host_clone = host.clone();
        let cmd_clone = command.to_string();
        let pool_clone = pool.clone();
        let state_clone = state.clone();

        let handle = tokio::spawn(async move {
            run_worker(host_clone, cmd_clone, pool_clone, state_clone, slot, idx).await;
        });
        join_handles.push(handle);
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let _guard = TerminalGuard;

    let mut interval = tokio::time::interval(Duration::from_millis(50));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                terminal.draw(|f| {
                    let s = state.lock().unwrap();
                    draw_ui(f, &s);
                })?;
            }
            res = tokio::task::spawn_blocking(|| event::poll(Duration::from_millis(10))) => {
                if let Ok(Ok(true)) = res {
                    if let Event::Key(key) = event::read()? {
                        if key.code == KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
                            let mut s = state.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
                            if s.is_teardown {
                                break;
                            } else {
                                s.is_teardown = true;
                                s.first_ctrl_c = true;
                                propagate_signals(channel_slots.clone());
                            }
                        } else if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') {
                            let mut s = state.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
                            if !s.is_teardown {
                                s.is_teardown = true;
                                propagate_signals(channel_slots.clone());
                            } else {
                                break;
                            }
                        } else if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                            let mut s = state.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
                            if s.selected_index + 1 < s.hosts.len() {
                                s.selected_index += 1;
                            }
                        } else if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                            let mut s = state.lock().map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
                            if s.selected_index > 0 {
                                s.selected_index -= 1;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_buffer() {
        let mut lb = LineBuffer::new();
        let mut lines = Vec::new();

        lb.push("hello\n", &mut lines, "");
        assert_eq!(lines, vec!["hello".to_string()]);

        lb.push("world\nthis is ", &mut lines, "[PREFIX] ");
        assert_eq!(
            lines,
            vec!["hello".to_string(), "[PREFIX] world".to_string()]
        );

        lb.push("incomplete", &mut lines, "[PREFIX] ");
        assert_eq!(
            lines,
            vec!["hello".to_string(), "[PREFIX] world".to_string()]
        );

        lb.flush_remaining(&mut lines, "[PREFIX] ");
        assert_eq!(
            lines,
            vec![
                "hello".to_string(),
                "[PREFIX] world".to_string(),
                "[PREFIX] this is incomplete".to_string()
            ]
        );
    }
}
