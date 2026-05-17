//! OpenClaw-Aligned Group Session Support
//!
//! Member role awareness and group-level session scoping.
//! Inspired by OpenClaw's `group.ts`.
//!
//! Features:
//! - Member roles (Owner, Admin, Member, Observer)
//! - Group-level session scoping
//! - Member management (add, remove, list)
//! - Role-based permissions for session operations

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Role of a member within a group session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupRole {
    /// Full control over the session.
    Owner,
    /// Can manage members and moderate.
    Admin,
    /// Regular participant.
    Member,
    /// Read-only observer.
    Observer,
}

impl std::fmt::Display for GroupRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupRole::Owner => write!(f, "owner"),
            GroupRole::Admin => write!(f, "admin"),
            GroupRole::Member => write!(f, "member"),
            GroupRole::Observer => write!(f, "observer"),
        }
    }
}

impl GroupRole {
    /// Check if this role can add/remove members.
    pub fn can_manage_members(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin)
    }

    /// Check if this role can terminate the session.
    pub fn can_terminate_session(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin)
    }

    /// Check if this role can spawn agents.
    pub fn can_spawn_agents(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin | GroupRole::Member)
    }

    /// Check if this role can send messages (not read-only).
    pub fn can_participate(&self) -> bool {
        matches!(self, GroupRole::Owner | GroupRole::Admin | GroupRole::Member)
    }
}

/// A member of a group session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupMember {
    /// User ID (peer).
    pub user_id: String,
    /// Display name.
    pub display_name: String,
    /// Role in the group.
    pub role: GroupRole,
    /// When the member joined.
    pub joined_at: chrono::DateTime<chrono::Utc>,
    /// Whether the member is currently active.
    pub is_active: bool,
}

impl GroupMember {
    /// Create a new group member.
    pub fn new(
        user_id: impl Into<String>,
        display_name: impl Into<String>,
        role: GroupRole,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            display_name: display_name.into(),
            role,
            joined_at: chrono::Utc::now(),
            is_active: true,
        }
    }

    /// Mark the member as active.
    pub fn mark_active(&mut self) {
        self.is_active = true;
    }

    /// Mark the member as inactive.
    pub fn mark_inactive(&mut self) {
        self.is_active = false;
    }
}

/// Group-level session with member management.
#[derive(Debug)]
pub struct GroupSession {
    /// Session ID.
    pub id: String,
    /// Group name.
    pub name: String,
    /// Members indexed by user_id.
    members: HashMap<String, GroupMember>,
    /// Owner user ID.
    pub owner_id: String,
    /// When the group was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last activity time.
    pub last_activity: std::time::Instant,
    /// Whether the group is archived.
    pub is_archived: bool,
    /// Optional metadata (topic, description, etc.).
    pub metadata: Option<serde_json::Value>,
}

impl GroupSession {
    /// Create a new group session.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        owner_id: impl Into<String>,
        owner_name: impl Into<String>,
    ) -> Self {
        let id = id.into();
        let owner_id = owner_id.into();
        let mut members = HashMap::new();
        members.insert(
            owner_id.clone(),
            GroupMember::new(owner_id.clone(), owner_name, GroupRole::Owner),
        );

        Self {
            id,
            name: name.into(),
            members,
            owner_id,
            created_at: chrono::Utc::now(),
            last_activity: std::time::Instant::now(),
            is_archived: false,
            metadata: None,
        }
    }

    /// Add a member to the group.
    pub fn add_member(
        &mut self,
        user_id: impl Into<String>,
        display_name: impl Into<String>,
        role: GroupRole,
    ) -> Result<(), GroupSessionError> {
        let user_id = user_id.into();
        if self.members.contains_key(&user_id) {
            return Err(GroupSessionError::MemberAlreadyExists(user_id));
        }

        self.members
            .insert(user_id.clone(), GroupMember::new(user_id, display_name, role));
        self.last_activity = std::time::Instant::now();
        debug!("Added member to group session {}", self.id);
        Ok(())
    }

    /// Remove a member from the group.
    pub fn remove_member(&mut self, user_id: &str) -> Result<(), GroupSessionError> {
        if user_id == self.owner_id {
            return Err(GroupSessionError::CannotRemoveOwner);
        }

        if self.members.remove(user_id).is_some() {
            self.last_activity = std::time::Instant::now();
            debug!("Removed member {} from group session {}", user_id, self.id);
            Ok(())
        } else {
            Err(GroupSessionError::MemberNotFound(user_id.to_string()))
        }
    }

    /// Update a member's role.
    pub fn update_member_role(
        &mut self,
        user_id: &str,
        new_role: GroupRole,
    ) -> Result<(), GroupSessionError> {
        if user_id == self.owner_id && new_role != GroupRole::Owner {
            return Err(GroupSessionError::CannotDemoteOwner);
        }

        let member = self
            .members
            .get_mut(user_id)
            .ok_or_else(|| GroupSessionError::MemberNotFound(user_id.to_string()))?;

        member.role = new_role;
        self.last_activity = std::time::Instant::now();
        Ok(())
    }

    /// Get a member by user ID.
    pub fn get_member(&self, user_id: &str) -> Option<&GroupMember> {
        self.members.get(user_id)
    }

    /// Get mutable member reference.
    pub fn get_member_mut(&mut self, user_id: &str) -> Option<&mut GroupMember> {
        self.members.get_mut(user_id)
    }

    /// Get all members.
    pub fn get_members(&self) -> Vec<&GroupMember> {
        self.members.values().collect()
    }

    /// Get active members.
    pub fn get_active_members(&self) -> Vec<&GroupMember> {
        self.members.values().filter(|m| m.is_active).collect()
    }

    /// Get members by role.
    pub fn get_members_by_role(&self, role: GroupRole) -> Vec<&GroupMember> {
        self.members.values().filter(|m| m.role == role).collect()
    }

    /// Check if a user is a member.
    pub fn is_member(&self, user_id: &str) -> bool {
        self.members.contains_key(user_id)
    }

    /// Check if a user has at least the given role.
    pub fn has_role(&self, user_id: &str, min_role: GroupRole) -> bool {
        self.members
            .get(user_id)
            .map(|m| role_level(m.role) >= role_level(min_role))
            .unwrap_or(false)
    }

    /// Get the number of members.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Archive the group session.
    pub fn archive(&mut self) {
        self.is_archived = true;
        info!("Archived group session {}", self.id);
    }

    /// Check if the session has timed out.
    pub fn is_timed_out(&self, timeout: std::time::Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }
}

/// Helper: role hierarchy level (higher = more privileged).
fn role_level(role: GroupRole) -> u8 {
    match role {
        GroupRole::Owner => 4,
        GroupRole::Admin => 3,
        GroupRole::Member => 2,
        GroupRole::Observer => 1,
    }
}

/// Errors from group session operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum GroupSessionError {
    #[error("Member already exists: {0}")]
    MemberAlreadyExists(String),
    #[error("Member not found: {0}")]
    MemberNotFound(String),
    #[error("Cannot remove the owner from the group")]
    CannotRemoveOwner,
    #[error("Cannot demote the owner")]
    CannotDemoteOwner,
    #[error("Insufficient permissions")]
    InsufficientPermissions,
}

/// Manager for all group sessions.
#[derive(Debug, Default)]
pub struct GroupSessionManager {
    /// Active group sessions.
    groups: HashMap<String, Arc<RwLock<GroupSession>>>,
    /// User ID -> list of group IDs they belong to.
    user_index: HashMap<String, Vec<String>>,
    /// Session timeout.
    timeout: std::time::Duration,
}

impl GroupSessionManager {
    /// Create a new group session manager.
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
            user_index: HashMap::new(),
            timeout: std::time::Duration::from_secs(3600 * 24), // 24 hours default
        }
    }

    /// Create a new group session.
    pub fn create_group(
        &mut self,
        id: impl Into<String>,
        name: impl Into<String>,
        owner_id: impl Into<String>,
        owner_name: impl Into<String>,
    ) -> Arc<RwLock<GroupSession>> {
        let id = id.into();
        let owner_id = owner_id.into();
        let group = GroupSession::new(id.clone(), name, owner_id.clone(), owner_name);
        let arc = Arc::new(RwLock::new(group));

        self.groups.insert(id.clone(), Arc::clone(&arc));
        self.user_index
            .entry(owner_id)
            .or_default()
            .push(id.clone());

        info!("Created group session {}", id);
        arc
    }

    /// Get a group session by ID.
    pub fn get_group(&self, group_id: &str) -> Option<Arc<RwLock<GroupSession>>> {
        self.groups.get(group_id).cloned()
    }

    /// List all group IDs.
    pub fn list_groups(&self) -> Vec<String> {
        self.groups.keys().cloned().collect()
    }

    /// Get groups for a user.
    pub fn get_user_groups(&self, user_id: &str) -> Vec<String> {
        self.user_index.get(user_id).cloned().unwrap_or_default()
    }

    /// Remove a group session.
    pub async fn remove_group(&mut self, group_id: &str) {
        if let Some(group) = self.groups.remove(group_id) {
            let members: Vec<String> = {
                let g = group.read().await;
                g.members.keys().cloned().collect()
            };
            for user_id in members {
                if let Some(groups) = self.user_index.get_mut(&user_id) {
                    groups.retain(|g| g != group_id);
                }
            }
            info!("Removed group session {}", group_id);
        }
    }

    /// Add a member to a group (with user index update).
    pub async fn add_member(
        &mut self,
        group_id: &str,
        user_id: impl Into<String>,
        display_name: impl Into<String>,
        role: GroupRole,
    ) -> Result<(), GroupSessionError> {
        let user_id = user_id.into();
        let group = self
            .groups
            .get(group_id)
            .ok_or_else(|| GroupSessionError::MemberNotFound(group_id.to_string()))?;

        {
            let mut g = group.write().await;
            g.add_member(user_id.clone(), display_name, role)?;
        }

        self.user_index
            .entry(user_id)
            .or_default()
            .push(group_id.to_string());

        Ok(())
    }

    /// Remove a member from a group (with user index update).
    pub async fn remove_member(
        &mut self,
        group_id: &str,
        user_id: &str,
    ) -> Result<(), GroupSessionError> {
        let group = self
            .groups
            .get(group_id)
            .ok_or_else(|| GroupSessionError::MemberNotFound(group_id.to_string()))?;

        {
            let mut g = group.write().await;
            g.remove_member(user_id)?;
        }

        if let Some(groups) = self.user_index.get_mut(user_id) {
            groups.retain(|g| g != group_id);
        }

        Ok(())
    }

    /// Cleanup timed-out groups.
    pub async fn cleanup_timed_out(&mut self) {
        let timed_out: Vec<String> = {
            let mut ids = Vec::new();
            for (id, group) in &self.groups {
                if group.read().await.is_timed_out(self.timeout) {
                    ids.push(id.clone());
                }
            }
            ids
        };

        for id in timed_out {
            info!("Group session '{}' timed out, removing", id);
            self.remove_group(&id).await;
        }
    }

    /// Set session timeout.
    pub fn set_timeout(&mut self, timeout: std::time::Duration) {
        self.timeout = timeout;
    }

    /// Get global stats.
    pub fn stats(&self) -> GroupManagerStats {
        GroupManagerStats {
            group_count: self.groups.len(),
            total_members: self.user_index.len(),
        }
    }
}

/// Stats for the group session manager.
#[derive(Debug, Clone)]
pub struct GroupManagerStats {
    pub group_count: usize,
    pub total_members: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_role_permissions() {
        assert!(GroupRole::Owner.can_manage_members());
        assert!(GroupRole::Admin.can_manage_members());
        assert!(!GroupRole::Member.can_manage_members());
        assert!(!GroupRole::Observer.can_participate());
    }

    #[test]
    fn test_group_session_member_management() {
        let mut group = GroupSession::new("g1", "Test Group", "user1", "Alice");

        assert_eq!(group.member_count(), 1);
        assert!(group.is_member("user1"));

        group.add_member("user2", "Bob", GroupRole::Member).unwrap();
        assert_eq!(group.member_count(), 2);

        let member = group.get_member("user2").unwrap();
        assert_eq!(member.role, GroupRole::Member);

        group.update_member_role("user2", GroupRole::Admin).unwrap();
        let member = group.get_member("user2").unwrap();
        assert_eq!(member.role, GroupRole::Admin);

        group.remove_member("user2").unwrap();
        assert_eq!(group.member_count(), 1);
    }

    #[test]
    fn test_cannot_remove_owner() {
        let mut group = GroupSession::new("g1", "Test Group", "user1", "Alice");
        let result = group.remove_member("user1");
        assert!(result.is_err());
    }

    #[test]
    fn test_role_level_check() {
        let mut group = GroupSession::new("g1", "Test Group", "user1", "Alice");
        group.add_member("user2", "Bob", GroupRole::Admin).unwrap();
        group
            .add_member("user3", "Charlie", GroupRole::Member)
            .unwrap();

        assert!(group.has_role("user1", GroupRole::Admin));
        assert!(group.has_role("user2", GroupRole::Member));
        assert!(!group.has_role("user3", GroupRole::Admin));
    }

    #[test]
    fn test_group_role_all_permissions() {
        assert!(GroupRole::Owner.can_terminate_session());
        assert!(GroupRole::Admin.can_terminate_session());
        assert!(!GroupRole::Member.can_terminate_session());
        assert!(!GroupRole::Observer.can_terminate_session());

        assert!(GroupRole::Owner.can_spawn_agents());
        assert!(GroupRole::Admin.can_spawn_agents());
        assert!(GroupRole::Member.can_spawn_agents());
        assert!(!GroupRole::Observer.can_spawn_agents());

        assert!(GroupRole::Owner.can_participate());
        assert!(GroupRole::Admin.can_participate());
        assert!(GroupRole::Member.can_participate());
        assert!(!GroupRole::Observer.can_participate());
    }

    #[test]
    fn test_group_role_display() {
        assert_eq!(GroupRole::Owner.to_string(), "owner");
        assert_eq!(GroupRole::Admin.to_string(), "admin");
        assert_eq!(GroupRole::Member.to_string(), "member");
        assert_eq!(GroupRole::Observer.to_string(), "observer");
    }

    #[test]
    fn test_group_member_lifecycle() {
        let mut member = GroupMember::new("u1", "Alice", GroupRole::Member);
        assert_eq!(member.user_id, "u1");
        assert_eq!(member.display_name, "Alice");
        assert_eq!(member.role, GroupRole::Member);
        assert!(member.is_active);

        member.mark_inactive();
        assert!(!member.is_active);

        member.mark_active();
        assert!(member.is_active);
    }

    #[test]
    fn test_group_session_new_has_owner() {
        let group = GroupSession::new("g1", "Test", "user1", "Alice");
        assert_eq!(group.member_count(), 1);
        assert_eq!(group.owner_id, "user1");
        assert!(group.is_member("user1"));
        let owner = group.get_member("user1").unwrap();
        assert_eq!(owner.role, GroupRole::Owner);
    }

    #[test]
    fn test_group_session_add_duplicate_member() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        group.add_member("user2", "Bob", GroupRole::Member).unwrap();
        let result = group.add_member("user2", "Bob2", GroupRole::Admin);
        assert!(matches!(result, Err(GroupSessionError::MemberAlreadyExists(_))));
    }

    #[test]
    fn test_group_session_remove_not_found() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        let result = group.remove_member("nonexistent");
        assert!(matches!(result, Err(GroupSessionError::MemberNotFound(_))));
    }

    #[test]
    fn test_group_session_cannot_demote_owner() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        let result = group.update_member_role("user1", GroupRole::Admin);
        assert!(matches!(result, Err(GroupSessionError::CannotDemoteOwner)));
    }

    #[test]
    fn test_group_session_update_role_not_found() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        let result = group.update_member_role("nonexistent", GroupRole::Admin);
        assert!(matches!(result, Err(GroupSessionError::MemberNotFound(_))));
    }

    #[test]
    fn test_group_session_get_members() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        group.add_member("user2", "Bob", GroupRole::Member).unwrap();
        group
            .add_member("user3", "Charlie", GroupRole::Observer)
            .unwrap();

        let members = group.get_members();
        assert_eq!(members.len(), 3);

        let active = group.get_active_members();
        assert_eq!(active.len(), 3);
    }

    #[test]
    fn test_group_session_get_members_by_role() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        group.add_member("user2", "Bob", GroupRole::Admin).unwrap();
        group
            .add_member("user3", "Charlie", GroupRole::Member)
            .unwrap();

        let admins = group.get_members_by_role(GroupRole::Admin);
        assert_eq!(admins.len(), 1);
        assert_eq!(admins[0].user_id, "user2");

        let members = group.get_members_by_role(GroupRole::Member);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].user_id, "user3");
    }

    #[test]
    fn test_group_session_is_member() {
        let group = GroupSession::new("g1", "Test", "user1", "Alice");
        assert!(group.is_member("user1"));
        assert!(!group.is_member("user2"));
    }

    #[test]
    fn test_group_session_has_role_non_member() {
        let group = GroupSession::new("g1", "Test", "user1", "Alice");
        assert!(!group.has_role("nonexistent", GroupRole::Observer));
    }

    #[test]
    fn test_group_session_archive() {
        let mut group = GroupSession::new("g1", "Test", "user1", "Alice");
        assert!(!group.is_archived);
        group.archive();
        assert!(group.is_archived);
    }

    #[tokio::test]
    async fn test_group_session_is_timed_out() {
        let group = GroupSession::new("g1", "Test", "user1", "Alice");
        assert!(!group.is_timed_out(std::time::Duration::from_secs(3600)));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(group.is_timed_out(std::time::Duration::from_secs(0)));
    }

    #[test]
    fn test_group_session_error_display() {
        assert_eq!(
            GroupSessionError::MemberAlreadyExists("u1".to_string()).to_string(),
            "Member already exists: u1"
        );
        assert_eq!(
            GroupSessionError::MemberNotFound("u1".to_string()).to_string(),
            "Member not found: u1"
        );
        assert_eq!(
            GroupSessionError::CannotRemoveOwner.to_string(),
            "Cannot remove the owner from the group"
        );
        assert_eq!(GroupSessionError::CannotDemoteOwner.to_string(), "Cannot demote the owner");
        assert_eq!(
            GroupSessionError::InsufficientPermissions.to_string(),
            "Insufficient permissions"
        );
    }

    #[test]
    fn test_group_manager_new() {
        let manager = GroupSessionManager::new();
        assert!(manager.list_groups().is_empty());
        assert!(manager.get_user_groups("any").is_empty());
    }

    #[test]
    fn test_group_manager_create_and_get() {
        let mut manager = GroupSessionManager::new();
        let _group = manager.create_group("g1", "Test", "user1", "Alice");
        assert_eq!(manager.list_groups(), vec!["g1"]);

        let retrieved = manager.get_group("g1");
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_group_manager_user_groups() {
        let mut manager = GroupSessionManager::new();
        manager.create_group("g1", "Test1", "user1", "Alice");
        manager.create_group("g2", "Test2", "user1", "Alice");
        manager.create_group("g3", "Test3", "user2", "Bob");

        let user1_groups = manager.get_user_groups("user1");
        assert_eq!(user1_groups.len(), 2);
        assert!(user1_groups.contains(&"g1".to_string()));
        assert!(user1_groups.contains(&"g2".to_string()));

        let user2_groups = manager.get_user_groups("user2");
        assert_eq!(user2_groups.len(), 1);
    }

    #[tokio::test]
    async fn test_group_manager_remove_group() {
        let mut manager = GroupSessionManager::new();
        manager.create_group("g1", "Test", "user1", "Alice");
        manager
            .add_member("g1", "user2", "Bob", GroupRole::Member)
            .await
            .unwrap();

        assert_eq!(manager.get_user_groups("user2").len(), 1);
        manager.remove_group("g1").await;
        assert!(manager.get_group("g1").is_none());
        assert!(manager.get_user_groups("user2").is_empty());
    }

    #[tokio::test]
    async fn test_group_manager_add_remove_member() {
        let mut manager = GroupSessionManager::new();
        manager.create_group("g1", "Test", "user1", "Alice");

        manager
            .add_member("g1", "user2", "Bob", GroupRole::Member)
            .await
            .unwrap();
        assert_eq!(manager.get_user_groups("user2").len(), 1);

        manager.remove_member("g1", "user2").await.unwrap();
        assert!(manager.get_user_groups("user2").is_empty());
    }

    #[test]
    fn test_group_manager_stats() {
        let mut manager = GroupSessionManager::new();
        manager.create_group("g1", "Test1", "user1", "Alice");
        manager.create_group("g2", "Test2", "user2", "Bob");

        let stats = manager.stats();
        assert_eq!(stats.group_count, 2);
        assert_eq!(stats.total_members, 2);
    }

    #[test]
    fn test_group_manager_set_timeout() {
        let mut manager = GroupSessionManager::new();
        manager.set_timeout(std::time::Duration::from_secs(60));
        // Just verify it doesn't panic
    }

    #[tokio::test]
    async fn test_group_manager_cleanup_timed_out() {
        let mut manager = GroupSessionManager::new();
        manager.set_timeout(std::time::Duration::from_secs(0));
        manager.create_group("g1", "Test", "user1", "Alice");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        manager.cleanup_timed_out().await;
        assert!(manager.get_group("g1").is_none());
    }

    #[test]
    fn test_role_level() {
        assert_eq!(role_level(GroupRole::Owner), 4);
        assert_eq!(role_level(GroupRole::Admin), 3);
        assert_eq!(role_level(GroupRole::Member), 2);
        assert_eq!(role_level(GroupRole::Observer), 1);
    }

    #[test]
    fn test_group_manager_default() {
        let manager: GroupSessionManager = Default::default();
        assert!(manager.list_groups().is_empty());
    }
}
