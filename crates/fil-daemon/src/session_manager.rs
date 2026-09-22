use std::collections::HashMap;
use std::sync::RwLock;

pub struct ProxySession {
    pub session_id: String,
    pub shell: String,
    pub command: String,
    pub cwd: String,
    pub created_at: i64,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Debug)]
pub enum ProxyCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    ClientAttached,
    ClientDetached,
}

pub struct SessionManager {
    sessions: RwLock<HashMap<String, ProxySession>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn add(&self, session: ProxySession) {
        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(session.session_id.clone(), session);
    }

    pub fn remove(&self, session_id: &str) {
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(session_id);
    }

    pub fn update_size(&self, session_id: &str, cols: u32, rows: u32) {
        let mut sessions = self.sessions.write().unwrap();
        if let Some(s) = sessions.get_mut(session_id) {
            s.cols = cols;
            s.rows = rows;
        }
    }

    pub fn update_metadata(&self, session_id: &str, cwd: String, command: String) {
        let mut sessions = self.sessions.write().unwrap();
        if let Some(session) = sessions.get_mut(session_id) {
            session.cwd = cwd;
            session.command = command;
        }
    }

    pub fn all_session_infos(&self) -> Vec<fil_protocol::proto::SessionInfo> {
        let sessions = self.sessions.read().unwrap();
        let mut infos: Vec<_> = sessions
            .values()
            .map(|s| fil_protocol::proto::SessionInfo {
                session_id: s.session_id.clone(),
                shell: s.shell.clone(),
                cwd: s.cwd.clone(),
                created_at: s.created_at,
                cols: s.cols,
                rows: s.rows,
                command: s.command.clone(),
            })
            .collect();
        infos.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        infos
    }
}
