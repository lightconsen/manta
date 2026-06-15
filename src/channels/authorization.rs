//! Command authorization building blocks.
//!
//! Provides owner state tracking, sender candidate matching, and provider
//! inference helpers used by the channel command gate.

use crate::channels::identity::SenderIdentity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Verified owner lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerState {
    /// No ownership claim has been verified.
    Unverified,
    /// Ownership verified.
    Verified,
    /// Ownership temporarily delegated to another user.
    Delegated,
    /// Ownership explicitly revoked.
    Revoked,
}

impl Default for OwnerState {
    fn default() -> Self {
        OwnerState::Unverified
    }
}

/// In-memory owner state store keyed by user ID.
#[derive(Debug, Clone, Default)]
pub struct OwnerStore {
    states: Arc<RwLock<HashMap<String, OwnerState>>>,
}

impl OwnerStore {
    /// Create an empty owner store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the current state for a user.
    pub fn get(&self, user_id: &str) -> OwnerState {
        let states = self.states.read().expect("OwnerStore lock poisoned");
        states.get(user_id).copied().unwrap_or_default()
    }

    /// Set the state for a user directly.
    pub fn set(&self, user_id: impl Into<String>, state: OwnerState) {
        let mut states = self.states.write().expect("OwnerStore lock poisoned");
        states.insert(user_id.into(), state);
    }

    /// Transition between states with validation.
    pub fn transition(
        &self,
        user_id: impl Into<String>,
        new_state: OwnerState,
    ) -> Result<(), OwnerTransitionError> {
        let user_id = user_id.into();
        let mut states = self.states.write().expect("OwnerStore lock poisoned");
        let current = states.get(&user_id).copied().unwrap_or_default();

        let allowed = match (current, new_state) {
            // From Unverified you can verify, delegate, or revoke.
            (OwnerState::Unverified, _) => true,
            // From Verified you can delegate or revoke.
            (OwnerState::Verified, OwnerState::Delegated)
            | (OwnerState::Verified, OwnerState::Revoked) => true,
            // From Delegated you can return to Verified or Revoke.
            (OwnerState::Delegated, OwnerState::Verified)
            | (OwnerState::Delegated, OwnerState::Revoked) => true,
            // From Revoked only re-verification is allowed.
            (OwnerState::Revoked, OwnerState::Verified) => true,
            // Self-transitions and anything else is allowed as a no-op refresh.
            (a, b) if a == b => true,
            _ => false,
        };

        if !allowed {
            return Err(OwnerTransitionError {
                current,
                requested: new_state,
            });
        }

        states.insert(user_id, new_state);
        Ok(())
    }

    /// Return true if the user is a verified owner (not revoked/unverified).
    pub fn is_owner(&self, user_id: &str) -> bool {
        matches!(self.get(user_id), OwnerState::Verified | OwnerState::Delegated)
    }
}

/// Error returned when an owner state transition is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerTransitionError {
    pub current: OwnerState,
    pub requested: OwnerState,
}

impl std::fmt::Display for OwnerTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Cannot transition owner state from {:?} to {:?}",
            self.current, self.requested
        )
    }
}

impl std::error::Error for OwnerTransitionError {}

/// A candidate identity match with a confidence score.
#[derive(Debug, Clone, PartialEq)]
pub struct SenderCandidate {
    pub identity: SenderIdentity,
    /// Confidence between 0.0 and 1.0.
    pub confidence: f64,
}

impl SenderCandidate {
    /// Create a candidate from a known identity with perfect confidence.
    pub fn exact(identity: SenderIdentity) -> Self {
        Self {
            identity,
            confidence: 1.0,
        }
    }

    /// Create a candidate with a confidence score clamped to [0, 1].
    pub fn with_confidence(identity: SenderIdentity, confidence: f64) -> Self {
        Self {
            identity,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

/// Match an incoming sender identity against a registry of known identities.
#[derive(Debug, Clone, Default)]
pub struct CandidateMatcher {
    known: Vec<SenderIdentity>,
}

impl CandidateMatcher {
    /// Create a matcher with known identities.
    pub fn new(known: Vec<SenderIdentity>) -> Self {
        Self { known }
    }

    /// Find the best matching candidate for `incoming`.
    pub fn match_candidates(
        &self,
        incoming: &SenderIdentity,
    ) -> Option<SenderCandidate> {
        let mut best: Option<SenderCandidate> = None;

        for known in &self.known {
            let confidence = Self::score(incoming, known);
            if confidence > 0.0 {
                let candidate = SenderCandidate::with_confidence(known.clone(), confidence);
                if best.as_ref().map_or(true, |b| candidate.confidence > b.confidence) {
                    best = Some(candidate);
                }
            }
        }

        // If no known identity matches, treat the incoming identity itself as
        // a candidate with low confidence.
        best.or_else(|| Some(SenderCandidate::with_confidence(incoming.clone(), 0.1)))
    }

    /// Score similarity between two identities (0.0 = no match, 1.0 = exact).
    fn score(a: &SenderIdentity, b: &SenderIdentity) -> f64 {
        if a.user_id == b.user_id {
            return 1.0;
        }

        let mut hits = 0;
        let mut checks = 0;

        if a.username.is_some() || b.username.is_some() {
            checks += 1;
            if a.username == b.username {
                hits += 1;
            }
        }
        if a.phone.is_some() || b.phone.is_some() {
            checks += 1;
            if a.phone == b.phone {
                hits += 1;
            }
        }
        if a.email.is_some() || b.email.is_some() {
            checks += 1;
            if a.email == b.email {
                hits += 1;
            }
        }

        if checks == 0 {
            0.0
        } else {
            (hits as f64 / checks as f64) * 0.8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_state_transition_verify() {
        let store = OwnerStore::new();
        assert!(store.transition("u1", OwnerState::Verified).is_ok());
        assert!(store.is_owner("u1"));
    }

    #[test]
    fn test_owner_state_transition_invalid() {
        let store = OwnerStore::new();
        store.set("u1", OwnerState::Revoked);
        assert!(store.transition("u1", OwnerState::Delegated).is_err());
    }

    #[test]
    fn test_owner_state_reverify_after_revoke() {
        let store = OwnerStore::new();
        store.set("u1", OwnerState::Revoked);
        assert!(store.transition("u1", OwnerState::Verified).is_ok());
        assert!(store.is_owner("u1"));
    }

    #[test]
    fn test_candidate_exact_match() {
        let known = SenderIdentity::new("u1").with_username("alice");
        let matcher = CandidateMatcher::new(vec![known.clone()]);
        let incoming = SenderIdentity::new("u1");
        let candidate = matcher.match_candidates(&incoming).unwrap();
        assert_eq!(candidate.confidence, 1.0);
        assert_eq!(candidate.identity.user_id, "u1");
    }

    #[test]
    fn test_candidate_username_match() {
        let known = SenderIdentity::new("u1").with_username("alice");
        let matcher = CandidateMatcher::new(vec![known]);
        let incoming = SenderIdentity::new("u2").with_username("alice");
        let candidate = matcher.match_candidates(&incoming).unwrap();
        assert!(candidate.confidence > 0.0 && candidate.confidence < 1.0);
    }

    #[test]
    fn test_candidate_no_match_falls_back() {
        let matcher = CandidateMatcher::new(vec![]);
        let incoming = SenderIdentity::new("u1");
        let candidate = matcher.match_candidates(&incoming).unwrap();
        assert_eq!(candidate.identity.user_id, "u1");
        assert_eq!(candidate.confidence, 0.1);
    }
}
