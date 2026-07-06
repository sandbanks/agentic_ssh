use anyhow::{Context, Result, anyhow};
use russh::client::Handle;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::ssh_config::{expand_path, list_ssh_hosts, load_ssh_config};

#[derive(Clone)]
pub struct ClientHandler;

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all server keys for agentic SSH usage.
        Ok(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandTemplate {
    Simple(String),
    Array(Vec<String>),
}

impl<'de> serde::Deserialize<'de> for CommandTemplate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = CommandTemplate;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or an array of strings")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(CommandTemplate::Simple(value.to_string()))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element::<String>()? {
                    vec.push(elem);
                }
                Ok(CommandTemplate::Array(vec))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl serde::Serialize for CommandTemplate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            CommandTemplate::Simple(s) => s.serialize(serializer),
            CommandTemplate::Array(arr) => arr.serialize(serializer),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct ParamInfo {
    pub validation: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct PreparedTool {
    pub description: String,
    pub command: CommandTemplate,
    #[serde(default)]
    pub allow_shell: bool,
    #[serde(default)]
    pub allow_hosts: Vec<String>,
    #[serde(default)]
    pub params: HashMap<String, ParamInfo>,
}

impl PreparedTool {
    pub fn validate(&self, tool_name: &str) -> Result<()> {
        match &self.command {
            CommandTemplate::Simple(cmd_str) => {
                if !self.allow_shell {
                    anyhow::bail!(
                        "Tool '{}': command is a string, but allow_shell is false. A string command requires allow_shell = true.",
                        tool_name
                    );
                }
                let placeholders = extract_placeholders(cmd_str);
                for placeholder in &placeholders {
                    if !self.params.contains_key(placeholder) {
                        anyhow::bail!(
                            "Tool '{}': parameter '{}' is used in the command template but not defined in the params block.",
                            tool_name,
                            placeholder
                        );
                    }
                }
            }
            CommandTemplate::Array(cmd_array) => {
                if self.allow_shell {
                    anyhow::bail!(
                        "Tool '{}': command is an array, but allow_shell is true. An array command requires allow_shell = false.",
                        tool_name
                    );
                }
                for arg in cmd_array {
                    let placeholders = extract_placeholders(arg);
                    for placeholder in &placeholders {
                        if !self.params.contains_key(placeholder) {
                            anyhow::bail!(
                                "Tool '{}': parameter '{}' is used in the command template but not defined in the params block.",
                                tool_name,
                                placeholder
                            );
                        }
                    }
                }
            }
        }

        for (param_name, param_info) in &self.params {
            match param_info.validation.as_str() {
                "strict" | "path" | "permissive" => {}
                other => {
                    anyhow::bail!(
                        "Tool '{}': parameter '{}' has unrecognized validation type '{}'. Supported types are: strict, path, permissive.",
                        tool_name,
                        param_name,
                        other
                    );
                }
            }
        }

        Ok(())
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct LegacyCustomTool {
    pub name: String,
    pub description: String,
    pub command: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Default, Clone)]
pub struct Config {
    pub pool_status_path: Option<String>,
    #[serde(default)]
    pub disable_local_config: bool,
    #[serde(default)]
    pub tools: HashMap<String, PreparedTool>,
    #[serde(default)]
    pub ignore_hosts: Vec<String>,
    #[serde(default, alias = "include_hosts")]
    pub allow_hosts: Vec<String>,
    #[serde(default)]
    pub groups: HashMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_tools: Vec<LegacyCustomTool>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        for (name, tool) in &self.tools {
            tool.validate(name)?;
        }

        // Loop through all group keys and ensure none clash with existing SSH aliases
        let ssh_hosts: Vec<String> = list_ssh_hosts()
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.to_lowercase())
            .collect();
        for group_name in self.groups.keys() {
            if ssh_hosts.contains(&group_name.to_lowercase()) {
                anyhow::bail!(
                    "Configuration Error: Group name '{}' clashes with an existing SSH host alias.",
                    group_name
                );
            }
        }

        Ok(())
    }
}

pub fn extract_placeholders(s: &str) -> Vec<String> {
    let re = regex::Regex::new(r"\{\{\s*([a-zA-Z0-9_-]+)\s*\}\}").unwrap();
    re.captures_iter(s).map(|cap| cap[1].to_string()).collect()
}

pub fn replace_placeholders(template: &str, args: &HashMap<String, String>) -> Result<String> {
    let re = regex::Regex::new(r"\{\{\s*([a-zA-Z0-9_-]+)\s*\}\}").unwrap();
    let mut err = None;
    let result = re.replace_all(template, |caps: &regex::Captures| {
        let param_name = &caps[1];
        match args.get(param_name) {
            Some(val) => val.clone(),
            None => {
                err = Some(anyhow!("Missing value for parameter '{}'", param_name));
                caps[0].to_string()
            }
        }
    });
    if let Some(e) = err {
        Err(e)
    } else {
        Ok(result.into_owned())
    }
}

pub fn shell_escape(arg: &str) -> String {
    let mut escaped = String::new();
    escaped.push('\'');
    for c in arg.chars() {
        if c == '\'' {
            escaped.push_str("'\\''");
        } else {
            escaped.push(c);
        }
    }
    escaped.push('\'');
    escaped
}

pub fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_escape(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn validate_param_value(value: &str, rule: &str) -> bool {
    match rule {
        "strict" => {
            !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }
        "path" => {
            !value.is_empty()
                && value.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_'
                })
        }
        "permissive" => true,
        _ => false,
    }
}

pub fn is_host_allowed_for_tool(
    host: &str,
    resolved_host: Option<&str>,
    allow_hosts: &[String],
) -> bool {
    if allow_hosts.is_empty() {
        return true;
    }
    let host_lower = host.to_lowercase();
    let resolved_lower = resolved_host.map(|s| s.to_lowercase());

    for pattern_str in allow_hosts {
        let pattern_lower = pattern_str.to_lowercase();
        if let Ok(pattern) = glob::Pattern::new(&pattern_lower) {
            if pattern.matches(&host_lower) {
                return true;
            }
            if resolved_lower
                .as_ref()
                .is_some_and(|res_host| pattern.matches(res_host))
            {
                return true;
            }
        }
    }
    false
}

pub fn load_config_from_str(content: &str) -> Result<Config> {
    let config: Config = toml::from_str(content).context("Failed to parse TOML configuration")?;
    config
        .validate()
        .context("Configuration validation failed")?;
    Ok(config)
}

#[derive(Debug, Clone)]
pub struct CliOverride {
    pub config_path: Option<PathBuf>,
    pub no_global: bool,
}

pub static CLI_OVERRIDE: OnceLock<CliOverride> = OnceLock::new();

pub fn find_local_config(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(".agentic_ssh.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    None
}

fn validation_level(v: &str) -> i32 {
    match v {
        "strict" => 1,
        "path" => 2,
        "permissive" => 3,
        _ => 3,
    }
}

pub fn merge_configs(mut global: Config, local: Config, trusted: bool) -> Config {
    if trusted {
        for (name, tool) in local.tools {
            global.tools.insert(name, tool);
        }
        for host in local.ignore_hosts {
            if !global.ignore_hosts.contains(&host) {
                global.ignore_hosts.push(host);
            }
        }
        for host in local.allow_hosts {
            if !global.allow_hosts.contains(&host) {
                global.allow_hosts.push(host);
            }
        }
    } else {
        for (name, global_tool) in global.tools.iter_mut() {
            if let Some(local_tool) = local.tools.get(name) {
                global_tool.allow_shell = global_tool.allow_shell && local_tool.allow_shell;

                if global_tool.allow_hosts.is_empty() {
                    if !local_tool.allow_hosts.is_empty() {
                        global_tool.allow_hosts = local_tool.allow_hosts.clone();
                    }
                } else {
                    if !local_tool.allow_hosts.is_empty() {
                        let mut intersected = Vec::new();
                        for pattern in &local_tool.allow_hosts {
                            if global_tool.allow_hosts.contains(pattern) {
                                intersected.push(pattern.clone());
                            }
                        }
                        if intersected.is_empty() {
                            intersected.push("untrusted_empty_intersection_blocked".to_string());
                        }
                        global_tool.allow_hosts = intersected;
                    }
                }

                for (param_name, global_param) in global_tool.params.iter_mut() {
                    if let Some(local_param) = local_tool.params.get(param_name) {
                        let g_level = validation_level(&global_param.validation);
                        let l_level = validation_level(&local_param.validation);
                        if l_level < g_level {
                            global_param.validation = local_param.validation.clone();
                        }
                    }
                }
            }
        }
    }

    for (name, group) in local.groups {
        global.groups.insert(name, group);
    }

    global
}

fn migrate_legacy_tools(cfg: &mut Config, path: &Path) {
    if !cfg.custom_tools.is_empty() {
        let legacy_tools = std::mem::take(&mut cfg.custom_tools);
        for custom in legacy_tools {
            let mut command_str = custom.command.clone();
            let mut params = HashMap::new();
            if command_str.contains("{args}") {
                command_str = command_str.replace("{args}", "{{args}}");
                params.insert(
                    "args".to_string(),
                    ParamInfo {
                        validation: "permissive".to_string(),
                    },
                );
            }
            cfg.tools.insert(
                custom.name,
                PreparedTool {
                    description: custom.description,
                    command: CommandTemplate::Simple(command_str),
                    allow_shell: true,
                    allow_hosts: Vec::new(),
                    params,
                },
            );
        }

        if let Ok(toml_string) = toml::to_string_pretty(&cfg) {
            let _ = std::fs::write(path, toml_string).map_err(|e| {
                eprintln!(
                    "Warning: failed to write migrated config to {:?}: {}",
                    path, e
                );
            });
        }
    }
}

pub fn load_config() -> Config {
    let overrides = CLI_OVERRIDE.get();
    let config_path_override = overrides.and_then(|o| o.config_path.clone());
    let no_global = overrides.map(|o| o.no_global).unwrap_or(false);

    let mut cfg = if no_global {
        Config::default()
    } else {
        let global_path = match home::home_dir() {
            Some(home_dir) => home_dir
                .join(".config")
                .join("agentic_ssh")
                .join("config.toml"),
            None => PathBuf::new(),
        };

        if global_path.exists() {
            let content = match std::fs::read_to_string(&global_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading global config file {:?}: {}", global_path, e);
                    std::process::exit(1);
                }
            };
            match load_config_from_str(&content) {
                Ok(mut global_cfg) => {
                    migrate_legacy_tools(&mut global_cfg, &global_path);
                    global_cfg
                }
                Err(e) => {
                    eprintln!(
                        "Configuration error in global config {:?}: {}",
                        global_path, e
                    );
                    std::process::exit(1);
                }
            }
        } else {
            Config::default()
        }
    };

    let local_path = if let Some(ref explicit_path) = config_path_override {
        Some(explicit_path.clone())
    } else if !cfg.disable_local_config {
        if let Ok(current_dir) = std::env::current_dir() {
            find_local_config(&current_dir)
        } else {
            None
        }
    } else {
        None
    };

    if let Some(path) = local_path.as_ref().filter(|p| p.exists()) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error reading local config file {:?}: {}", path, e);
                std::process::exit(1);
            }
        };
        match load_config_from_str(&content) {
            Ok(mut local_cfg) => {
                migrate_legacy_tools(&mut local_cfg, path);
                let is_trusted = crate::security::is_config_trusted(path).unwrap_or(false);
                cfg = merge_configs(cfg, local_cfg, is_trusted);
            }
            Err(e) => {
                eprintln!("Configuration error in local config {:?}: {}", path, e);
                std::process::exit(1);
            }
        }
    }

    cfg
}

pub fn is_host_ignored(host: &str, resolved_host: Option<&str>) -> bool {
    let config = load_config();
    is_host_ignored_impl(
        host,
        resolved_host,
        &config.ignore_hosts,
        &config.allow_hosts,
    )
}

fn is_host_ignored_impl(
    host: &str,
    resolved_host: Option<&str>,
    ignore_hosts: &[String],
    allow_hosts: &[String],
) -> bool {
    let host_lower = host.to_lowercase();
    let resolved_lower = resolved_host.map(|s| s.to_lowercase());

    // 1. If allowlist is not empty, host must match at least one allow pattern
    if !allow_hosts.is_empty() {
        let mut allowed = false;
        for pattern_str in allow_hosts {
            let pattern_lower = pattern_str.to_lowercase();
            if let Ok(pattern) = glob::Pattern::new(&pattern_lower) {
                if pattern.matches(&host_lower) {
                    allowed = true;
                    break;
                }
                if resolved_lower
                    .as_ref()
                    .is_some_and(|res_host| pattern.matches(res_host))
                {
                    allowed = true;
                    break;
                }
            }
        }
        if !allowed {
            return true; // Blocked because it's not in the allowlist
        }
    }

    // 2. If it matches any pattern in ignore_hosts, it is ignored
    for pattern_str in ignore_hosts {
        // If it's "*" and we already matched allowlist, skip it
        if pattern_str == "*" && !allow_hosts.is_empty() {
            continue;
        }
        let pattern_lower = pattern_str.to_lowercase();
        if let Ok(pattern) = glob::Pattern::new(&pattern_lower) {
            if pattern.matches(&host_lower) {
                return true;
            }
            if resolved_lower
                .as_ref()
                .is_some_and(|res_host| pattern.matches(res_host))
            {
                return true;
            }
        }
    }

    false
}

pub fn get_pool_status_path() -> std::path::PathBuf {
    if let Ok(val) = std::env::var("AGENTIC_SSH_POOL_STATUS") {
        return std::path::PathBuf::from(val);
    }
    let config = load_config();
    if let Some(ref path_str) = config.pool_status_path {
        let raw_path = std::path::Path::new(path_str);
        return expand_path(raw_path);
    }
    if let Some(home_dir) = home::home_dir() {
        return home_dir.join(".agentic_ssh_pool_status.json");
    }
    std::path::PathBuf::from("pool_status.json")
}

pub struct SshConnection {
    pub handle: Arc<Handle<ClientHandler>>,
    pub last_used: Instant,
    pub active_operations: usize,
    pub idle_timeout_secs: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ConnectionStatus {
    pub host: String,
    pub last_used_timestamp: u64,
    pub idle_timeout_secs: u64,
    #[serde(default)]
    pub status: String,
}

pub struct ConnectionPool {
    connections: Arc<Mutex<HashMap<String, SshConnection>>>,
    idle_timeout: Duration,
}

pub struct ActiveOperationGuard {
    connections: Arc<Mutex<HashMap<String, SshConnection>>>,
    host: String,
    pool_status_path: std::path::PathBuf,
}

impl Drop for ActiveOperationGuard {
    fn drop(&mut self) {
        let connections = Arc::clone(&self.connections);
        let host = self.host.clone();
        let path = self.pool_status_path.clone();
        tokio::spawn(async move {
            let mut map = connections.lock().await;
            if let Some(conn) = map.get_mut(&host) {
                conn.active_operations = conn.active_operations.saturating_sub(1);
                if conn.active_operations == 0 {
                    conn.last_used = Instant::now();
                }
            }
            // Save status
            let now = Instant::now();
            let now_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let statuses: Vec<ConnectionStatus> = map
                .iter()
                .map(|(h, c)| {
                    let (last_used_unix, status_str) = if c.active_operations > 0 {
                        (now_unix, "Executing".to_string())
                    } else {
                        let elapsed = now.duration_since(c.last_used);
                        (
                            now_unix.saturating_sub(elapsed.as_secs()),
                            "Active".to_string(),
                        )
                    };
                    ConnectionStatus {
                        host: h.clone(),
                        last_used_timestamp: last_used_unix,
                        idle_timeout_secs: c.idle_timeout_secs,
                        status: status_str,
                    }
                })
                .collect();
            if let Ok(file) = std::fs::File::create(path) {
                let _ = serde_json::to_writer_pretty(file, &statuses);
            }
        });
    }
}

impl ConnectionPool {
    pub fn new(idle_timeout: Duration) -> Self {
        let connections: Arc<Mutex<HashMap<String, SshConnection>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let pool = Self {
            connections: Arc::clone(&connections),
            idle_timeout,
        };

        // Start background cleaner task
        let pool_clone = Arc::clone(&connections);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let mut map = pool_clone.lock().await;
                let now = Instant::now();
                map.retain(|host, conn| {
                    if conn.active_operations > 0 {
                        true
                    } else if now.duration_since(conn.last_used) > idle_timeout {
                        eprintln!("Closing idle connection to host: {}", host);
                        false
                    } else {
                        true
                    }
                });

                let now_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let statuses: Vec<ConnectionStatus> = map
                    .iter()
                    .map(|(host, conn)| {
                        let (last_used_unix, status_str) = if conn.active_operations > 0 {
                            (now_unix, "Executing".to_string())
                        } else {
                            let elapsed = now.duration_since(conn.last_used);
                            (
                                now_unix.saturating_sub(elapsed.as_secs()),
                                "Active".to_string(),
                            )
                        };
                        ConnectionStatus {
                            host: host.clone(),
                            last_used_timestamp: last_used_unix,
                            idle_timeout_secs: idle_timeout.as_secs(),
                            status: status_str,
                        }
                    })
                    .collect();
                if let Ok(file) = std::fs::File::create(get_pool_status_path()) {
                    let _ = serde_json::to_writer_pretty(file, &statuses);
                }
            }
        });

        pool
    }

    async fn save_status(&self) {
        let map = self.connections.lock().await;
        let now = Instant::now();
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let statuses: Vec<ConnectionStatus> = map
            .iter()
            .map(|(host, conn)| {
                let (last_used_unix, status_str) = if conn.active_operations > 0 {
                    (now_unix, "Executing".to_string())
                } else {
                    let elapsed = now.duration_since(conn.last_used);
                    (
                        now_unix.saturating_sub(elapsed.as_secs()),
                        "Active".to_string(),
                    )
                };
                ConnectionStatus {
                    host: host.clone(),
                    last_used_timestamp: last_used_unix,
                    idle_timeout_secs: self.idle_timeout.as_secs(),
                    status: status_str,
                }
            })
            .collect();
        if let Ok(file) = std::fs::File::create(get_pool_status_path()) {
            let _ = serde_json::to_writer_pretty(file, &statuses);
        }
    }

    pub async fn start_operation(&self, host: &str) -> ActiveOperationGuard {
        let mut map = self.connections.lock().await;
        if let Some(conn) = map.get_mut(host) {
            conn.active_operations += 1;
        }
        drop(map);
        self.save_status().await;

        ActiveOperationGuard {
            connections: Arc::clone(&self.connections),
            host: host.to_string(),
            pool_status_path: get_pool_status_path(),
        }
    }

    /// Gets or creates a connection to the specified host.
    pub async fn get_connection(&self, host: &str) -> Result<Arc<Handle<ClientHandler>>> {
        let real_host = {
            let ssh_config = crate::ssh_config::load_ssh_config().unwrap_or_default();
            let params = ssh_config.query(host);
            params.host_name.as_deref().unwrap_or(host).to_string()
        };

        if is_host_ignored(host, Some(&real_host)) {
            return Err(anyhow!(
                "Access to host '{}' (resolved: '{}') is blocked by ignore rules",
                host,
                real_host
            ));
        }

        let mut map = self.connections.lock().await;

        // Check if we have an active connection that's still working
        if let Some(conn) = map.get_mut(host) {
            // Test if the connection is alive by checking if we can open a channel
            match conn.handle.channel_open_session().await {
                Ok(channel) => {
                    // It works! We can close this test channel immediately and return the handle.
                    let _ = channel.close().await;
                    conn.last_used = Instant::now();
                    let handle = Arc::clone(&conn.handle);
                    drop(map);
                    self.save_status().await;
                    return Ok(handle);
                }
                Err(_) => {
                    // Connection is dead, remove it from the pool and build a new one.
                    eprintln!("Existing connection to {} was dead. Reconnecting...", host);
                    map.remove(host);
                }
            }
        }

        // Create new connection
        let handle = Arc::new(self.connect_new(host).await?);
        map.insert(
            host.to_string(),
            SshConnection {
                handle: Arc::clone(&handle),
                last_used: Instant::now(),
                active_operations: 0,
                idle_timeout_secs: self.idle_timeout.as_secs(),
            },
        );

        drop(map);
        self.save_status().await;

        Ok(handle)
    }

    async fn connect_new(&self, host: &str) -> Result<Handle<ClientHandler>> {
        let ssh_config = load_ssh_config().unwrap_or_default();
        let params = ssh_config.query(host);

        let real_host = params.host_name.as_deref().unwrap_or(host);
        let port = params.port.unwrap_or(22);

        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let user = params.user.as_deref().unwrap_or(&current_user);

        // Build list of keys to try
        let mut keys_to_try = Vec::new();
        if let Some(ref identity_files) = params.identity_file {
            for id_file in identity_files {
                keys_to_try.push(expand_path(id_file));
            }
        }

        // Always append standard default keys as fallbacks (mimicking OpenSSH)
        if let Some(home_dir) = home::home_dir() {
            let ssh_dir = home_dir.join(".ssh");
            keys_to_try.push(ssh_dir.join("id_rsa"));
            keys_to_try.push(ssh_dir.join("id_ed25519"));
            keys_to_try.push(ssh_dir.join("id_ecdsa"));
            keys_to_try.push(ssh_dir.join("id_dsa"));
        }

        // Deduplicate key files while keeping the original order
        let mut seen = std::collections::HashSet::new();
        keys_to_try.retain(|x| seen.insert(x.clone()));

        eprintln!(
            "Connecting to {} ({}:{}) as user {}...",
            host, real_host, port, user
        );

        // Resolve host to socket address
        let addr_str = format!("{}:{}", real_host, port);
        let addrs = tokio::net::lookup_host(&addr_str)
            .await
            .with_context(|| format!("Failed to resolve address: {}", addr_str))?;
        let socket_addr = addrs
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Could not resolve host: {}", real_host))?;

        let config = Arc::new(russh::client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            ..Default::default()
        });
        let mut handle = russh::client::connect(config, socket_addr, ClientHandler)
            .await
            .context("Failed to connect via TCP/SSH")?;

        let mut authenticated = false;

        // Try SSH Agent first if available
        if let Ok(socket_path) = std::env::var("SSH_AUTH_SOCK") {
            eprintln!(
                "SSH_AUTH_SOCK found at {:?}. Attempting agent authentication...",
                socket_path
            );
            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(stream) => {
                    let mut agent_client = russh::keys::agent::client::AgentClient::connect(stream);
                    match agent_client.request_identities().await {
                        Ok(identities) => {
                            eprintln!("Found {} keys in SSH agent", identities.len());
                            for identity in identities {
                                eprintln!("Trying agent key...");
                                match handle
                                    .authenticate_publickey_with(
                                        user,
                                        identity.public_key().into_owned(),
                                        None,
                                        &mut agent_client,
                                    )
                                    .await
                                {
                                    Ok(success) => {
                                        if success.success() {
                                            eprintln!(
                                                "Authentication succeeded for {} using SSH agent key",
                                                host
                                            );
                                            authenticated = true;
                                            break;
                                        } else {
                                            eprintln!("Server rejected agent key");
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Error with agent authentication: {:?}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to request identities from SSH agent: {:?}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to connect to SSH agent socket: {:?}", e);
                }
            }
        }

        if !authenticated {
            for key_path in &keys_to_try {
                if !key_path.exists() {
                    continue;
                }
                eprintln!("Attempting authentication with key: {:?}", key_path);
                if let Ok(key) = russh::keys::load_secret_key(key_path, None) {
                    let key_with_alg = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
                    match handle.authenticate_publickey(user, key_with_alg).await {
                        Ok(success) => {
                            if success.success() {
                                eprintln!(
                                    "Authentication succeeded for {} using {:?}",
                                    host, key_path
                                );
                                authenticated = true;
                                break;
                            } else {
                                eprintln!("Server rejected key {:?}", key_path);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error authenticating with key {:?}: {:?}", key_path, e);
                        }
                    }
                }
            }
        }

        if !authenticated {
            return Err(anyhow!(
                "Failed to authenticate connection to {} as user {}. No working keys found.",
                host,
                user
            ));
        }

        Ok(handle)
    }

    /// Runs a command on a host, updating the last used time.
    pub async fn execute_command(
        &self,
        host: &str,
        command: &str,
        quiet: bool,
        progress_interval_secs: u64,
        log_path: std::path::PathBuf,
    ) -> Result<(String, String, u32)> {
        let handle = self.get_connection(host).await?;
        let _guard = self.start_operation(host).await;

        eprintln!("Executing command on {}: {:?}", host, command);
        let mut channel = handle
            .channel_open_session()
            .await
            .context("Failed to open SSH channel")?;

        channel
            .exec(true, command)
            .await
            .context("Failed to request command execution")?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code = 0;

        // Open progress log file
        use tokio::io::AsyncWriteExt;
        let mut log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .await
            .ok();

        if let Some(ref mut f) = log_file {
            let header = format!("--- Executing command on {}: {:?} ---\n", host, command);
            let _ = f.write_all(header.as_bytes()).await;
            let _ = f.flush().await;
        }

        let start_time = Instant::now();
        // If quiet is false, we don't tick, so set first tick to 100 years in the future
        let mut next_tick = if quiet {
            start_time + Duration::from_secs(5)
        } else {
            start_time + Duration::from_secs(3600 * 24 * 365 * 100)
        };

        loop {
            tokio::select! {
                msg = channel.wait() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { data }) => {
                            stdout.extend_from_slice(&data);
                            if !quiet {
                                let _ = tokio::io::stderr().write_all(&data).await;
                                let _ = tokio::io::stderr().flush().await;
                                if let Some(ref mut f) = log_file {
                                    let _ = f.write_all(&data).await;
                                    let _ = f.flush().await;
                                }
                            }
                        }
                        Some(russh::ChannelMsg::ExtendedData { data, ext }) => {
                            if ext == 1 {
                                stderr.extend_from_slice(&data);
                                if !quiet {
                                    let _ = tokio::io::stderr().write_all(&data).await;
                                    let _ = tokio::io::stderr().flush().await;
                                    if let Some(ref mut f) = log_file {
                                        let _ = f.write_all(&data).await;
                                        let _ = f.flush().await;
                                    }
                                }
                            }
                        }
                        Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                            exit_code = exit_status;
                        }
                        None => break,
                        _ => {}
                    }
                }
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(next_tick)) => {
                    if quiet {
                        let elapsed = start_time.elapsed();
                        let total_kb = (stdout.len() + stderr.len()) / 1024;
                        let msg = format!(
                            "[agentic_ssh] -> [Progress: {}KB read from streams...] (elapsed: {}s)\n",
                            total_kb,
                            elapsed.as_secs()
                        );
                        eprint!("{}", msg);
                        if let Some(ref mut f) = log_file {
                            let _ = f.write_all(msg.as_bytes()).await;
                            let _ = f.flush().await;
                        }
                    }
                    next_tick = Instant::now() + Duration::from_secs(progress_interval_secs);
                }
            }
        }

        let stdout_str = String::from_utf8_lossy(&stdout).into_owned();
        let stderr_str = String::from_utf8_lossy(&stderr).into_owned();

        Ok((stdout_str, stderr_str, exit_code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_connect_long_hostname() {
        let pool = ConnectionPool::new(Duration::from_secs(300));

        println!("CONNECTING TO LONG HOSTNAME...");
        match pool
            .execute_command(
                "fred-direct-with-more-words-than-you-need-to-test-with",
                "uptime && docker ps",
                true,
                5,
                std::path::PathBuf::from("/tmp/test.log"),
            )
            .await
        {
            Ok((stdout, stderr, code)) => {
                println!(
                    "SUCCESS: code={}, stdout={}, stderr={}",
                    code, stdout, stderr
                );
            }
            Err(e) => {
                println!("FAILED: {:#?}", e);
            }
        }

        println!("WAITING 15 SECONDS FOR TUI DISPLAY... (Watch the TUI!)");
        tokio::time::sleep(Duration::from_secs(15)).await;
    }

    #[test]
    fn test_is_host_ignored() {
        let ignore_list = vec![
            "*.prod.company.com".to_string(),
            "secure-*".to_string(),
            "db-prod".to_string(),
        ];
        let allow_list = vec![];

        // Exact match
        assert!(is_host_ignored_impl(
            "db-prod",
            None,
            &ignore_list,
            &allow_list
        ));
        // Substring case insensitive
        assert!(is_host_ignored_impl(
            "DB-PROD",
            None,
            &ignore_list,
            &allow_list
        ));

        // Glob matches
        assert!(is_host_ignored_impl(
            "app.prod.company.com",
            None,
            &ignore_list,
            &allow_list
        ));
        assert!(is_host_ignored_impl(
            "secure-host-1",
            None,
            &ignore_list,
            &allow_list
        ));

        // Resolved hostname match
        assert!(is_host_ignored_impl(
            "my-alias",
            Some("database.prod.company.com"),
            &ignore_list,
            &allow_list
        ));
        assert!(is_host_ignored_impl(
            "my-alias",
            Some("SECURE-GATEWAY"),
            &ignore_list,
            &allow_list
        ));

        // Non-matching
        assert!(!is_host_ignored_impl(
            "dev-server",
            Some("10.0.0.5"),
            &ignore_list,
            &allow_list
        ));
        assert!(!is_host_ignored_impl(
            "company.com",
            None,
            &ignore_list,
            &allow_list
        ));

        // Test allowlist block by default
        let allow_list_2 = vec!["*.staging.company.com".to_string(), "my-host".to_string()];
        assert!(!is_host_ignored_impl(
            "my-host",
            None,
            &ignore_list,
            &allow_list_2
        ));
        assert!(is_host_ignored_impl(
            "other-host",
            None,
            &ignore_list,
            &allow_list_2
        )); // Blocked by default because not in allowlist

        // Test ignore all except allowlist
        let ignore_all = vec!["*".to_string()];
        assert!(!is_host_ignored_impl(
            "my-host",
            None,
            &ignore_all,
            &allow_list_2
        )); // Allowed because it is in allowlist (even though ignore_all is *)
        assert!(is_host_ignored_impl(
            "other-host",
            None,
            &ignore_all,
            &allow_list_2
        )); // Blocked by default and by ignore_all
    }

    #[test]
    fn test_config_parsing_and_validation() {
        // 1. Valid config with allow_shell = false (Array) and strict parameter validation
        let content1 = r#"
            [tools.git_pull]
            description = "Fetches and merges"
            command = ["git", "pull", "origin", "{{branch}}"]
            allow_hosts = ["dev-*"]
            [tools.git_pull.params.branch]
            validation = "strict"
        "#;
        let cfg1 = load_config_from_str(content1);
        assert!(cfg1.is_ok());
        let cfg1 = cfg1.unwrap();
        assert_eq!(cfg1.tools.len(), 1);
        let tool = cfg1.tools.get("git_pull").unwrap();
        assert!(!tool.allow_shell);
        assert!(matches!(tool.command, CommandTemplate::Array(_)));

        // 2. Mismatch: allow_shell = true but command is Array -> Validation error
        let content2 = r#"
            [tools.bad_tool]
            description = "Mismatched shell flag"
            command = ["git", "pull"]
            allow_shell = true
        "#;
        let cfg2 = load_config_from_str(content2);
        assert!(cfg2.is_err());
        let err_msg = format!("{:#}", cfg2.err().unwrap());
        assert!(err_msg.contains("allow_shell is true"));

        // 3. Mismatch: allow_shell = false (default) but command is String -> Validation error
        let content3 = r#"
            [tools.bad_tool]
            description = "Mismatched shell flag"
            command = "git pull"
            allow_shell = false
        "#;
        let cfg3 = load_config_from_str(content3);
        assert!(cfg3.is_err());
        let err_msg = format!("{:#}", cfg3.err().unwrap());
        assert!(err_msg.contains("allow_shell is false"));

        // 4. Missing param definition in params block -> Validation error
        let content4 = r#"
            [tools.git_pull]
            description = "Missing param config"
            command = ["git", "pull", "{{branch}}"]
            allow_shell = false
        "#;
        let cfg4 = load_config_from_str(content4);
        assert!(cfg4.is_err());
        let err_msg = format!("{:#}", cfg4.err().unwrap());
        assert!(err_msg.contains(
            "parameter 'branch' is used in the command template but not defined in the params block"
        ));

        // 5. Unrecognized validation rule -> Validation error
        let content5 = r#"
            [tools.git_pull]
            description = "Unrecognized validation rule"
            command = ["git", "pull", "{{branch}}"]
            [tools.git_pull.params.branch]
            validation = "super-strict"
        "#;
        let cfg5 = load_config_from_str(content5);
        assert!(cfg5.is_err());
        let err_msg = format!("{:#}", cfg5.err().unwrap());
        assert!(err_msg.contains("unrecognized validation type 'super-strict'"));
    }

    #[test]
    fn test_parameter_validation_rules() {
        // strict: pure alphanumeric + hyphens
        assert!(validate_param_value("main-branch-01", "strict"));
        assert!(validate_param_value("main", "strict"));
        assert!(!validate_param_value("main_branch", "strict")); // underscore not allowed in strict
        assert!(!validate_param_value("main;rm", "strict"));
        assert!(!validate_param_value("", "strict"));

        // path: alphanumeric plus /, ., -, _
        assert!(validate_param_value("/var/log/app.log", "path"));
        assert!(validate_param_value("relative/path/file.txt", "path"));
        assert!(!validate_param_value("/var/log/app;rm.log", "path"));
        assert!(!validate_param_value("", "path"));

        // permissive: any
        assert!(validate_param_value(
            "anything-goes; rm -rf /",
            "permissive"
        ));
        assert!(validate_param_value("", "permissive"));
    }

    #[test]
    fn test_shell_escaping_and_joining() {
        let args = vec![
            "git".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            "hello 'world'; rm -rf /".to_string(),
        ];
        let escaped = shell_join(&args);
        assert_eq!(
            escaped,
            r#"'git' 'commit' '-m' 'hello '\''world'\''; rm -rf /'"#
        );
    }

    #[test]
    fn test_host_allowed_for_tool() {
        let allowed = vec!["dev-*".to_string(), "staging-*".to_string()];

        assert!(is_host_allowed_for_tool("dev-box", None, &allowed));
        assert!(is_host_allowed_for_tool(
            "staging-server-01",
            None,
            &allowed
        ));
        assert!(!is_host_allowed_for_tool("prod-box", None, &allowed));

        // Resolved hostname match
        assert!(is_host_allowed_for_tool(
            "my-alias",
            Some("dev-box-resolved"),
            &allowed
        ));
        assert!(!is_host_allowed_for_tool(
            "my-alias",
            Some("prod-box-resolved"),
            &allowed
        ));

        // Empty allow_hosts means allowed everywhere
        assert!(is_host_allowed_for_tool("prod-box", None, &[]));
    }

    #[test]
    fn test_config_migration() {
        let legacy_content = r#"
            pool_status_path = "/path/to/status.json"
            ignore_hosts = ["ignored-host"]
            allow_hosts = ["allowed-host"]

            [[custom_tools]]
            name = "legacy_tool_no_args"
            description = "Legacy tool without args"
            command = "uptime"

            [[custom_tools]]
            name = "legacy_tool_with_args"
            description = "Legacy tool with args"
            command = "grep -i '{args}' /var/log/syslog"
        "#;

        let mut cfg = load_config_from_str(legacy_content).unwrap();
        assert_eq!(cfg.custom_tools.len(), 2);

        // Perform migration manually since load_config_from_str doesn't mutate/write
        let legacy_tools = std::mem::take(&mut cfg.custom_tools);
        for custom in legacy_tools {
            let mut command_str = custom.command.clone();
            let mut params = HashMap::new();
            if command_str.contains("{args}") {
                command_str = command_str.replace("{args}", "{{args}}");
                params.insert(
                    "args".to_string(),
                    ParamInfo {
                        validation: "permissive".to_string(),
                    },
                );
            }
            cfg.tools.insert(
                custom.name,
                PreparedTool {
                    description: custom.description,
                    command: CommandTemplate::Simple(command_str),
                    allow_shell: true,
                    allow_hosts: Vec::new(),
                    params,
                },
            );
        }

        assert_eq!(cfg.custom_tools.len(), 0);
        assert_eq!(cfg.tools.len(), 2);

        let t1 = cfg.tools.get("legacy_tool_no_args").unwrap();
        assert!(t1.allow_shell);
        assert!(matches!(t1.command, CommandTemplate::Simple(_)));
        assert_eq!(t1.params.len(), 0);

        let t2 = cfg.tools.get("legacy_tool_with_args").unwrap();
        assert!(t2.allow_shell);
        assert!(
            matches!(t2.command, CommandTemplate::Simple(ref s) if s == "grep -i '{{args}}' /var/log/syslog")
        );
        assert_eq!(t2.params.len(), 1);
        assert_eq!(t2.params.get("args").unwrap().validation, "permissive");
    }

    #[test]
    fn test_find_local_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sub_dir = temp_dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&sub_dir).unwrap();

        assert!(find_local_config(&sub_dir).is_none());

        let config_path = temp_dir.path().join("a").join(".agentic_ssh.toml");
        std::fs::write(&config_path, "disable_local_config = false").unwrap();

        let found = find_local_config(&sub_dir);
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().canonicalize().unwrap(),
            config_path.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_merge_configs_trusted() {
        let mut global = Config::default();
        global.tools.insert(
            "global_tool".to_string(),
            PreparedTool {
                description: "Global Tool".to_string(),
                command: CommandTemplate::Simple("uptime".to_string()),
                allow_shell: true,
                allow_hosts: vec!["dev-box".to_string()],
                params: HashMap::new(),
            },
        );

        let mut local = Config::default();
        local.tools.insert(
            "local_tool".to_string(),
            PreparedTool {
                description: "Local Tool".to_string(),
                command: CommandTemplate::Simple("hostname".to_string()),
                allow_shell: true,
                allow_hosts: Vec::new(),
                params: HashMap::new(),
            },
        );
        local.tools.insert(
            "global_tool".to_string(),
            PreparedTool {
                description: "Global Tool Widened".to_string(),
                command: CommandTemplate::Simple("uptime -p".to_string()),
                allow_shell: true,
                allow_hosts: Vec::new(),
                params: HashMap::new(),
            },
        );

        let merged = merge_configs(global, local, true);
        assert_eq!(merged.tools.len(), 2);

        let gt = merged.tools.get("global_tool").unwrap();
        assert_eq!(gt.description, "Global Tool Widened");
        assert!(gt.allow_hosts.is_empty());

        let lt = merged.tools.get("local_tool").unwrap();
        assert_eq!(lt.description, "Local Tool");
    }

    #[test]
    fn test_merge_configs_untrusted() {
        let mut global = Config::default();
        let mut global_params = HashMap::new();
        global_params.insert(
            "arg".to_string(),
            ParamInfo {
                validation: "strict".to_string(),
            },
        );
        global_params.insert(
            "arg2".to_string(),
            ParamInfo {
                validation: "permissive".to_string(),
            },
        );

        global.tools.insert(
            "global_tool".to_string(),
            PreparedTool {
                description: "Global Tool".to_string(),
                command: CommandTemplate::Simple("uptime".to_string()),
                allow_shell: true,
                allow_hosts: vec!["dev-box".to_string()],
                params: global_params,
            },
        );

        let mut local = Config::default();
        local.tools.insert(
            "local_tool".to_string(),
            PreparedTool {
                description: "Local Tool".to_string(),
                command: CommandTemplate::Simple("hostname".to_string()),
                allow_shell: true,
                allow_hosts: Vec::new(),
                params: HashMap::new(),
            },
        );

        let mut local_params = HashMap::new();
        local_params.insert(
            "arg".to_string(),
            ParamInfo {
                validation: "permissive".to_string(),
            },
        );
        local_params.insert(
            "arg2".to_string(),
            ParamInfo {
                validation: "strict".to_string(),
            },
        );

        local.tools.insert(
            "global_tool".to_string(),
            PreparedTool {
                description: "Attempted Malicious Overwrite".to_string(),
                command: CommandTemplate::Simple("rm -rf /".to_string()),
                allow_shell: true,
                allow_hosts: Vec::new(),
                params: local_params,
            },
        );

        let merged = merge_configs(global, local, false);
        assert_eq!(merged.tools.len(), 1);
        assert!(!merged.tools.contains_key("local_tool"));

        let gt = merged.tools.get("global_tool").unwrap();
        assert_eq!(gt.description, "Global Tool");
        assert_eq!(gt.command, CommandTemplate::Simple("uptime".to_string()));
        assert_eq!(gt.allow_hosts, vec!["dev-box".to_string()]);
        assert_eq!(gt.params.get("arg").unwrap().validation, "strict");
        assert_eq!(gt.params.get("arg2").unwrap().validation, "strict");
    }

    #[test]
    fn test_group_clash_validation() {
        let mut config = Config::default();
        let ssh_hosts = list_ssh_hosts().unwrap_or_default();
        if !ssh_hosts.is_empty() {
            let clash_host = &ssh_hosts[0];
            config
                .groups
                .insert(clash_host.clone(), vec!["another-host".to_string()]);
            assert!(config.validate().is_err());
            let err_msg = format!("{:#}", config.validate().err().unwrap());
            assert!(err_msg.contains("clashes with an existing SSH host alias"));
        }

        let mut config_ok = Config::default();
        config_ok.groups.insert(
            "non-existent-unique-group-name-xyz".to_string(),
            vec!["host1".to_string()],
        );
        assert!(config_ok.validate().is_ok());
    }
}
