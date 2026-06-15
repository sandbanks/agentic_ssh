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
        // Clear screen and move cursor to top-left
        print!("\x1B[2J\x1B[1;1H");

        println!("┌──────────────────────────────────────────────────────────┐");
        println!("│             agentic_ssh Connection Pool                  │");
        println!("├──────────────────────────────────────────────────────────┤");
        println!("│ {:<15} │ {:<12} │ {:<12} │ {:<10} │", "Host", "Last Used", "Auto-Close", "Status");
        println!("├──────────────────────────────────────────────────────────┤");

        let mut active_count = 0;
        if path.exists() {
            if let Ok(file) = std::fs::File::open(path) {
                if let Ok(statuses) = serde_json::from_reader::<_, Vec<ssh_pool::ConnectionStatus>>(file) {
                    active_count = statuses.len();
                    for status in statuses {
                        let last_used_str = format!("{}s ago", status.elapsed_secs);
                        let auto_close_str = format!("{}s left", status.remaining_secs);
                        // Print using green color for active status
                        println!(
                            "│ {:<15} │ {:<12} │ {:<12} │ \x1B[32m{:<10}\x1B[0m │",
                            status.host, last_used_str, auto_close_str, "Active"
                        );
                    }
                }
            }
        }

        if active_count == 0 {
            println!("│ {:^54} │", "No active connections in the pool");
        }

        println!("└──────────────────────────────────────────────────────────┘");
        println!("Active connections: {}", active_count);
        println!("(Auto-refreshing every 1 second)");

        std::thread::sleep(Duration::from_secs(1));
    }
}
