// Rust guideline compliant 2025-10-17
//! Doctor command: comprehensive health check of the agentic_ssh installation.
//!
//! Checks the binary, SSH client, SSH config, pool daemon status, and agent integrations.

use crate::agents::{self, DoctorCounters, HealthcheckContext};
use crate::ssh_config;
use crate::ssh_pool;
use std::path::PathBuf;

/// Runs a comprehensive health check of the agentic_ssh installation.
pub async fn run_doctor(agent_filter: Option<&str>) {
    let mut dc = DoctorCounters::new();

    eprintln!(
        "\n\x1b[1magentic_ssh doctor v{}\x1b[0m\n",
        env!("CARGO_PKG_VERSION")
    );

    check_binary(&mut dc);
    check_ssh_environment(&mut dc);
    check_daemon_status(&mut dc);

    // Agent-specific health checks
    if let Some(ref home) = agents::home_dir() {
        let project_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let hctx = HealthcheckContext {
            home: home.clone(),
            project_path,
        };
        let agents_to_check: Vec<Box<dyn agents::AgentIntegration>> = match agent_filter {
            Some(id) => match agents::get_integration(id) {
                Ok(ag) => vec![ag],
                Err(e) => {
                    dc.fail(&format!("{e}"));
                    vec![]
                }
            },
            None => agents::all_integrations(),
        };
        for ag in &agents_to_check {
            ag.healthcheck(&mut dc, &hctx);
        }
    } else {
        dc.fail("Could not determine home directory");
    }

    print_summary(&dc);
}

fn check_binary(dc: &mut DoctorCounters) {
    eprintln!("\x1b[1mBinary\x1b[0m");
    if let Ok(exe) = std::env::current_exe() {
        dc.pass(&format!("Binary: {}", exe.display()));
    } else {
        dc.fail("Could not determine binary path");
    }
    dc.pass(&format!("Version: {}", env!("CARGO_PKG_VERSION")));
}

fn check_ssh_environment(dc: &mut DoctorCounters) {
    eprintln!("\n\x1b[1mSSH Environment\x1b[0m");

    // Check if ssh command exists and runs
    match std::process::Command::new("ssh").arg("-V").output() {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let version_info = stderr.trim();
            if !version_info.is_empty() {
                dc.pass(&format!("System SSH client detected: {}", version_info));
            } else {
                dc.pass("System SSH client detected");
            }
        }
        Err(_) => {
            dc.fail("System SSH client ('ssh' binary) not found in PATH");
        }
    }

    // Check if ~/.ssh/config exists
    if let Some(home) = agents::home_dir() {
        let ssh_config_path = home.join(".ssh").join("config");
        if ssh_config_path.exists() {
            dc.pass(&format!("SSH config found: {}", ssh_config_path.display()));

            // Check hosts parsed
            match ssh_config::list_ssh_hosts() {
                Ok(hosts) => {
                    dc.pass(&format!("Parsed {} hosts from SSH config", hosts.len()));
                    if hosts.is_empty() {
                        dc.info("No hosts found. You can define them in ~/.ssh/config.");
                    } else {
                        // Print first 5 hosts as info
                        let limit = hosts.len().min(5);
                        dc.info(&format!(
                            "First {limit} hosts: {}",
                            hosts[..limit].join(", ")
                        ));
                        if hosts.len() > limit {
                            dc.info(&format!("... and {} more", hosts.len() - limit));
                        }
                    }
                }
                Err(e) => {
                    dc.warn(&format!("Failed to parse SSH config: {e}"));
                }
            }
        } else {
            dc.warn("~/.ssh/config does not exist. AI agents may not know which SSH hosts are available.");
        }
    } else {
        dc.warn("Could not determine home directory to check ~/.ssh/config");
    }
}

fn check_daemon_status(dc: &mut DoctorCounters) {
    eprintln!("\n\x1b[1mConnection Pool Daemon\x1b[0m");
    let path_buf = ssh_pool::get_pool_status_path();
    let path = path_buf.as_path();

    let daemon_active = ssh_pool::is_daemon_active(path);

    if daemon_active {
        dc.pass("agentic_ssh daemon/MCP server is currently running");
        if let Some(statuses) = ssh_pool::load_connection_statuses(path) {
            let active_count = statuses.len();
            dc.pass(&format!(
                "Found {active_count} connection(s) in active pool status"
            ));
            for status in statuses {
                dc.info(&format!(
                    "• Host: {}, Idle Timeout: {}s",
                    status.host, status.idle_timeout_secs
                ));
            }
        }
    } else {
        dc.warn("No active daemon detected. Run `agentic_ssh serve` or use an integrated AI agent to start the MCP server.");
    }
}

fn print_summary(dc: &DoctorCounters) {
    eprintln!();
    if dc.issues == 0 && dc.warnings == 0 {
        eprintln!("\x1b[32mAll checks passed.\x1b[0m");
    } else if dc.issues == 0 {
        eprintln!("\x1b[33m{} warning(s), no issues.\x1b[0m", dc.warnings);
    } else {
        eprintln!(
            "\x1b[31m{} issue(s), {} warning(s).\x1b[0m",
            dc.issues, dc.warnings
        );
        eprintln!("Run \x1b[1magentic_ssh install\x1b[0m to configure agent integrations.");
    }
    eprintln!();
}
