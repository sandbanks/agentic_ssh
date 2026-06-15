use std::time::Duration;
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod mcp_server;
mod ssh_config;
mod ssh_pool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // We maintain a pool of open SSH connections, closing them after 5 minutes (300 seconds) of inactivity.
    let server = mcp_server::McpServer::new(Duration::from_secs(300));
    server.run().await?;
    Ok(())
}
