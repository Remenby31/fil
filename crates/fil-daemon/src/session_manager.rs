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
    /// The fil proxy process, used to map an agent's process tree to a session.
    pub proxy_pid: Option<i32>,
}

pub enum ProxyCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    ClientAttached,
    ClientDetached,
}

impl std::fmt::Debug for ProxyCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(data) => f.debug_struct("Input").field("bytes", &data.len()).finish(),
            Self::Resize { cols, rows } => f
                .debug_struct("Resize")
                .field("cols", cols)
                .field("rows", rows)
                .finish(),
            Self::ClientAttached => f.write_str("ClientAttached"),
            Self::ClientDetached => f.write_str("ClientDetached"),
        }
    }
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

    pub fn contains(&self, session_id: &str) -> bool {
        self.sessions.read().unwrap().contains_key(session_id)
    }

    /// The first pid in `pids` (nearest ancestor first) that is a fil proxy.
    pub fn session_for_proxy_pids(&self, pids: &[i32]) -> Option<String> {
        let sessions = self.sessions.read().unwrap();
        pids.iter().find_map(|pid| {
            sessions
                .values()
                .find(|session| session.proxy_pid == Some(*pid))
                .map(|session| session.session_id.clone())
        })
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

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn nearest_proxy_ancestor_wins() {
        let manager = SessionManager::new();
        for (id, pid) in [("outer", 10), ("inner", 20)] {
            manager.add(ProxySession {
                session_id: id.into(),
                shell: "zsh".into(),
                command: "zsh".into(),
                cwd: "/".into(),
                created_at: 0,
                cols: 80,
                rows: 24,
                proxy_pid: Some(pid),
            });
        }
        assert_eq!(
            manager
                .session_for_proxy_pids(&[99, 21, 20, 11, 10])
                .as_deref(),
            Some("inner")
        );
        assert_eq!(manager.session_for_proxy_pids(&[99, 1]), None);
        assert!(manager.contains("outer"));
        assert!(!manager.contains("gone"));
    }

    #[test]
    fn command_diagnostics_do_not_disclose_terminal_input() {
        let diagnostic = format!("{:?}", ProxyCommand::Input(b"secret-password".to_vec()));
        assert!(!diagnostic.contains("secret-password"));
        assert!(
            !diagnostic.contains("115, 101, 99"),
            "raw input bytes leaked"
        );
        assert_eq!(diagnostic, "Input { bytes: 15 }");
    }
}
