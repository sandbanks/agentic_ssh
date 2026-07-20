use mimalloc::MiMalloc;
use std::{path::PathBuf, time::Duration};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod agents;
mod doctor;
mod errors;
mod mcp_server;
mod security;
mod ssh_config;
mod ssh_pool;
mod watch;

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
    /// Explicit override path to a configuration file
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Ignore the global configuration baseline entirely
    #[arg(long)]
    no_global: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Cryptographically sign and trust a project-local configuration file (Human-only)
    Trust {
        /// Path to the local .agentic_ssh.toml file to trust
        #[arg(default_value = "./.agentic_ssh.toml")]
        path: PathBuf,
    },
    /// Start the live TUI connection pool dashboard
    Tui,
    /// Watch a command executing on one or more hosts in real-time
    Watch {
        /// Target host alias, comma-separated list, or group name
        target: String,
        /// Command to execute on the remote host(s)
        command: String,
    },
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
    /// Run diagnostic checks on the system and agent configurations
    Doctor {
        /// Agent to check (auto-detects if omitted)
        #[arg(long, value_enum)]
        agent: Option<AgentOption>,
    },
    /// Call an MCP tool directly via CLI and print its JSON response to stdout
    Json {
        /// Name of the tool to execute
        tool: String,

        /// Optional JSON arguments payload or comma-separated host list
        arguments: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Store CLI overrides globally
    let _ = ssh_pool::CLI_OVERRIDE.set(ssh_pool::CliOverride {
        config_path: cli.config.clone(),
        no_global: cli.no_global,
    });

    // Run configuration loading and validation check immediately upon startup
    let _ = ssh_pool::load_config();

    // GUARDRAIL 1: Protect explicit config override paths and trust commands from agent injection
    if cli.config.is_some() || cli.no_global || matches!(cli.command, Some(Commands::Trust { .. }))
    {
        // Force the /dev/tty human check before parsing an explicit configuration string
        if let Err(e) = security::enforce_human_interaction() {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }

    let is_server_init = matches!(cli.command, Some(Commands::Serve) | None);
    if is_server_init {
        let _ = auto_install_if_needed();
    }

    match cli.command {
        Some(Commands::Trust { path }) => {
            security::trust_local_config(&path)?;
            println!("🔒 Local file successfully trusted and registered in your global ledger.");
        }

        Some(Commands::Tui) => {
            run_tui()?;
        }
        Some(Commands::Watch { target, command }) => {
            watch::run_watch(&target, &command).await?;
        }
        Some(Commands::Install { agent, local }) => {
            let home = agents::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine user's home directory"))?;
            let agentic_ssh_bin = agents::which_agentic_ssh().ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not locate the `agentic_ssh` binary in PATH or current directory"
                )
            })?;
            let tool_permissions = agents::expected_tool_perms();

            let scope = if local {
                let project_path = std::env::current_dir()?;
                agents::InstallScope::Local { project_path }
            } else {
                agents::InstallScope::Global
            };

            let ctx = agents::InstallContext {
                home,
                agentic_ssh_bin,
                tool_permissions,
                scope,
            };

            if let Some(opt) = agent {
                let id = opt.to_str();
                let integration =
                    agents::get_integration(id).map_err(|e| anyhow::anyhow!("{}", e))?;
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
                let all = agents::all_integrations();
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
            let home = agents::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not determine user's home directory"))?;
            let agentic_ssh_bin =
                agents::which_agentic_ssh().unwrap_or_else(|| "agentic_ssh".to_string());
            let tool_permissions = agents::expected_tool_perms();

            let scope = if local {
                let project_path = std::env::current_dir()?;
                agents::InstallScope::Local { project_path }
            } else {
                agents::InstallScope::Global
            };

            let ctx = agents::InstallContext {
                home,
                agentic_ssh_bin,
                tool_permissions,
                scope,
            };

            if let Some(opt) = agent {
                let id = opt.to_str();
                let integration =
                    agents::get_integration(id).map_err(|e| anyhow::anyhow!("{}", e))?;
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
                let all = agents::all_integrations();
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
        Some(Commands::Doctor { agent }) => {
            let agent_str = agent.map(|a| a.to_str());
            doctor::run_doctor(agent_str).await;
        }
        Some(Commands::Json { tool, arguments }) => {
            let args_val = if let Some(ref args_str) = arguments {
                let trimmed = args_str.trim();
                if trimmed.starts_with('{') {
                    serde_json::from_str(trimmed)
                        .map_err(|e| anyhow::anyhow!("Invalid JSON arguments: {}", e))?
                } else {
                    let hosts: Vec<String> = trimmed
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    serde_json::json!({ "hosts": hosts })
                }
            } else {
                serde_json::json!({})
            };

            let server = mcp_server::McpServer::new(Duration::from_secs(300));
            let params = serde_json::json!({
                "name": tool,
                "arguments": args_val
            });
            match server.handle_tools_call(Some(params)).await {
                Ok(res) => {
                    let text = serde_json::to_string_pretty(&res)?;
                    println!("{}", text);
                }
                Err(e) => {
                    eprintln!("Error executing tool: {}", e);
                    std::process::exit(1);
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
        let daemon_active = ssh_pool::is_daemon_active(path);

        let mut active_connections = Vec::new();
        let mut max_host_len = 30; // Default / minimum Host column width

        if daemon_active && path.exists() {
            let now_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            if let Some(statuses) = ssh_pool::load_connection_statuses(path) {
                for status in statuses {
                    let elapsed_secs = now_unix.saturating_sub(status.last_used_timestamp);
                    let remaining_secs = status.idle_timeout_secs.saturating_sub(elapsed_secs);
                    let is_executing = status.status == "Executing";

                    if remaining_secs > 0 || is_executing {
                        let last_used_str = if is_executing {
                            "now".to_string()
                        } else {
                            format!("{}s ago", elapsed_secs)
                        };
                        let auto_close_str = if is_executing {
                            "pinned".to_string()
                        } else {
                            format!("{}s left", remaining_secs)
                        };
                        let status_str = if status.status.is_empty() {
                            "Active".to_string()
                        } else {
                            status.status.clone()
                        };
                        max_host_len = max_host_len.max(status.host.len());
                        active_connections.push((
                            status.host,
                            last_used_str,
                            auto_close_str,
                            status_str,
                        ));
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
            for (host, last_used_str, auto_close_str, status_str) in &active_connections {
                let status_color = if status_str == "Executing" {
                    "\x1B[33m" // Yellow for executing
                } else {
                    "\x1B[32m" // Green for active
                };
                println!(
                    "│ {:<width$} │ {:<12} │ {:<12} │ {}{:<10}\x1B[0m │",
                    host,
                    last_used_str,
                    auto_close_str,
                    status_color,
                    status_str,
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

fn auto_install_if_needed() -> anyhow::Result<()> {
    let home = agents::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine user's home directory"))?;
    let agentic_ssh_bin = match agents::which_agentic_ssh() {
        Some(bin) => bin,
        None => return Ok(()),
    };
    let tool_permissions = agents::expected_tool_perms();
    let ctx = agents::InstallContext {
        home: home.clone(),
        agentic_ssh_bin,
        tool_permissions,
        scope: agents::InstallScope::Global,
    };

    let all = agents::all_integrations();
    for integration in all {
        if integration.is_detected(&ctx.home) && !integration.has_agentic_ssh(&ctx.home) {
            eprintln!(
                "Auto-configuring agentic_ssh MCP server for {}...",
                integration.name()
            );
            if let Err(e) = integration.install(&ctx) {
                eprintln!("Failed to auto-configure {}: {}", integration.name(), e);
            }
        }
    }
    Ok(())
}
