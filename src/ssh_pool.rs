use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Context, Result};
use tokio::sync::Mutex;
use russh::client::Handle;

use crate::ssh_config::{expand_path, load_ssh_config};

#[derive(Clone)]
pub struct ClientHandler;

#[async_trait::async_trait]
impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all server keys for agentic SSH usage.
        Ok(true)
    }
}

pub struct SshConnection {
    pub handle: Arc<Handle<ClientHandler>>,
    pub last_used: Instant,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct ConnectionStatus {
    pub host: String,
    pub last_used_timestamp: u64,
    pub idle_timeout_secs: u64,
}

pub struct ConnectionPool {
    connections: Arc<Mutex<HashMap<String, SshConnection>>>,
    idle_timeout: Duration,
}

impl ConnectionPool {
    pub fn new(idle_timeout: Duration) -> Self {
        let connections: Arc<Mutex<HashMap<String, SshConnection>>> = Arc::new(Mutex::new(HashMap::new()));
        
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
                    if now.duration_since(conn.last_used) > idle_timeout {
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
                        let elapsed = now.duration_since(conn.last_used);
                        let last_used_unix = now_unix.saturating_sub(elapsed.as_secs());
                        ConnectionStatus {
                            host: host.clone(),
                            last_used_timestamp: last_used_unix,
                            idle_timeout_secs: idle_timeout.as_secs(),
                        }
                    })
                    .collect();
                if let Ok(file) = std::fs::File::create("/Users/richard/projects/rust/agentic_ssh/pool_status.json") {
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
                let elapsed = now.duration_since(conn.last_used);
                let last_used_unix = now_unix.saturating_sub(elapsed.as_secs());
                ConnectionStatus {
                    host: host.clone(),
                    last_used_timestamp: last_used_unix,
                    idle_timeout_secs: self.idle_timeout.as_secs(),
                }
            })
            .collect();
        if let Ok(file) = std::fs::File::create("/Users/richard/projects/rust/agentic_ssh/pool_status.json") {
            let _ = serde_json::to_writer_pretty(file, &statuses);
        }
    }

    /// Gets or creates a connection to the specified host.
    pub async fn get_connection(&self, host: &str) -> Result<Arc<Handle<ClientHandler>>> {
        let mut map = self.connections.lock().await;
        
        // Check if we have an active connection that's still working
        if let Some(conn) = map.get_mut(host) {
            // Test if the connection is alive by checking if we can open a channel
            match conn.handle.channel_open_session().await {
                Ok(channel) => {
                    // It works! We can close this test channel immediately and return the handle.
                    let _ = channel.close();
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
        let mut handle = russh::client::connect(config, socket_addr, ClientHandler).await
            .context("Failed to connect via TCP/SSH")?;

        let mut authenticated = false;

        // Try SSH Agent first if available
        if let Ok(socket_path) = std::env::var("SSH_AUTH_SOCK") {
            eprintln!("SSH_AUTH_SOCK found at {:?}. Attempting agent authentication...", socket_path);
            match tokio::net::UnixStream::connect(&socket_path).await {
                Ok(stream) => {
                    let mut agent_client = russh_keys::agent::client::AgentClient::connect(stream);
                    match agent_client.request_identities().await {
                        Ok(identities) => {
                            eprintln!("Found {} keys in SSH agent", identities.len());
                            for identity in identities {
                                eprintln!("Trying agent key...");
                                let (ac, res) = handle.authenticate_future(user, identity, agent_client).await;
                                agent_client = ac;
                                match res {
                                    Ok(success) => {
                                        if success {
                                            eprintln!("Authentication succeeded for {} using SSH agent key", host);
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
            if let Ok(key) = russh_keys::load_secret_key(key_path, None) {
                match handle.authenticate_publickey(user, Arc::new(key)).await {
                    Ok(success) => {
                        if success {
                            eprintln!("Authentication succeeded for {} using {:?}", host, key_path);
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
    pub async fn execute_command(&self, host: &str, command: &str) -> Result<(String, String, u32)> {
        let handle = self.get_connection(host).await?;

        // Update last used timestamp
        {
            let mut map = self.connections.lock().await;
            if let Some(conn) = map.get_mut(host) {
                conn.last_used = Instant::now();
            }
        }
        self.save_status().await;

        eprintln!("Executing command on {}: {:?}", host, command);
        let mut channel = handle.channel_open_session().await
            .context("Failed to open SSH channel")?;
        
        channel.exec(true, command).await
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

