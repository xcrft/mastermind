//! Minimal SessionStore.

use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub user_id: String,
}

pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn insert(&self, session: Session) {
        self.sessions
            .write()
            .unwrap()
            .insert(session.id.clone(), session);
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        self.sessions.read().unwrap().get(id).cloned()
    }

    pub fn refresh(&self) -> Result<Session, String> {
        Err("not implemented".to_string())
    }

    /// Number of active sessions currently tracked.
    pub fn session_count(&self) -> usize {
        self.sessions.read().unwrap().len()
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn auth_refresh(store: &SessionStore) -> Result<Session, String> {
    store.refresh()
}

pub fn api_login_refresh(store: &SessionStore) -> Result<Session, String> {
    store.refresh()
}

pub fn middleware_refresh(store: &SessionStore) -> Result<Session, String> {
    store.refresh()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_count_returns_current_size() {
        let store = SessionStore::new();
        assert_eq!(store.session_count(), 0);
        store.insert(Session {
            id: "a".into(),
            user_id: "u1".into(),
        });
        assert_eq!(store.session_count(), 1);
    }
}
