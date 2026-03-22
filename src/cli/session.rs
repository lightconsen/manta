//! Session management CLI commands for Manta
//!
//! Provides introspection and control over the Session → Thread → Turn
//! hierarchy stored in the running daemon.

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum SessionCommands {
    /// List all sessions
    List {
        /// Show only active sessions
        #[arg(short, long)]
        active: bool,
    },
    /// List threads within a session
    Threads {
        /// Session ID
        session_id: String,
    },
    /// List turns within a thread
    Turns {
        /// Session ID
        session_id: String,
        /// Thread ID
        thread_id: String,
    },
    /// Undo the last turn of a thread
    Undo {
        /// Session ID
        session_id: String,
        /// Thread ID
        thread_id: String,
    },
}

/// Run session commands
pub async fn run_session_command(command: &SessionCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        SessionCommands::List { active } => {
            let mut url = format!("{}/api/sessions", DAEMON_URL);
            if *active {
                url.push_str("?active=true");
            }
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("{}", body);
                    } else {
                        eprintln!("Error {}: {}", status, body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
                    eprintln!("Is the daemon running? Try: manta start");
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }

        SessionCommands::Threads { session_id } => {
            let url = format!("{}/api/sessions/{}/threads", DAEMON_URL, session_id);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("{}", body);
                    } else {
                        eprintln!("Error {}: {}", status, body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }

        SessionCommands::Turns {
            session_id,
            thread_id,
        } => {
            let url = format!(
                "{}/api/sessions/{}/threads/{}/turns",
                DAEMON_URL, session_id, thread_id
            );
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("{}", body);
                    } else {
                        eprintln!("Error {}: {}", status, body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }

        SessionCommands::Undo {
            session_id,
            thread_id,
        } => {
            let url = format!(
                "{}/api/sessions/{}/threads/{}/undo",
                DAEMON_URL, session_id, thread_id
            );
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Undo successful.");
                        if !body.is_empty() {
                            println!("{}", body);
                        }
                    } else {
                        eprintln!("Undo failed ({}): {}", status, body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
    }

    Ok(())
}
