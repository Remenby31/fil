use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub device_id: String,
    pub shell: String,
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

#[derive(Debug, Clone, Serialize)]
pub struct DeviceState {
    pub device_id: String,
    pub device_name: String,
    pub user_id: String,
    pub sessions: Vec<SessionInfo>,
    pub last_heartbeat: DateTime<Utc>,
    pub connected: bool,
}

#[derive(Clone)]
pub struct SessionRegistry {
    devices: Arc<RwLock<HashMap<String, DeviceState>>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            devices: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_device(&self, device_id: &str, user_id: &str, device_name: &str) {
        let mut devices = self.devices.write().unwrap();
        devices.insert(
            device_id.to_string(),
            DeviceState {
                device_id: device_id.to_string(),
                device_name: device_name.to_string(),
                user_id: user_id.to_string(),
                sessions: Vec::new(),
                last_heartbeat: Utc::now(),
                connected: true,
            },
        );
    }

    pub fn unregister_device(&self, device_id: &str) {
        let mut devices = self.devices.write().unwrap();
        devices.remove(device_id);
    }

    pub fn add_session(&self, device_id: &str, session: SessionInfo) {
        let mut devices = self.devices.write().unwrap();
        if let Some(device) = devices.get_mut(device_id) {
            device.sessions.push(session);
        }
    }

    pub fn remove_session(&self, device_id: &str, session_id: &str) {
        let mut devices = self.devices.write().unwrap();
        if let Some(device) = devices.get_mut(device_id) {
            device.sessions.retain(|s| s.session_id != session_id);
        }
    }

    pub fn update_heartbeat(&self, device_id: &str, sessions: Vec<SessionInfo>) {
        let mut devices = self.devices.write().unwrap();
        if let Some(device) = devices.get_mut(device_id) {
            device.last_heartbeat = Utc::now();
            device.sessions = sessions;
        }
    }

    pub fn get_user_sessions(&self, user_id: &str) -> Vec<DeviceState> {
        let devices = self.devices.read().unwrap();
        devices
            .values()
            .filter(|d| d.user_id == user_id)
            .cloned()
            .collect()
    }

    pub fn set_device_connected(&self, device_id: &str, connected: bool) {
        let mut devices = self.devices.write().unwrap();
        if let Some(device) = devices.get_mut(device_id) {
            device.connected = connected;
            let status = if connected {
                SessionStatus::Online
            } else {
                SessionStatus::Unreachable
            };
            for session in &mut device.sessions {
                session.status = status.clone();
            }
        }
    }
}
