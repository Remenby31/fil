use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
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
    updates: broadcast::Sender<UserSessionUpdate>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        let (updates, _) = broadcast::channel(256);
        Self {
            devices: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn add_session(&self, device_id: &str, session: SessionInfo) {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return;
            };
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

    pub fn remove_session(&self, device_id: &str, session_id: &str) {
        let user_id = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return;
            };
            let previous_len = device.sessions.len();
            device
                .sessions
                .retain(|session| session.session_id != session_id);
            if previous_len == device.sessions.len() {
                return;
            }
            device.user_id.clone()
        };
        self.publish(&user_id);
    }

    pub fn update_heartbeat(&self, device_id: &str, mut sessions: Vec<SessionInfo>) {
        let changed_user = {
            let mut devices = self.devices.write().unwrap();
            let Some(device) = devices.get_mut(device_id) else {
                return;
            };
            for session in &mut sessions {
                session.status = SessionStatus::Online;
            }
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
            changed.then(|| device.user_id.clone())
        };
        if let Some(user_id) = changed_user {
            self.publish(&user_id);
        }
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
                    session.status = SessionStatus::Unreachable;
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
