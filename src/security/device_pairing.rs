//! Device Pairing for WebSocket-native protocol
//!
//! Flow:
//! 1. Client connects via WS and sends `connect` with `device` identity
//! 2. If device not paired: server generates a short pairing code
//! 3. Server broadcasts `device.pair.requested` event to admin clients
//! 4. Admin runs `syscity device approve <code>`
//! 5. Server issues a device token to the client
//! 6. Client reconnects using the device token in `auth.token`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// A pending device pairing request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDeviceRequest {
    /// Short pairing code (e.g., "A3F7K").
    pub code: String,
    /// Device unique ID.
    pub device_id: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Ed25519 public key (base64) if provided.
    pub public_key: Option<String>,
    /// When the request was created.
    pub created_at: SystemTime,
    /// When the request expires.
    pub expires_at: SystemTime,
}

impl PendingDeviceRequest {
    pub fn is_valid(&self) -> bool {
        SystemTime::now() < self.expires_at
    }
}

/// An authorized device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizedDevice {
    pub device_id: String,
    pub display_name: Option<String>,
    pub public_key: Option<String>,
    /// Device token used for re-authentication.
    pub token: String,
    pub authorized_at: SystemTime,
    pub approved_by: Option<String>,
}

/// Result of a device access check.
#[derive(Debug, Clone)]
pub enum DeviceAccessResult {
    /// Device is authorized; here is the token.
    Authorized { token: String },
    /// New pairing request created.
    PairingRequired { code: String },
    /// Already has a pending request.
    AlreadyPending { code: String },
    /// Rate limited.
    RateLimited,
}

/// Thread-safe store for device pairing.
#[derive(Debug, Clone)]
pub struct DevicePairingStore {
    pending: Arc<RwLock<HashMap<String, PendingDeviceRequest>>>,
    /// device_id -> code reverse index.
    pending_index: Arc<RwLock<HashMap<String, String>>>,
    authorized: Arc<RwLock<HashMap<String, AuthorizedDevice>>>,
    default_ttl: Duration,
}

impl Default for DevicePairingStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DevicePairingStore {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            pending_index: Arc::new(RwLock::new(HashMap::new())),
            authorized: Arc::new(RwLock::new(HashMap::new())),
            default_ttl: Duration::from_secs(3600),
        }
    }

    /// Request device pairing access.
    pub async fn request_access(
        &self,
        device_id: &str,
        display_name: Option<&str>,
        public_key: Option<&str>,
    ) -> DeviceAccessResult {
        // Check if already authorized
        {
            let auth = self.authorized.read().await;
            if let Some(dev) = auth.get(device_id) {
                return DeviceAccessResult::Authorized { token: dev.token.clone() };
            }
        }

        // Check existing pending
        {
            let index = self.pending_index.read().await;
            if let Some(code) = index.get(device_id) {
                let pending = self.pending.read().await;
                if let Some(req) = pending.get(code) {
                    if req.is_valid() {
                        return DeviceAccessResult::AlreadyPending { code: code.clone() };
                    }
                }
            }
        }

        // Generate short code
        let code = Self::generate_code();
        let now = SystemTime::now();
        let req = PendingDeviceRequest {
            code: code.clone(),
            device_id: device_id.to_string(),
            display_name: display_name.map(|s| s.to_string()),
            public_key: public_key.map(|s| s.to_string()),
            created_at: now,
            expires_at: now + self.default_ttl,
        };

        {
            let mut pending = self.pending.write().await;
            pending.insert(code.clone(), req);
        }
        {
            let mut index = self.pending_index.write().await;
            index.insert(device_id.to_string(), code.clone());
        }

        info!("Device pairing request created: device_id={} code={}", device_id, code);

        DeviceAccessResult::PairingRequired { code }
    }

    /// Approve a pending device by code. Returns the device token on success.
    pub async fn approve(&self, code: &str, approved_by: Option<&str>) -> Option<String> {
        let req = {
            let mut pending = self.pending.write().await;
            pending.remove(code)?
        };

        if !req.is_valid() {
            warn!("Attempted to approve expired device pairing code: {}", code);
            // Clean up index
            let mut index = self.pending_index.write().await;
            index.remove(&req.device_id);
            return None;
        }

        let token = format!("dt_{}", uuid::Uuid::new_v4());
        let dev = AuthorizedDevice {
            device_id: req.device_id.clone(),
            display_name: req.display_name.clone(),
            public_key: req.public_key.clone(),
            token: token.clone(),
            authorized_at: SystemTime::now(),
            approved_by: approved_by.map(|s| s.to_string()),
        };

        {
            let mut auth = self.authorized.write().await;
            auth.insert(req.device_id.clone(), dev);
        }
        {
            let mut index = self.pending_index.write().await;
            index.remove(&req.device_id);
        }

        info!(
            "Device approved: device_id={} code={} by={:?}",
            req.device_id, code, approved_by
        );

        Some(token)
    }

    /// Revoke an authorized device.
    pub async fn revoke(&self, device_id: &str) -> bool {
        let mut auth = self.authorized.write().await;
        auth.remove(device_id).is_some()
    }

    /// Check if a device token is valid.
    pub async fn validate_token(&self, token: &str) -> Option<String> {
        let auth = self.authorized.read().await;
        for (device_id, dev) in auth.iter() {
            if dev.token == token {
                return Some(device_id.clone());
            }
        }
        None
    }

    /// Get device info by ID.
    pub async fn get_device(&self, device_id: &str) -> Option<AuthorizedDevice> {
        let auth = self.authorized.read().await;
        auth.get(device_id).cloned()
    }

    /// List all authorized devices.
    pub async fn list_authorized(&self) -> Vec<AuthorizedDevice> {
        let auth = self.authorized.read().await;
        auth.values().cloned().collect()
    }

    /// List pending requests.
    pub async fn list_pending(&self) -> Vec<PendingDeviceRequest> {
        let pending = self.pending.read().await;
        pending.values().cloned().collect()
    }

    fn generate_code() -> String {
        use rand::Rng;
        const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
        let mut rng = rand::thread_rng();
        (0..5)
            .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_device_pairing_flow() {
        let store = DevicePairingStore::new();

        // Request access
        let result = store.request_access("dev_1", Some("My Laptop"), None).await;
        let code = match result {
            DeviceAccessResult::PairingRequired { code } => code,
            _ => panic!("Expected PairingRequired"),
        };
        assert_eq!(code.len(), 5);

        // Same device returns same pending code
        let result2 = store.request_access("dev_1", None, None).await;
        assert!(
            matches!(result2, DeviceAccessResult::AlreadyPending { code: ref c } if c == &code)
        );

        // Approve
        let token = store.approve(&code, Some("admin")).await;
        assert!(token.is_some());
        let token = token.unwrap();
        assert!(token.starts_with("dt_"));

        // Now authorized
        let result3 = store.request_access("dev_1", None, None).await;
        assert!(matches!(result3, DeviceAccessResult::Authorized { token: ref t } if t == &token));

        // Validate token
        let validated = store.validate_token(&token).await;
        assert_eq!(validated, Some("dev_1".to_string()));

        // Revoke
        assert!(store.revoke("dev_1").await);
        assert!(matches!(
            store.request_access("dev_1", None, None).await,
            DeviceAccessResult::PairingRequired { .. }
        ));
    }
}
