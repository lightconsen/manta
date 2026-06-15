//! Authentication configuration for the TUI WebSocket connection.

use crate::gateway::protocol::AuthMode;

/// Auth mode and credentials for the TUI client.
#[derive(Debug, Clone)]
pub enum AuthConfig {
    /// No authentication (development / local desktop mode).
    None,
    /// Shared secret token passed as a query parameter.
    Token {
        /// The shared token.
        token: String,
    },
}

impl AuthConfig {
    /// Build an `AuthConfig` from an optional token.
    pub fn from_token(token: Option<&str>) -> Self {
        match token {
            Some(t) if !t.is_empty() => Self::Token { token: t.to_string() },
            _ => Self::None,
        }
    }

    /// Build the WebSocket URL, appending the token query parameter when needed.
    pub fn ws_url(&self, host: &str, port: u16, session_id: Option<&str>, client: &str) -> String {
        let mut url = format!("ws://{}:{}/ws", host, port);
        let mut first = true;

        let mut append = |key: &str, value: &str| {
            let sep = if first {
                first = false;
                "?"
            } else {
                "&"
            };
            url.push_str(sep);
            url.push_str(key);
            url.push('=');
            url.push_str(&urlencoding::encode(value));
        };

        if let Self::Token { token } = self {
            append("token", token);
        }
        if let Some(sid) = session_id {
            append("session_id", sid);
        }
        if !client.is_empty() {
            append("client", client);
        }

        url
    }

    /// Build the HTTP base URL.
    #[allow(dead_code)]
    pub fn http_url(&self, host: &str, port: u16) -> String {
        format!("http://{}:{}", host, port)
    }

    /// Return the configured auth mode for protocol handshake.
    #[allow(dead_code)]
    pub fn auth_mode(&self) -> AuthMode {
        match self {
            Self::None => AuthMode::None,
            Self::Token { .. } => AuthMode::Token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_without_auth() {
        let auth = AuthConfig::None;
        assert_eq!(
            auth.ws_url("127.0.0.1", 18080, None, "tui"),
            "ws://127.0.0.1:18080/ws?client=tui"
        );
    }

    #[test]
    fn ws_url_with_token_and_session() {
        let auth = AuthConfig::Token {
            token: "secret token".to_string(),
        };
        assert_eq!(
            auth.ws_url("127.0.0.1", 18080, Some("sess-1"), "tui"),
            "ws://127.0.0.1:18080/ws?token=secret%20token&session_id=sess-1&client=tui"
        );
    }
}
