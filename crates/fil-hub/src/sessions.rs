use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    pub device_id: String,
    pub shell: String,
    pub command: String,
    pub cwd: String,
    pub cols: u32,
    pub rows: u32,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Online,
    Unreachable,
    Offline,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeviceState {
    pub device_id: String,
    pub device_name: String,
    pub user_id: String,
    pub sessions: Vec<SessionInfo>,
    pub last_heartbeat: DateTime<Utc>,
    pub connected: bool,
}

#[derive(Debug, Clone)]
pub struct UserSessionUpdate {
    pub user_id: String,
    pub devices: Vec<DeviceState>,
}

#[derive(Clone)]
pub struct SessionRegistry {
    devices: Arc<RwLock<HashMap<String, DeviceState>>>,
    // Globally reserve IDs to authenticated devices, including offline/omitted
    // sessions. Always lock devices before owners; claims and mutations are atomic.
    session_owners: Arc<RwLock<HashMap<String, String>>>,
    updates: broadcast::Sender<UserSessionUpdate>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        let (updates, _) = broadcast::channel(256);
        Self {
            devices: Arc::new(RwLock::new(HashMap::new())),
            session_owners: Arc::new(RwLock::new(HashMap::new())),
            updates,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UserSessionUpdate> {
        self.updates.subscribe()
    }

    pub fn register_device(&self, device_id: &str, user_id: &str, device_name: &str) {
        {
            let mut devices = self.devices.write().unwrap();
            let device = devices
                .entry(device_id.to_string())
                .or_insert_with(|| DeviceState {
                    device_id: device_id.to_string(),
                    device_name: device_name.to_string(),
                    user_id: user_id.to_string(),
                    sessions: Vec::new(),
                    last_heartbeat: Utc::now(),
                    connected: true,
                });
            device.device_name = device_name.to_string();
            device.user_id = user_id.to_string();
            device.connected = true;
            device.last_heartbeat = Utc::now();
        }
        self.publish(user_id);
    }

    pub fn add_session(&self, device_id: &str, mut session: SessionInfo) {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return;
            };
            let mut owners = self.session_owners.write().unwrap();
            if !claim_session(&mut owners, device_id, &session.session_id) {
                return;
            }
            session.device_id = device_id.to_string();
            if let Some(existing) = device
                .sessions
                .iter_mut()
                .find(|existing| existing.session_id == session.session_id)
            {
                *existing = session;
            } else {
                device.sessions.push(session);
            }
            device.user_id.clone()
        };
        self.publish(&user_id);
    }

    /// Release only this device's reservation, even if a heartbeat omitted it.
    /// The result authorizes the caller to clean up the global data route.
    pub fn remove_session(&self, device_id: &str, session_id: &str) -> bool {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return false;
            };
            let mut owners = self.session_owners.write().unwrap();
            if owners
                .get(session_id)
                .is_none_or(|owner| owner != device_id)
            {
                return false;
            }
            owners.remove(session_id);
            device
                .sessions
                .retain(|session| session.session_id != session_id);
            device.user_id.clone()
        };
        self.publish(&user_id);
        true
    }

    /// Return only sessions previously listed by this device and now absent.
    /// Their routing state can be forgotten, but their IDs remain reserved.
    pub fn update_heartbeat(&self, device_id: &str, mut sessions: Vec<SessionInfo>) -> Vec<String> {
        let (changed_user, removed) = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return Vec::new();
            };
            let mut owners = self.session_owners.write().unwrap();
            let mut seen = HashSet::new();
            sessions.retain_mut(|session| {
                if !claim_session(&mut owners, device_id, &session.session_id)
                    || !seen.insert(session.session_id.clone())
                {
                    return false;
                }
                session.device_id = device_id.to_string();
                session.status = SessionStatus::Online;
                true
            });
            let removed = device
                .sessions
                .iter()
                .filter(|session| !seen.contains(&session.session_id))
                .map(|session| session.session_id.clone())
                .collect();
            let changed = !device.connected || device.sessions != sessions;
            device.connected = true;
            device.last_heartbeat = Utc::now();
            device.sessions = sessions;
            device.sessions.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.session_id.cmp(&right.session_id))
            });
            (changed.then(|| device.user_id.clone()), removed)
        };
        if let Some(user_id) = changed_user {
            self.publish(&user_id);
        }
        removed
    }

    /// Resize from the daemon control plane, scoped to its authenticated device.
    pub fn update_device_session_size(
        &self,
        device_id: &str,
        session_id: &str,
        cols: u32,
        rows: u32,
    ) -> bool {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return false;
            };
            let Some(session) = device
                .sessions
                .iter_mut()
                .find(|session| session.session_id == session_id)
            else {
                return false;
            };
            if session.cols == cols && session.rows == rows {
                return true;
            }
            session.cols = cols;
            session.rows = rows;
            device.user_id.clone()
        };
        self.publish(&user_id);
        true
    }

    pub fn update_session_size(&self, session_id: &str, cols: u32, rows: u32) {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let mut changed_user = None;
            for device in devices.values_mut() {
                if let Some(session) = device
                    .sessions
                    .iter_mut()
                    .find(|session| session.session_id == session_id)
                {
                    if session.cols != cols || session.rows != rows {
                        session.cols = cols;
                        session.rows = rows;
                        changed_user = Some(device.user_id.clone());
                    }
                    break;
                }
            }
            changed_user
        };
        if let Some(user_id) = user_id {
            self.publish(&user_id);
        }
    }

    pub fn get_user_sessions(&self, user_id: &str) -> Vec<DeviceState> {
        let devices = self.devices.read().unwrap();
        let mut result: Vec<_> = devices
            .values()
            .filter(|device| device.user_id == user_id)
            .cloned()
            .collect();
        for device in &mut result {
            device.sessions.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.session_id.cmp(&right.session_id))
            });
        }
        result.sort_by(|left, right| left.device_name.cmp(&right.device_name));
        result
    }

    pub fn owns_session(&self, user_id: &str, session_id: &str) -> bool {
        self.get_user_sessions(user_id).iter().any(|device| {
            device
                .sessions
                .iter()
                .any(|session| session.session_id == session_id)
        })
    }

    pub fn set_device_connected(&self, device_id: &str, connected: bool) {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return;
            };
            if device.connected == connected {
                return;
            }
            device.connected = connected;
            for session in &mut device.sessions {
                session.status = if connected {
                    SessionStatus::Online
                } else {
                    SessionStatus::Unreachable
                };
            }
            device.user_id.clone()
        };
        self.publish(&user_id);
    }

    /// Mark every device whose last heartbeat is older than `max_age` as
    /// disconnected, and return how many changed.
    ///
    /// `last_heartbeat` was written in three places and compared in none, so a
    /// Mac that slept, lost Wi-Fi or was force-quit stayed "Online" forever and
    /// the app happily offered to attach to sessions that no longer existed.
    /// Measured before: a device silent for 1205 ms still reported connected.
    pub fn reap_stale_devices(&self, max_age: chrono::Duration) -> usize {
        let now = Utc::now();
        let affected_users: Vec<String> = {
            let mut devices = self.devices.write().unwrap();
            let mut users = Vec::new();
            for device in devices.values_mut() {
                if !device.connected {
                    continue;
                }
                if now.signed_duration_since(device.last_heartbeat) <= max_age {
                    continue;
                }
                device.connected = false;
                for session in &mut device.sessions {
                    session.status = SessionStatus::Offline;
                }
                users.push(device.user_id.clone());
            }
            users
        };

        let count = affected_users.len();
        let mut seen: Vec<&String> = Vec::new();
        for user_id in &affected_users {
            if seen.contains(&user_id) {
                continue;
            }
            seen.push(user_id);
            self.publish(user_id);
        }
        count
    }

    pub fn remove_user(&self, user_id: &str) {
        {
            let mut devices = self.devices.write().unwrap();
            devices.retain(|_, device| device.user_id != user_id);
            self.session_owners
                .write()
                .unwrap()
                .retain(|_, device_id| devices.contains_key(device_id));
        }
        let _ = self.updates.send(UserSessionUpdate {
            user_id: user_id.to_string(),
            devices: Vec::new(),
        });
    }

    fn publish(&self, user_id: &str) {
        let _ = self.updates.send(UserSessionUpdate {
            user_id: user_id.to_string(),
            devices: self.get_user_sessions(user_id),
        });
    }
}

fn claim_session(owners: &mut HashMap<String, String>, device_id: &str, session_id: &str) -> bool {
    match owners.get(session_id) {
        Some(owner) => owner == device_id,
        None => {
            owners.insert(session_id.to_string(), device_id.to_string());
            true
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn session(device_id: &str, session_id: &str) -> SessionInfo {
        SessionInfo {
            session_id: session_id.into(),
            device_id: device_id.into(),
            shell: "sh".into(),
            command: String::new(),
            cwd: "/tmp".into(),
            cols: 80,
            rows: 24,
            status: SessionStatus::Online,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn session_created_cannot_claim_another_devices_session_id() {
        for other_user in ["owner", "attacker"] {
            let registry = SessionRegistry::new();
            registry.register_device("original", "owner", "Original");
            registry.register_device("other", other_user, "Other");
            registry.add_session("original", session("original", "shared"));
            registry.add_session("other", session("other", "shared"));
            assert!(
                registry.devices.read().unwrap()["other"]
                    .sessions
                    .is_empty()
            );
            assert!(registry.owns_session("owner", "shared"));
            assert!(!registry.owns_session("attacker", "shared"));
        }
    }

    #[test]
    fn heartbeat_cannot_claim_another_devices_session_id() {
        for other_user in ["owner", "attacker"] {
            let registry = SessionRegistry::new();
            registry.register_device("original", "owner", "Original");
            registry.register_device("other", other_user, "Other");
            registry.update_heartbeat("original", vec![session("original", "shared")]);
            registry.update_heartbeat(
                "other",
                vec![session("other", "shared"), session("other", "fresh")],
            );
            let devices = registry.devices.read().unwrap();
            assert_eq!(devices["other"].sessions.len(), 1);
            assert_eq!(devices["other"].sessions[0].session_id, "fresh");
            drop(devices);
            assert!(!registry.owns_session("attacker", "shared"));
        }
    }

    #[test]
    fn offline_and_omitted_ids_remain_reserved_until_owner_explicitly_removes_them() {
        let registry = SessionRegistry::new();
        registry.register_device("original", "owner", "Original");
        registry.register_device("other", "attacker", "Other");
        registry.add_session("original", session("original", "shared"));
        registry.set_device_connected("original", false);
        registry.add_session("other", session("other", "shared"));
        assert!(!registry.owns_session("attacker", "shared"));
        registry.register_device("original", "owner", "Reconnected");
        registry.update_heartbeat("original", vec![]);
        registry.remove_session("other", "shared");
        registry.update_heartbeat("other", vec![session("other", "shared")]);
        assert!(!registry.owns_session("attacker", "shared"));
        registry.remove_session("original", "shared");
        registry.add_session("other", session("other", "shared"));
        assert!(registry.owns_session("attacker", "shared"));
    }

    #[test]
    fn original_device_can_reconnect_refresh_and_restore_its_session() {
        let registry = SessionRegistry::new();
        registry.register_device("original", "owner", "Original");
        registry.add_session("original", session("original", "shared"));
        registry.set_device_connected("original", false);
        registry.register_device("original", "owner", "Reconnected");
        let mut updated = session("original", "shared");
        updated.cols = 120;
        registry.add_session("original", updated.clone());
        registry.update_heartbeat("original", vec![]);
        registry.update_heartbeat("original", vec![updated]);
        let devices = registry.get_user_sessions("owner");
        assert_eq!(devices[0].sessions.len(), 1);
        assert_eq!(devices[0].sessions[0].cols, 120);
        assert!(registry.owns_session("owner", "shared"));
    }

    #[test]
    fn removing_account_releases_even_omitted_session_reservations() {
        let registry = SessionRegistry::new();
        registry.register_device("original", "owner", "Original");
        registry.register_device("other", "attacker", "Other");
        registry.add_session("original", session("original", "shared"));
        registry.update_heartbeat("original", vec![]);
        registry.remove_user("owner");
        registry.update_heartbeat("other", vec![session("other", "shared")]);
        assert!(registry.owns_session("attacker", "shared"));
    }

    #[test]
    fn destroy_and_resize_require_the_exact_device_not_just_the_user() {
        for other_user in ["owner", "attacker"] {
            let registry = SessionRegistry::new();
            registry.register_device("original", "owner", "Original");
            registry.register_device("other", other_user, "Other");
            registry.add_session("original", session("original", "shared"));
            assert!(!registry.remove_session("other", "shared"));
            assert!(!registry.update_device_session_size("other", "shared", 1, 1));
            assert_eq!(
                registry.devices.read().unwrap()["original"].sessions[0].cols,
                80
            );
            assert!(registry.update_device_session_size("original", "shared", 120, 40));
            assert_eq!(
                registry.devices.read().unwrap()["original"].sessions[0].cols,
                120
            );
            assert!(registry.remove_session("original", "shared"));
            assert!(!registry.remove_session("original", "shared"));
        }
    }

    #[test]
    fn heartbeat_cleanup_returns_only_its_own_removed_ids() {
        let registry = SessionRegistry::new();
        registry.register_device("original", "owner", "Original");
        registry.register_device("other", "attacker", "Other");
        registry.add_session("original", session("original", "victim"));
        registry.add_session("other", session("other", "old"));
        let removed = registry.update_heartbeat(
            "other",
            vec![
                session("original", "victim"),
                session("spoofed-device", "fresh"),
                session("other", "fresh"),
            ],
        );
        assert_eq!(removed, ["old"]);
        let devices = registry.devices.read().unwrap();
        assert_eq!(devices["other"].sessions.len(), 1);
        assert_eq!(devices["other"].sessions[0].device_id, "other");
        drop(devices);
        assert!(
            registry
                .update_heartbeat("missing", vec![session("missing", "new")])
                .is_empty()
        );
        registry.add_session("original", session("original", "old"));
        assert!(!registry.owns_session("owner", "old"));
    }

    #[test]
    fn simultaneous_devices_cannot_both_claim_the_same_id() {
        let registry = SessionRegistry::new();
        registry.register_device("original", "owner", "Original");
        registry.register_device("other", "attacker", "Other");
        let barrier = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            for device in ["original", "other"] {
                let registry = &registry;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    registry.update_heartbeat(device, vec![session(device, "shared")]);
                });
            }
        });
        assert_ne!(
            registry.owns_session("owner", "shared"),
            registry.owns_session("attacker", "shared")
        );
    }
}

#[cfg(test)]
mod reaper_tests {
    use super::*;

    #[test]
    fn a_device_that_stops_heartbeating_is_marked_offline() {
        let registry = SessionRegistry::new();
        registry.register_device("dev-1", "user-1", "Mac");
        assert!(registry.get_user_sessions("user-1")[0].connected);

        // Backdate the heartbeat rather than sleeping.
        {
            let mut devices = registry.devices.write().unwrap();
            let device = devices.get_mut("dev-1").unwrap();
            device.last_heartbeat = Utc::now() - chrono::Duration::seconds(60);
        }

        let reaped = registry.reap_stale_devices(chrono::Duration::seconds(20));

        assert_eq!(reaped, 1);
        assert!(
            !registry.get_user_sessions("user-1")[0].connected,
            "a device silent past the deadline must not still advertise as online"
        );
    }

    #[test]
    fn a_live_device_is_left_alone() {
        let registry = SessionRegistry::new();
        registry.register_device("dev-2", "user-2", "Mac");

        let reaped = registry.reap_stale_devices(chrono::Duration::seconds(20));

        assert_eq!(reaped, 0);
        assert!(registry.get_user_sessions("user-2")[0].connected);
    }
}
