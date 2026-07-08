use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

use super::parsers::{parse_listening_ports, parse_system_stats};
use super::{abbreviate_output, find_matched_line, make_temp_log_path};
use crate::ssh_pool::ConnectionPool;

pub async fn run_get_system_stats(
    pool: &Arc<ConnectionPool>,
    host: &str,
) -> Result<serde_json::Value> {
    let cmd = "echo '=== LOAD ===' && (cat /proc/loadavg 2>/dev/null || uptime) && echo '=== MEM ===' && (cat /proc/meminfo 2>/dev/null || free -k 2>/dev/null) && echo '=== DISK ===' && df -kP /";
    let log_path = make_temp_log_path(host);
    let (stdout, stderr, exit_code) = pool
        .execute_command(host, cmd, true, 5, log_path.clone())
        .await?;
    let _ = std::fs::remove_file(&log_path);
    if exit_code != 0 {
        anyhow::bail!(
            "Error executing stats command (exit code {}):\n{}",
            exit_code,
            stderr
        );
    }
    let stats = parse_system_stats(&stdout);
    Ok(serde_json::to_value(&stats)?)
}

pub async fn run_list_ports(
    pool: &Arc<ConnectionPool>,
    host: &str,
    filter_port: Option<u32>,
) -> Result<serde_json::Value> {
    // Read listening ports (ss -tulpn)
    let cmd = "ss -tulpn 2>/dev/null || netstat -tulpn 2>/dev/null || cat /proc/net/tcp";
    let log_path = make_temp_log_path(host);
    let (stdout, stderr, exit_code) = pool
        .execute_command(host, cmd, true, 5, log_path.clone())
        .await?;
    let _ = std::fs::remove_file(&log_path);
    if exit_code != 0 {
        anyhow::bail!(
            "Error executing ports command (exit code {}):\n{}",
            exit_code,
            stderr
        );
    }
    let ports = parse_listening_ports(&stdout, filter_port);
    Ok(serde_json::to_value(&ports)?)
}

#[allow(clippy::too_many_arguments)]
pub async fn run_run_command(
    pool: &Arc<ConnectionPool>,
    host: &str,
    command: &str,
    quiet: bool,
    progress_interval_secs: u64,
    background: bool,
    abbreviate: bool,
    max_lines: usize,
) -> Result<serde_json::Value> {
    let sessions_dir = home::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".agentic_ssh")
        .join("sessions");
    std::fs::create_dir_all(&sessions_dir)
        .with_context(|| format!("Failed to create sessions directory at {:?}", sessions_dir))?;

    let now_str = chrono::Local::now().format("%Y%m%d_%H%M").to_string();
    let nano_rand = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand_hex = format!("{:04x}", nano_rand & 0xffff);
    let log_file_name = format!("job_{}_{}_{}.log", now_str, host, rand_hex);
    let log_path = sessions_dir.join(&log_file_name);

    if background {
        let pool_clone = pool.clone();
        let host_clone = host.to_string();
        let command_clone = command.to_string();
        let log_path_clone = log_path.clone();
        tokio::spawn(async move {
            let _ = pool_clone
                .execute_command(
                    &host_clone,
                    &command_clone,
                    quiet,
                    progress_interval_secs,
                    log_path_clone,
                )
                .await;
        });

        return Ok(serde_json::json!({
            "status": "started",
            "log_path": log_path.to_string_lossy().to_string(),
            "message": format!(
                "🚀 Command started in background. To watch live progress, run:\n  tail -f {}",
                log_path.to_string_lossy()
            )
        }));
    }

    let (stdout, stderr, exit_code) = pool
        .execute_command(host, command, quiet, progress_interval_secs, log_path)
        .await?;
    let stdout_final = if abbreviate {
        abbreviate_output(&stdout, max_lines)
    } else {
        stdout
    };
    Ok(serde_json::json!({
        "stdout": stdout_final,
        "stderr": stderr,
        "exit_code": exit_code
    }))
}

pub async fn run_search_processes(
    pool: &Arc<ConnectionPool>,
    host: &str,
    re: regex::Regex,
    full_info: bool,
) -> Result<serde_json::Value> {
    // POSIX-standard process listing
    let log_path = make_temp_log_path(host);
    let (stdout, stderr, exit_code) = pool
        .execute_command(
            host,
            "ps -eo pid,user,%cpu,%mem,args",
            true,
            5,
            log_path.clone(),
        )
        .await?;
    let _ = std::fs::remove_file(&log_path);
    if exit_code != 0 {
        anyhow::bail!(
            "Error running ps command (exit code {}):\n{}",
            exit_code,
            stderr
        );
    }

    let mut results = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Split by whitespace
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }

        // If the first column doesn't parse as PID, it's the header row or invalid
        let pid = match parts[0].parse::<u32>() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let user = parts[1];
        let cpu = parts[2];
        let mem = parts[3];
        let command = parts[4..].join(" ");

        // Filter using the regex on the command line
        if re.is_match(&command) {
            if full_info {
                results.push(serde_json::json!({
                    "pid": pid,
                    "user": user,
                    "cpu": cpu,
                    "mem": mem,
                    "command": command
                }));
            } else {
                results.push(serde_json::json!({
                    "pid": pid,
                    "command": command
                }));
            }
        }
    }
    Ok(serde_json::Value::Array(results))
}

pub async fn run_tail_log(
    pool: &Arc<ConnectionPool>,
    host: &str,
    file_path: &str,
    lines: usize,
) -> Result<serde_json::Value> {
    let command = format!(
        "tail -n {} {}",
        lines,
        crate::ssh_pool::shell_escape(file_path)
    );
    let log_path = make_temp_log_path(host);
    let (stdout, stderr, exit_code) = pool
        .execute_command(host, &command, true, 5, log_path.clone())
        .await?;
    let _ = std::fs::remove_file(&log_path);
    if exit_code != 0 {
        anyhow::bail!("Error tailing file (exit code {}):\n{}", exit_code, stderr);
    }
    Ok(serde_json::json!(stdout))
}

pub async fn run_tail_container_logs(
    pool: &Arc<ConnectionPool>,
    host: &str,
    container: &str,
    lines: usize,
    timestamps: bool,
) -> Result<serde_json::Value> {
    let ts_flag = if timestamps { "--timestamps" } else { "" };
    let command = format!(
        "docker logs --tail {} {} {}",
        lines,
        ts_flag,
        crate::ssh_pool::shell_escape(container)
    );
    let log_path = make_temp_log_path(host);
    let (stdout, stderr, exit_code) = pool
        .execute_command(host, &command, true, 5, log_path.clone())
        .await?;
    let _ = std::fs::remove_file(&log_path);
    let text = if exit_code != 0 { stderr } else { stdout };
    Ok(serde_json::json!(text))
}

pub async fn run_wait_for_log_pattern(
    pool: &Arc<ConnectionPool>,
    host: &str,
    file_path: Option<String>,
    container: Option<String>,
    re: regex::Regex,
    pattern: &str,
    timeout_secs: u64,
) -> Result<serde_json::Value> {
    let cmd = if let Some(path) = &file_path {
        format!("tail -f -n 10 {}", crate::ssh_pool::shell_escape(path))
    } else {
        format!(
            "docker logs -f --tail 10 {}",
            crate::ssh_pool::shell_escape(container.as_ref().unwrap())
        )
    };

    let handle = pool.get_connection(host).await?;
    let _guard = pool.start_operation(host).await;
    let mut channel = handle
        .channel_open_session()
        .await
        .context("Failed to open SSH channel")?;

    channel
        .exec(true, cmd)
        .await
        .context("Failed to execute tail/log command")?;

    let mut stdout_buf = Vec::new();
    let mut matched_line = None;
    let mut error_msg = None;

    let sleep_duration = Duration::from_millis(50);
    let start_time = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    loop {
        if start_time.elapsed() >= timeout {
            error_msg = Some(format!(
                "Timed out after {} seconds waiting for pattern '{}'",
                timeout_secs, pattern
            ));
            break;
        }

        match tokio::time::timeout(sleep_duration, channel.wait()).await {
            Ok(Some(russh::ChannelMsg::Data { data })) => {
                stdout_buf.extend_from_slice(&data);
                if let Some(matched) = find_matched_line(&mut stdout_buf, &re) {
                    matched_line = Some(matched);
                    break;
                }
            }
            Ok(Some(russh::ChannelMsg::ExtendedData { data, ext })) => {
                if ext == 1 {
                    stdout_buf.extend_from_slice(&data);
                    if let Some(matched) = find_matched_line(&mut stdout_buf, &re) {
                        matched_line = Some(matched);
                        break;
                    }
                }
            }
            Ok(Some(russh::ChannelMsg::ExitStatus { exit_status })) => {
                if exit_status != 0 {
                    error_msg = Some(format!("Command exited with status {}", exit_status));
                }
                break;
            }
            Ok(None) => {
                break;
            }
            Err(_) => {
                // Timeout elapsed, loop again to check total timeout
            }
            _ => {}
        }
    }

    if matched_line.is_none() && error_msg.is_none() && !stdout_buf.is_empty() {
        let line_str = String::from_utf8_lossy(&stdout_buf).into_owned();
        if re.is_match(&line_str) {
            matched_line = Some(line_str);
        }
    }

    let _ = channel.close().await;

    if let Some(line) = matched_line {
        Ok(serde_json::json!(line))
    } else {
        let err =
            error_msg.unwrap_or_else(|| "Connection closed before pattern was matched".to_string());
        anyhow::bail!("{}", err);
    }
}
