//! UNRELATED to the spec — added in `scope-creep` variant to test that
//! the auditor catches scope creep. Spec was "single file change in
//! session.rs" but executor refactored config handling here too.

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub max_sessions: usize,
    pub session_ttl_secs: u64,
}

impl RuntimeConfig {
    pub fn from_env() -> Self {
        Self {
            max_sessions: 10_000,
            session_ttl_secs: 3600,
        }
    }

    /// Refactor not in the spec — adjusts TTL based on environment hint.
    pub fn adjust_ttl(&mut self, hint: u64) {
        self.session_ttl_secs = hint.max(60);
    }
}
