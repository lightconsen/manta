//! Local HTTP callback server for OAuth 2.0 redirects
//!
//! Binds to a localhost port, waits for the provider's redirect, and extracts
//! the `code` and `state` query parameters.
//!
//! ```rust,ignore
//! let (code, state) = wait_for_callback(18081, 300).await?;
//! ```

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, warn};

/// Start a temporary HTTP server on `port`, wait for a single GET request
/// to `/callback`, extract `code` and `state`, then shut down.
///
/// Returns `Err` if the timeout expires or the request is malformed.
pub async fn wait_for_callback(port: u16, timeout_secs: u64) -> crate::Result<(String, String)> {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await.map_err(|e| {
        crate::error::SyscityError::ExternalService {
            source: format!("Failed to bind callback server to {}: {}", addr, e),
            cause: Some(Box::new(e)),
        }
    })?;

    info!("Waiting for OAuth callback on http://{}/callback", addr);

    let accept_future =
        async {
            let (mut stream, peer) = listener.accept().await.map_err(|e| {
                crate::error::SyscityError::ExternalService {
                    source: format!("Failed to accept callback connection: {}", e),
                    cause: Some(Box::new(e)),
                }
            })?;

            debug!("OAuth callback connection from {:?}", peer);

            // Read the first HTTP request line
            let mut buf = [0u8; 4096];
            let n = stream.peek(&mut buf).await.map_err(|e| {
                crate::error::SyscityError::ExternalService {
                    source: format!("Failed to read callback request: {}", e),
                    cause: Some(Box::new(e)),
                }
            })?;

            let request = String::from_utf8_lossy(&buf[..n]);
            debug!("Callback request: {}", request.lines().next().unwrap_or("(empty)"));

            // Extract code and state from the request line
            let (code, state) = parse_callback_request(&request)?;

            // Send a simple success response and close
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: \
                            close\r\n\r\n<html><body><h2>Authorization successful</h2><p>You can \
                            close this window.</p></body></html>";
            stream
                .write_all(response.as_bytes())
                .await
                .unwrap_or_else(|e| warn!("Failed to send OAuth response: {}", e));
            stream
                .shutdown()
                .await
                .unwrap_or_else(|e| warn!("Failed to shutdown OAuth stream: {}", e));

            Ok::<_, crate::error::SyscityError>((code, state))
        };

    match timeout(Duration::from_secs(timeout_secs), accept_future).await {
        Ok(Ok(result)) => {
            info!("Received OAuth callback with state={}", result.1);
            Ok(result)
        }
        Ok(Err(e)) => {
            error!("OAuth callback processing failed: {}", e);
            Err(e)
        }
        Err(_) => {
            warn!("OAuth callback timed out after {}s", timeout_secs);
            Err(crate::error::SyscityError::ExternalService {
                source: format!(
                    "OAuth callback timed out after {} seconds — no redirect received",
                    timeout_secs
                ),
                cause: None,
            })
        }
    }
}

fn parse_callback_request(request: &str) -> crate::Result<(String, String)> {
    // Find the request line: GET /callback?code=xxx&state=yyy HTTP/1.1
    let line =
        request
            .lines()
            .next()
            .ok_or_else(|| crate::error::SyscityError::ExternalService {
                source: "OAuth callback request is empty".to_string(),
                cause: None,
            })?;

    let path_part = line.split_whitespace().nth(1).ok_or_else(|| {
        crate::error::SyscityError::ExternalService {
            source: "OAuth callback request has no path".to_string(),
            cause: None,
        }
    })?;

    // Extract query string after '?'
    let query = path_part
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or(path_part);

    let mut code = None;
    let mut state = None;

    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            let decoded = urlencoding::decode(value).unwrap_or_else(|_| value.into());
            match key {
                "code" => code = Some(decoded.to_string()),
                "state" => state = Some(decoded.to_string()),
                "error" => {
                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!("OAuth authorization error: {}", decoded),
                        cause: None,
                    })
                }
                _ => {}
            }
        }
    }

    let code = code.ok_or_else(|| crate::error::SyscityError::ExternalService {
        source: "OAuth callback missing 'code' parameter".to_string(),
        cause: None,
    })?;

    let state = state.ok_or_else(|| crate::error::SyscityError::ExternalService {
        source: "OAuth callback missing 'state' parameter".to_string(),
        cause: None,
    })?;

    Ok((code, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callback_request() {
        let req = "GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: localhost\r\n";
        let (code, state) = parse_callback_request(req).unwrap();
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz");
    }

    #[test]
    fn test_parse_callback_with_urlencoding() {
        let req = "GET /callback?code=abc%2B123&state=x%2Fy HTTP/1.1\r\n";
        let (code, state) = parse_callback_request(req).unwrap();
        assert_eq!(code, "abc+123");
        assert_eq!(state, "x/y");
    }

    #[test]
    fn test_parse_callback_missing_code() {
        let req = "GET /callback?state=xyz HTTP/1.1\r\n";
        assert!(parse_callback_request(req).is_err());
    }

    #[test]
    fn test_parse_callback_error() {
        let req = "GET /callback?error=access_denied&state=xyz HTTP/1.1\r\n";
        assert!(parse_callback_request(req).is_err());
    }
}
