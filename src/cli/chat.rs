//! Chat and web interface commands for Manta

use crate::config::Config;
use crate::error::{MantaError, Result};

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

/// Chat with the AI assistant
pub async fn run_chat(
    _config: &Config,
    conversation: Option<String>,
    message: Option<String>,
) -> Result<()> {
    let client = reqwest::Client::new();
    let session_id = conversation.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if let Some(msg) = message {
        // Single message mode
        send_message(&client, &session_id, &msg).await?;
    } else {
        // Interactive REPL mode
        println!("Manta chat (session: {})", session_id);
        println!("Type your message and press Enter. Type 'exit' or Ctrl-C to quit.");
        println!();

        let stdin = tokio::io::stdin();
        let reader = tokio::io::BufReader::new(stdin);
        use tokio::io::AsyncBufReadExt;
        let mut lines = reader.lines();

        loop {
            print!("> ");
            use std::io::Write;
            std::io::stdout().flush().ok();

            match lines.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed == "exit" || trimmed == "quit" {
                        break;
                    }
                    send_message(&client, &session_id, &trimmed).await?;
                }
                Ok(None) | Err(_) => break,
            }
        }
    }

    Ok(())
}

/// Send a single message and print the response
async fn send_message(client: &reqwest::Client, session_id: &str, message: &str) -> Result<()> {
    let url = format!("{}/api/chat", DAEMON_URL);
    let body = serde_json::json!({
        "session_id": session_id,
        "message": message,
    });

    match client.post(&url).json(&body).send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                // Try to parse JSON response
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(content) = json.get("response").or_else(|| json.get("content")) {
                        println!("{}", content.as_str().unwrap_or(&text));
                    } else {
                        println!("{}", text);
                    }
                } else {
                    println!("{}", text);
                }
            } else {
                eprintln!("Error ({}): {}", status, text);
            }
        }
        Err(e) => {
            eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
            eprintln!("Is the daemon running? Try: manta start");
            return Err(MantaError::Internal(e.to_string()));
        }
    }
    Ok(())
}

/// Start web terminal interface
pub async fn run_web(_config: &Config, port: u16) -> Result<()> {
    println!("Web terminal interface on port {}", port);
    println!(
        "The daemon's built-in web UI is available at http://127.0.0.1:{}/",
        port
    );
    println!(
        "Start the daemon with: manta start --web-port {}",
        port
    );
    Ok(())
}
