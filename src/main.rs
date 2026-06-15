use std::time::Duration;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod mcp_server;
mod ssh_config;
mod ssh_pool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "tui" {
        run_tui()?;
        return Ok(());
    }

    // We maintain a pool of open SSH connections, closing them after 5 minutes (300 seconds) of inactivity.
    let server = mcp_server::McpServer::new(Duration::from_secs(300));
    server.run().await?;
    Ok(())
}

fn run_tui() -> anyhow::Result<()> {
    println!("Starting agentic_ssh TUI Dashboard... Press Ctrl+C to exit.");
    let path = std::path::Path::new("/Users/richard/projects/rust/agentic_ssh/pool_status.json");

    loop {
        let mut daemon_active = false;
        if let Ok(metadata) = std::fs::metadata(path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed.as_secs() < 15 {
                        daemon_active = true;
                    }
                }
            }
        }

        let mut active_connections = Vec::new();
        let mut max_host_len = 30; // Default / minimum Host column width

        if daemon_active && path.exists() {
            let now_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if let Ok(file) = std::fs::File::open(path) {
                if let Ok(statuses) = serde_json::from_reader::<_, Vec<ssh_pool::ConnectionStatus>>(file) {
                    for status in statuses {
                        let elapsed_secs = now_unix.saturating_sub(status.last_used_timestamp);
                        let remaining_secs = status.idle_timeout_secs.saturating_sub(elapsed_secs);

                        if remaining_secs > 0 {
                            let last_used_str = format!("{}s ago", elapsed_secs);
                            let auto_close_str = format!("{}s left", remaining_secs);
                            max_host_len = max_host_len.max(status.host.len());
                            active_connections.push((status.host, last_used_str, auto_close_str));
                        }
                    }
                }
            }
        }

        // Position cursor at home (1,1) and clear everything below it
        print!("\x1B[H\x1B[J");

        let inner_width = max_host_len + 43; // max_host_len + 12 (Last Used) + 12 (Auto-Close) + 10 (Status) + 9 (separators/spaces)
        let border_top = format!("┌{}┐", "─".repeat(inner_width));
        let border_mid = format!("├{}┤", "─".repeat(inner_width));
        let border_bot = format!("└{}┘", "─".repeat(inner_width));

        println!("{}", border_top);
        println!("│{:^width$}│", "agentic_ssh Connection Pool", width = inner_width);
        println!("{}", border_mid);
        println!(
            "│ {:<width$} │ {:<12} │ {:<12} │ {:<10} │",
            "Host", "Last Used", "Auto-Close", "Status",
            width = max_host_len
        );
        println!("{}", border_mid);

        if !daemon_active {
            let msg = "[Daemon Inactive / Offline]";
            let padded = format!("{:^width$}", msg, width = inner_width);
            let colored = padded.replace(msg, "\x1B[31m[Daemon Inactive / Offline]\x1B[0m");
            println!("│{}│", colored);
        } else if active_connections.is_empty() {
            println!("│{:^width$}│", "No active connections in the pool", width = inner_width);
        } else {
            for (host, last_used_str, auto_close_str) in &active_connections {
                println!(
                    "│ {:<width$} │ {:<12} │ {:<12} │ \x1B[32m{:<10}\x1B[0m │",
                    host, last_used_str, auto_close_str, "Active",
                    width = max_host_len
                );
            }
        }

        println!("{}", border_bot);
        if daemon_active {
            println!("Active connections: {}", active_connections.len());
        } else {
            println!("Active connections: 0 (Daemon offline)");
        }
        println!("(Auto-refreshing every 1 second)");

        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::thread::sleep(Duration::from_secs(1));
    }
}
