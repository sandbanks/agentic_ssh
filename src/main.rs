use mimalloc::MiMalloc;
use std::time::Duration;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod agents;
mod errors;
mod mcp_server;
mod ssh_config;
mod ssh_pool;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
enum AgentOption {
    Claude,
    #[value(name = "opencode")]
    Opencode,
    Codex,
    Gemini,
    Copilot,
    Cursor,
    Zed,
    Cline,
    RooCode,
    Antigravity,
    Kilo,
    Kiro,
    Kimi,
    Vibe,
    Grok,
    Pi,
}

impl AgentOption {
    fn to_str(self) -> &'static str {
        match self {
            AgentOption::Claude => "claude",
            AgentOption::Opencode => "opencode",
            AgentOption::Codex => "codex",
            AgentOption::Gemini => "gemini",
            AgentOption::Copilot => "copilot",
            AgentOption::Cursor => "cursor",
            AgentOption::Zed => "zed",
            AgentOption::Cline => "cline",
            AgentOption::RooCode => "roo-code",
            AgentOption::Antigravity => "antigravity",
            AgentOption::Kilo => "kilo",
            AgentOption::Kiro => "kiro",
            AgentOption::Kimi => "kimi",
            AgentOption::Vibe => "vibe",
            AgentOption::Grok => "grok",
            AgentOption::Pi => "pi",
        }
    }
}

#[derive(Parser)]
#[command(name = "agentic_ssh")]
#[command(version)]
#[command(about = "agentic_ssh - SSH connection pooling & MCP server for AI agents", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the live TUI connection pool dashboard
    Tui,
    /// Start the MCP server (default)
    Serve,
    /// Install the MCP server configuration for AI agents
    Install {
        /// Agent to configure (auto-detects if omitted)
        #[arg(long, value_enum)]
        agent: Option<AgentOption>,

        /// Install the MCP server only in the local project folder
        #[arg(long)]
        local: bool,
    },
    /// Uninstall the MCP server configuration for AI agents
    Uninstall {
        /// Agent to configure (auto-detects if omitted)
        #[arg(long, value_enum)]
        agent: Option<AgentOption>,

        /// Uninstall the MCP server only in the local project folder
        #[arg(long)]
        local: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Tui) => {
            run_tui()?;
        }
        Some(Commands::Install { agent, local }) => {
            let home = crate::agents::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine user's home directory"))?;
            let agentic_ssh_bin = crate::agents::which_agentic_ssh().ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not locate the `agentic_ssh` binary in PATH or current directory"
                )
            })?;
            let tool_permissions = crate::agents::expected_tool_perms();

            let scope = if local {
                let project_path = std::env::current_dir()?;
                crate::agents::InstallScope::Local { project_path }
            } else {
                crate::agents::InstallScope::Global
            };

            let ctx = crate::agents::InstallContext {
                home,
                agentic_ssh_bin,
                tool_permissions,
                scope,
            };

            if let Some(opt) = agent {
                let id = opt.to_str();
                let integration =
                    crate::agents::get_integration(id).map_err(|e| anyhow::anyhow!("{}", e))?;
                if local && !integration.supports_local() {
                    anyhow::bail!(
                        "Agent '{}' does not support project-scoped (--local) installation.",
                        id
                    );
                }
                eprintln!(
                    "Installing agentic_ssh MCP server for {}...",
                    integration.name()
                );
                integration
                    .install(&ctx)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                eprintln!(
                    "Successfully configured agentic_ssh for {}!\n",
                    integration.name()
                );
            } else {
                // Auto-detect
                let all = crate::agents::all_integrations();
                let mut detected = Vec::new();
                for integration in all {
                    if integration.is_detected(&ctx.home)
                        && (!local || integration.supports_local())
                    {
                        detected.push(integration);
                    }
                }

                if detected.is_empty() {
                    anyhow::bail!(
                        "No supported AI agents were auto-detected on this system. Please specify which agent to configure using '--agent <AGENT>'."
                    );
                }

                eprintln!(
                    "Auto-detected agents: {}\n",
                    detected
                        .iter()
                        .map(|a| a.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for integration in detected {
                    eprintln!(
                        "Installing agentic_ssh MCP server for {}...",
                        integration.name()
                    );
                    integration
                        .install(&ctx)
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                    eprintln!(
                        "Successfully configured agentic_ssh for {}!\n",
                        integration.name()
                    );
                }
            }
        }
        Some(Commands::Uninstall { agent, local }) => {
            let home = crate::agents::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine user's home directory"))?;
            let agentic_ssh_bin =
                crate::agents::which_agentic_ssh().unwrap_or_else(|| "agentic_ssh".to_string());
            let tool_permissions = crate::agents::expected_tool_perms();

            let scope = if local {
                let project_path = std::env::current_dir()?;
                crate::agents::InstallScope::Local { project_path }
            } else {
                crate::agents::InstallScope::Global
            };

            let ctx = crate::agents::InstallContext {
                home,
                agentic_ssh_bin,
                tool_permissions,
                scope,
            };

            if let Some(opt) = agent {
                let id = opt.to_str();
                let integration =
                    crate::agents::get_integration(id).map_err(|e| anyhow::anyhow!("{}", e))?;
                if local && !integration.supports_local() {
                    anyhow::bail!(
                        "Agent '{}' does not support project-scoped (--local) installation.",
                        id
                    );
                }
                eprintln!(
                    "Uninstalling agentic_ssh MCP server for {}...",
                    integration.name()
                );
                integration
                    .uninstall(&ctx)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                eprintln!(
                    "Successfully uninstalled agentic_ssh from {}!",
                    integration.name()
                );
            } else {
                // Auto-detect
                let all = crate::agents::all_integrations();
                let mut detected = Vec::new();
                for integration in all {
                    if integration.is_detected(&ctx.home)
                        && (!local || integration.supports_local())
                    {
                        detected.push(integration);
                    }
                }

                if detected.is_empty() {
                    anyhow::bail!(
                        "No supported AI agents were auto-detected on this system. Please specify which agent to uninstall using '--agent <AGENT>'."
                    );
                }

                eprintln!(
                    "Auto-detected agents: {}",
                    detected
                        .iter()
                        .map(|a| a.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for integration in detected {
                    eprintln!(
                        "Uninstalling agentic_ssh MCP server from {}...",
                        integration.name()
                    );
                    integration
                        .uninstall(&ctx)
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                    eprintln!(
                        "Successfully uninstalled agentic_ssh from {}!",
                        integration.name()
                    );
                }
            }
        }
        Some(Commands::Serve) | None => {
            // We maintain a pool of open SSH connections, closing them after 5 minutes (300 seconds) of inactivity.
            let server = mcp_server::McpServer::new(Duration::from_secs(300));
            server.run().await?;
        }
    }
    Ok(())
}

fn run_tui() -> anyhow::Result<()> {
    println!("Starting agentic_ssh TUI Dashboard... Press Ctrl+C to exit.");
    let path_buf = ssh_pool::get_pool_status_path();
    let path = path_buf.as_path();

    loop {
        let daemon_active = std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|e| e.as_secs() < 15)
            .unwrap_or(false);

        let mut active_connections = Vec::new();
        let mut max_host_len = 30; // Default / minimum Host column width

        if daemon_active && path.exists() {
            let now_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if let Some(statuses) = std::fs::File::open(path).ok().and_then(|file| {
                serde_json::from_reader::<_, Vec<ssh_pool::ConnectionStatus>>(file).ok()
            }) {
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

        // Position cursor at home (1,1) and clear everything below it
        print!("\x1B[H\x1B[J");

        let inner_width = max_host_len + 45; // max_host_len + 12 (Last Used) + 12 (Auto-Close) + 10 (Status) + 11 (separators/spaces)
        let border_top = format!("┌{}┐", "─".repeat(inner_width));
        let border_mid = format!("├{}┤", "─".repeat(inner_width));
        let border_bot = format!("└{}┘", "─".repeat(inner_width));

        println!("{}", border_top);
        println!(
            "│{:^width$}│",
            "agentic_ssh Connection Pool",
            width = inner_width
        );
        println!("{}", border_mid);
        println!(
            "│ {:<width$} │ {:<12} │ {:<12} │ {:<10} │",
            "Host",
            "Last Used",
            "Auto-Close",
            "Status",
            width = max_host_len
        );
        println!("{}", border_mid);

        if !daemon_active {
            let msg = "[Daemon Inactive / Offline]";
            let padded = format!("{:^width$}", msg, width = inner_width);
            let colored = padded.replace(msg, "\x1B[31m[Daemon Inactive / Offline]\x1B[0m");
            println!("│{}│", colored);
        } else if active_connections.is_empty() {
            println!(
                "│{:^width$}│",
                "No active connections in the pool",
                width = inner_width
            );
        } else {
            for (host, last_used_str, auto_close_str) in &active_connections {
                println!(
                    "│ {:<width$} │ {:<12} │ {:<12} │ \x1B[32m{:<10}\x1B[0m │",
                    host,
                    last_used_str,
                    auto_close_str,
                    "Active",
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
