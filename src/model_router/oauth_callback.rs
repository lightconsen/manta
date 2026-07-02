//! Local HTTP callback server for OAuth 2.0 redirects
//!
//! Binds to a localhost port, waits for the provider's redirect, and extracts
//! the `code` and `state` query parameters.
//!
//! ```rust,ignore
//! let code = wait_for_callback(18081, 300, "expected-state").await?;
//! ```

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, warn};

/// Start a temporary HTTP server on `port`, wait for a single GET request
/// to `/callback`, extract `code` and `state`, verify `state` matches
/// `expected_state`, then shut down.
///
/// Returns `Err` if the timeout expires, the request is malformed, or the
/// returned `state` does not match the expected value.
pub async fn wait_for_callback(
    port: u16,
    timeout_secs: u64,
    expected_state: &str,
) -> crate::Result<String> {
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

            // Read the full HTTP request line (and headers) instead of peeking a
            // fixed 4KB buffer, so long callback URLs are not truncated.
            let request = read_request_line(&mut stream, 64 * 1024).await?;

            debug!("Callback request: {}", request.lines().next().unwrap_or("(empty)"));

            // Extract code and state from the request line
            let (code, state) = parse_callback_request(&request)?;

            if state != expected_state {
                return Err(crate::error::SyscityError::ExternalService {
                    source: format!(
                        "OAuth callback state mismatch: expected '{}', got '{}'",
                        expected_state, state
                    ),
                    cause: None,
                });
            }

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

            Ok::<_, crate::error::SyscityError>(code)
        };

    match timeout(Duration::from_secs(timeout_secs), accept_future).await {
        Ok(Ok(code)) => {
            info!("Received OAuth callback with matching state");
            Ok(code)
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

/// Read the HTTP request line and headers from `stream` until the header
/// terminator (`\r\n\r\n`) is seen or `max_len` bytes have been read.
async fn read_request_line(
    stream: &mut tokio::net::TcpStream,
    max_len: usize,
) -> crate::Result<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    loop {
        let n = stream.read(&mut chunk).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: format!("Failed to read callback request: {}", e),
                cause: Some(Box::new(e)),
            }
        })?;

        if n == 0 {
            break;
        }

        buf.extend_from_slice(&chunk[..n]);

        if buf.len() > max_len {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("OAuth callback request exceeded {} byte limit", max_len),
                cause: None,
            });
        }

        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    String::from_utf8(buf).map_err(|e| crate::error::SyscityError::ExternalService {
        source: format!("OAuth callback request is not valid UTF-8: {}", e),
        cause: None,
    })
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

    #[test]
    fn test_parse_callback_long_url() {
        let long_state = "a".repeat(5000);
        let req = format!(
            "GET /callback?code=longcode&state={} HTTP/1.1\r\n",
            urlencoding::encode(&long_state)
        );
        let (code, state) = parse_callback_request(&req).unwrap();
        assert_eq!(code, "longcode");
        assert_eq!(state, long_state);
    }

    #[tokio::test]
    async fn test_wait_for_callback_state_mismatch() {
        let port = 18099u16;
        let expected = "expected-state";
        let wrong = "wrong-state";

        let server = tokio::spawn(async move { wait_for_callback(port, 5, expected).await });

        // Give the server a moment to bind.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let request = format!(
            "GET /callback?code=abc123&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            wrong
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let result = server.await.unwrap();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("state mismatch"), "error should mention state mismatch: {}", err);
    }
}
