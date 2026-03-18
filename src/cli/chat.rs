//! Chat and web interface commands for Manta

use crate::config::Config;
use crate::error::Result;

/// Chat with the AI assistant
pub async fn run_chat(
    _config: &Config,
    _conversation: Option<String>,
    _message: Option<String>,
) -> Result<()> {
    // TODO: Move implementation from cli.rs
    println!("Chat command...");
    Ok(())
}

/// Start web terminal interface
pub async fn run_web(_config: &Config, _port: u16) -> Result<()> {
    // TODO: Move implementation from cli.rs
    println!("Starting web interface...");
    Ok(())
}
