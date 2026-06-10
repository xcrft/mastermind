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

    // Signature changed: now takes `force` argument.
    pub fn refresh(&self, force: bool) -> Result<Session, String> {
        let _ = force;
        Err("not implemented".to_string())
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn auth_refresh(store: &SessionStore) -> Result<Session, String> {
    store.refresh(false)
}

pub fn api_login_refresh(store: &SessionStore) -> Result<Session, String> {
    store.refresh(false)
}
