use anyhow::{Context, Result, anyhow};
use russh::client::Handle;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::ssh_config::{expand_path, load_ssh_config};

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

#[derive(serde::Deserialize, Debug, Clone)]
pub struct CustomTool {
    pub name: String,
    pub description: String,
    pub command: String,
}

#[derive(serde::Deserialize, Debug, Default)]
pub struct Config {
    pub pool_status_path: Option<String>,
    #[serde(default)]
    pub custom_tools: Vec<CustomTool>,
    #[serde(default)]
    pub ignore_hosts: Vec<String>,
    #[serde(default, alias = "include_hosts")]
    pub allow_hosts: Vec<String>,
}

pub fn load_config() -> Config {
    home::home_dir()
        .map(|home_dir| {
            home_dir
                .join(".config")
                .join("agentic_ssh")
                .join("config.toml")
        })
        .filter(|path| path.exists())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|content| toml::from_str::<Config>(&content).ok())
        .unwrap_or_default()
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
                        (now_unix.saturating_sub(elapsed.as_secs()), "Active".to_string())
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
                            (now_unix.saturating_sub(elapsed.as_secs()), "Active".to_string())
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
                    (now_unix.saturating_sub(elapsed.as_secs()), "Active".to_string())
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

        let config = Arc::new(russh::client::Config::default());
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

        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    stdout.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::ExtendedData { data, ext }) => {
                    if ext == 1 {
                        stderr.extend_from_slice(&data);
                    }
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = exit_status;
                }
                None => break,
                _ => {}
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
}
