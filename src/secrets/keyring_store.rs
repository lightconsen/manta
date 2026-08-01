//! Tier 1 backend — OS keyring.
//!
//! Each `(namespace, entity)` maps to a single keyring entry:
//!
//! - service: `syscity/{namespace}`
//! - account: `{entity}`
//! - value: JSON-serialized map of kind → value
//!
//! The keyring stores opaque byte strings, so the per-entity map is flattened
//! into a single JSON string. This mirrors the file fallback (`FileStore`),
//! where an entity is one TOML file whose `[secrets]` table is the same
//! kind → value map — both backends therefore expose identical semantics.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::error::SyscityError;
use crate::secrets::store::{SecretId, SecretOrigin, SecretStore};

/// Tier 1 backend bound to a namespace (`syscity/{namespace}` service).
#[derive(Debug, Clone)]
pub struct KeyringStore {
    namespace: String,
}

impl KeyringStore {
    /// Create a keyring backend bound to a namespace.
    pub fn new(namespace: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
        }
    }

    /// Keyring service name: `syscity/{namespace}`.
    pub fn service(&self) -> String {
        format!("syscity/{}", self.namespace)
    }

    /// Read an entity's whole map; a missing entry yields an empty map.
    fn get_entry(&self, entity: &str) -> crate::Result<HashMap<String, String>> {
        let entry = keyring_entry(&self.service(), entity)?;
        match entry.get_password() {
            Ok(raw) => decode_value(&raw),
            Err(keyring::Error::NoEntry) => Ok(HashMap::new()),
            Err(e) => Err(SyscityError::Internal(format!(
                "keyring read error ({} / {}): {e}",
                self.service(),
                entity
            ))),
        }
    }

    /// Overwrite an entity's whole map (an empty map deletes the entry).
    fn set_entry(&self, entity: &str, map: &HashMap<String, String>) -> crate::Result<()> {
        if map.is_empty() {
            return self.delete_entry(entity);
        }
        let entry = keyring_entry(&self.service(), entity)?;
        let raw = encode_value(map)?;
        entry.set_password(&raw).map_err(|e| {
            SyscityError::Internal(format!(
                "keyring write error ({} / {}): {e}",
                self.service(),
                entity
            ))
        })
    }

    /// Delete an entity's whole entry (missing is not an error).
    fn delete_entry(&self, entity: &str) -> crate::Result<()> {
        let entry = keyring_entry(&self.service(), entity)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SyscityError::Internal(format!(
                "keyring delete error ({} / {}): {e}",
                self.service(),
                entity
            ))),
        }
    }

    /// Whether an entity already has a keyring entry.
    fn has_entry(&self, entity: &str) -> bool {
        match keyring_entry(&self.service(), entity) {
            Ok(entry) => entry.get_password().is_ok(),
            Err(_) => false,
        }
    }
}

/// Build a keyring entry, mapping platform errors to a Syscity error.
fn keyring_entry(service: &str, user: &str) -> crate::Result<keyring::Entry> {
    keyring::Entry::new(service, user).map_err(|e| {
        SyscityError::Internal(format!("keyring entry unavailable ({service} / {user}): {e}"))
    })
}

/// Serialize a map into a single keyring value.
pub fn encode_value(map: &HashMap<String, String>) -> crate::Result<String> {
    serde_json::to_string(map)
        .map_err(|e| SyscityError::Internal(format!("keyring value serialization failed: {e}")))
}

/// Parse a keyring value back into a map.
pub fn decode_value(raw: &str) -> crate::Result<HashMap<String, String>> {
    serde_json::from_str(raw)
        .map_err(|e| SyscityError::Internal(format!("keyring value deserialization failed: {e}")))
}

#[async_trait::async_trait]
impl SecretStore for KeyringStore {
    async fn get(&self, id: &SecretId) -> crate::Result<Option<String>> {
        let map = self.get_entry(&id.entity)?;
        Ok(map.get(&id.kind).cloned())
    }

    async fn set(&self, id: &SecretId, value: &str, _origin: SecretOrigin) -> crate::Result<()> {
        let mut map = self.get_entry(&id.entity)?;
        map.insert(id.kind.clone(), value.to_string());
        self.set_entry(&id.entity, &map)
    }

    async fn delete(&self, id: &SecretId) -> crate::Result<()> {
        let mut map = self.get_entry(&id.entity)?;
        if map.remove(&id.kind).is_some() {
            if map.is_empty() {
                self.delete_entry(&id.entity)?;
            } else {
                self.set_entry(&id.entity, &map)?;
            }
        }
        Ok(())
    }

    async fn has(&self, id: &SecretId) -> bool {
        matches!(self.get(id).await, Ok(Some(_)))
    }

    async fn get_all(&self, entity: &str) -> crate::Result<HashMap<String, String>> {
        self.get_entry(entity)
    }

    async fn set_all(&self, entity: &str, map: &HashMap<String, String>) -> crate::Result<()> {
        self.set_entry(entity, map)
    }

    async fn delete_entity(&self, entity: &str) -> crate::Result<()> {
        self.delete_entry(entity)
    }

    async fn has_entity(&self, entity: &str) -> bool {
        self.has_entry(entity)
    }
}

/// Whether the OS keyring is usable. Cached per process.
///
/// Under `cfg(test)` this always returns `false` so tests never touch the real
/// user keychain. In production a throwaway entry is written and read back to
/// verify the backend actually works — headless Linux has no Secret Service
/// daemon, so the probe gracefully degrades to `false`.
pub fn probe_keyring() -> bool {
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(probe_keyring_uncached)
}

#[cfg(test)]
fn probe_keyring_uncached() -> bool {
    false
}

#[cfg(not(test))]
fn probe_keyring_uncached() -> bool {
    let entry = match keyring::Entry::new("syscity/probe", "probe") {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("Keyring probe failed to open entry: {e}");
            return false;
        }
    };
    // Clear any stale probe result, then verify a full write/read cycle.
    let _ = entry.delete_credential();
    if entry.set_password("probe-ok").is_err() {
        return false;
    }
    let ok = matches!(entry.get_password().as_deref(), Ok("probe-ok"));
    let _ = entry.delete_credential();
    ok
}

// ─────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_name() {
        let store = KeyringStore::new("mcp-env");
        assert_eq!(store.service(), "syscity/mcp-env");
    }

    #[test]
    fn test_value_roundtrip() {
        let mut map = HashMap::new();
        map.insert("refresh_token".to_string(), "rt_abc".to_string());
        map.insert("client_id".to_string(), "cid".to_string());
        let raw = encode_value(&map).unwrap();
        assert!(raw.contains("rt_abc"));
        let decoded = decode_value(&raw).unwrap();
        assert_eq!(decoded, map);
    }

    #[test]
    fn test_decode_rejects_garbage() {
        assert!(decode_value("not json").is_err());
        assert!(decode_value("").is_err());
    }

    #[test]
    fn test_probe_is_safe_under_test() {
        // Under cfg(test) the probe must never touch a real keychain.
        assert!(!probe_keyring());
    }
}
