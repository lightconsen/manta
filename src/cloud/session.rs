//! Cloud session: OAuth login URL building + session token storage.

use crate::cloud::config::CloudConfig;
use crate::secrets::{choose_store, SecretId, SecretOrigin};

pub const CLOUD_NS: &str = "cloud";
pub const ENTITY_SESSION: &str = "session";

fn token_id() -> SecretId {
    SecretId::secret(CLOUD_NS, ENTITY_SESSION)
}

/// The stored cloud session token, if any.
pub async fn get_token() -> Option<String> {
    let value: Option<String> = choose_store(&token_id())
        .get(&token_id())
        .await
        .ok()
        .flatten();
    value
}

/// Persist a session token (keyring-preferred for user-entered secrets).
pub async fn set_token(token: &str) -> crate::Result<()> {
    choose_store(&token_id())
        .set(&token_id(), token, SecretOrigin::UserEntered)
        .await
}

/// Forget the session token (revoked / signed out).
pub async fn clear_token() -> crate::Result<()> {
    choose_store(&token_id()).delete(&token_id()).await
}

/// Whether a session token is stored.
pub async fn logged_in() -> bool {
    get_token().await.is_some()
}

/// Cloud OAuth login URL for a provider. After auth the cloud redirects back
/// to `config.redirect_base#token=...`.
pub fn login_url(cfg: &CloudConfig, provider: &str) -> String {
    let redirect = format!("{}{}", cfg.redirect_base, "");
    format!("{}/auth/{provider}?redirect={}", cfg.api_base, urlencoding(&redirect))
}

/// Minimal percent-encoding for the `redirect` query param.
fn urlencoding(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
