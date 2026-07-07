//! Google Antigravity (formerly Windsurf) agent integration.
//!
//! Handles registration of the agentic_ssh MCP server in:
//!
//! - `~/.gemini/config/mcp_config.json` — the central Antigravity config,
//!   shape `{"mcpServers": {"agentic_ssh": {...}}}`.
//! - `~/.gemini/antigravity-cli/plugins/agentic_ssh.json` — the Antigravity
//!   CLI (`agy`) plugin file, same shape. Required because the IDE config
//!   is not picked up by the CLI (#85).
//!
//! Both files are kept in sync by `install` and `uninstall`; `doctor` checks
//! both and reports each location separately.

use std::path::Path;

use serde_json::json;

use crate::errors::Result;

use super::{
    AgentIntegration, DoctorCounters, HealthcheckContext, InstallContext, backup_config_file,
    load_json_file, load_json_file_strict, safe_write_json_file,
};

/// Google Antigravity agent.
pub struct AntigravityIntegration;

fn mcp_config_path(home: &Path) -> std::path::PathBuf {
    home.join(".gemini/config/mcp_config.json")
}

/// Per-plugin file used by the Antigravity CLI. Holds the same shape as
/// the IDE config so a future shared loader can read either location.
fn cli_plugin_path(home: &Path) -> std::path::PathBuf {
    home.join(".gemini/antigravity-cli/plugins/agentic_ssh.json")
}

impl AgentIntegration for AntigravityIntegration {
    fn name(&self) -> &'static str {
        "Antigravity"
    }

    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn install(&self, ctx: &InstallContext) -> Result<()> {
        // 1. Antigravity central config (~/.gemini/config/mcp_config.json)
        let mcp_path = mcp_config_path(&ctx.home);
        if let Some(parent) = mcp_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let backup = backup_config_file(&mcp_path)?;
        let mut settings = match load_json_file_strict(&mcp_path) {
            Ok(v) => v,
            Err(e) => {
                if let Some(ref b) = backup {
                    eprintln!("  Backup preserved at: {}", b.display());
                }
                return Err(e);
            }
        };
        settings["mcpServers"]["agentic_ssh"] = json!({
            "command": ctx.agentic_ssh_bin,
            "args": ["serve"]
        });
        safe_write_json_file(&mcp_path, &settings, backup.as_deref())?;
        eprintln!(
            "\x1b[32m✔\x1b[0m Added agentic_ssh MCP server to {}",
            mcp_path.display()
        );

        // 2. Antigravity CLI plugin (~/.gemini/antigravity-cli/plugins/agentic_ssh.json).
        //    Same shape as the IDE config; required because the IDE config is
        //    not picked up by the CLI (#85).
        let plugin_path = cli_plugin_path(&ctx.home);
        if let Some(parent) = plugin_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let plugin_backup = backup_config_file(&plugin_path)?;
        let plugin_settings = json!({
            "mcpServers": {
                "agentic_ssh": {
                    "command": ctx.agentic_ssh_bin,
                    "args": ["serve"],
                }
            }
        });
        safe_write_json_file(&plugin_path, &plugin_settings, plugin_backup.as_deref())?;
        eprintln!(
            "\x1b[32m✔\x1b[0m Added agentic_ssh CLI plugin to {}",
            plugin_path.display()
        );

        eprintln!();
        eprintln!("Setup complete. Next steps:");
        eprintln!(
            "  2. Restart Antigravity (IDE or `agy` CLI) — agentic_ssh tools are now available"
        );
        Ok(())
    }

    fn uninstall(&self, ctx: &InstallContext) -> Result<()> {
        let mcp_path = mcp_config_path(&ctx.home);
        uninstall_mcp_server(&mcp_path);
        uninstall_cli_plugin(&cli_plugin_path(&ctx.home));

        eprintln!();
        eprintln!("Uninstall complete. AgenticSsh has been removed from Antigravity.");
        eprintln!("Restart Antigravity (IDE or `agy` CLI) for changes to take effect.");
        Ok(())
    }

    fn healthcheck(&self, dc: &mut DoctorCounters, ctx: &HealthcheckContext) {
        eprintln!("\n\x1b[1mAntigravity integration\x1b[0m");
        doctor_check_settings(dc, &ctx.home);
        doctor_check_cli_plugin(dc, &ctx.home);
    }

    fn is_detected(&self, home: &Path) -> bool {
        home.join(".gemini/antigravity").is_dir() || home.join(".gemini/antigravity-cli").is_dir()
    }

    fn has_agentic_ssh(&self, home: &Path) -> bool {
        let current_bin = super::which_agentic_ssh();
        let check_file = |path: &Path| -> bool {
            if !path.exists() {
                return false;
            }
            let json = load_json_file(path);
            if let Some(agentic_ssh) = json.get("mcpServers").and_then(|v| v.get("agentic_ssh")) {
                if let Some(ref current) = current_bin {
                    let cmd = agentic_ssh
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    cmd == *current
                } else {
                    true
                }
            } else {
                false
            }
        };

        let mut ok = true;
        let mut any_checked = false;
        if home.join(".gemini/config").is_dir() || home.join(".gemini/antigravity").is_dir() {
            ok = ok && check_file(&mcp_config_path(home));
            any_checked = true;
        }
        if home.join(".gemini/antigravity-cli").is_dir() {
            ok = ok && check_file(&cli_plugin_path(home));
            any_checked = true;
        }
        any_checked && ok
    }

    fn primary_config_path(&self, home: &Path) -> Option<std::path::PathBuf> {
        Some(mcp_config_path(home))
    }
}

// ---------------------------------------------------------------------------
// Uninstall helpers
// ---------------------------------------------------------------------------

fn uninstall_mcp_server(mcp_path: &Path) {
    if !mcp_path.exists() {
        eprintln!("  {} not found, skipping", mcp_path.display());
        return;
    }

    let Ok(contents) = std::fs::read_to_string(mcp_path) else {
        return;
    };
    let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return;
    };

    let Some(servers) = settings
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
    else {
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            mcp_path.display()
        );
        return;
    };

    if servers.remove("agentic_ssh").is_none() {
        eprintln!(
            "  No agentic_ssh MCP server in {}, skipping",
            mcp_path.display()
        );
        return;
    }

    let is_empty = settings.as_object().is_some_and(|o| {
        o.iter()
            .all(|(k, v)| k == "mcpServers" && v.as_object().is_some_and(serde_json::Map::is_empty))
    });

    if is_empty {
        std::fs::remove_file(mcp_path).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed {} (was empty)",
            mcp_path.display()
        );
    } else {
        let pretty = serde_json::to_string_pretty(&settings).unwrap_or_default();
        std::fs::write(mcp_path, format!("{pretty}\n")).ok();
        eprintln!(
            "\x1b[32m✔\x1b[0m Removed agentic_ssh MCP server from {}",
            mcp_path.display()
        );
    }
}

/// Remove the per-plugin file the CLI loader picks up. Unlike the IDE config
/// — which is shared across other tools — the plugin file belongs exclusively
/// to agentic_ssh, so we just delete it.
fn uninstall_cli_plugin(plugin_path: &Path) {
    if !plugin_path.exists() {
        eprintln!("  {} not found, skipping", plugin_path.display());
        return;
    }
    if std::fs::remove_file(plugin_path).is_ok() {
        eprintln!("\x1b[32m✔\x1b[0m Removed {} ", plugin_path.display());
    }
}

// ---------------------------------------------------------------------------
// Healthcheck helpers
// ---------------------------------------------------------------------------

fn doctor_check_settings(dc: &mut DoctorCounters, home: &Path) {
    let mcp_path = mcp_config_path(home);

    if !mcp_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent antigravity` if you use Antigravity",
            mcp_path.display()
        ));
        return;
    }

    let settings = load_json_file(&mcp_path);
    let server = settings
        .get("mcpServers")
        .and_then(|v| v.get("agentic_ssh"));

    if server.and_then(|v| v.as_object()).is_some() {
        dc.pass(&format!(
            "Central MCP server registered in {}",
            mcp_path.display()
        ));
    } else {
        dc.fail(&format!(
            "MCP server NOT registered in {} — run `agentic_ssh install --agent antigravity`",
            mcp_path.display()
        ));
    }
}

fn doctor_check_cli_plugin(dc: &mut DoctorCounters, home: &Path) {
    let plugin_path = cli_plugin_path(home);

    if !plugin_path.exists() {
        dc.warn(&format!(
            "{} not found — run `agentic_ssh install --agent antigravity` if you use the Antigravity CLI (#85)",
            plugin_path.display()
        ));
        return;
    }

    let settings = load_json_file(&plugin_path);
    let server = settings
        .get("mcpServers")
        .and_then(|v| v.get("agentic_ssh"));

    if server.and_then(|v| v.as_object()).is_some() {
        dc.pass(&format!(
            "CLI plugin registered in {}",
            plugin_path.display()
        ));
    } else {
        dc.fail(&format!(
            "CLI plugin file exists but lacks `mcpServers.agentic_ssh` in {} — run `agentic_ssh install --agent antigravity`",
            plugin_path.display()
        ));
    }
}
