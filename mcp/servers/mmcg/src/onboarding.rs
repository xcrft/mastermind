//! Project-local onboarding choices. Runtime grants and worker counters remain
//! in their existing stores. This file records intent, never authority.

use crate::bounded_fs::{
    self, AtomicWriteExpectation, BoundedReadError, ReadControl, RootCapability,
    StableFileIdentity, StableFileLock,
};
use crate::invocation::{DEFAULT_MAX_TURNS as DEFAULT_EXEC_MAX_TURNS, DEFAULT_WALL_TIMEOUT_SECS};
use crate::review_invocation::{DEFAULT_MAX_TURNS, DEFAULT_TIMEOUT_SECS};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

type Error = Box<dyn std::error::Error>;
const MAX_BYTES: u64 = 16 * 1024;
/// Default `mmcg_profile` / hook / context-person delivery budget, in effect
/// whenever a project has not set `profile_budget_tokens`.
pub const DEFAULT_PROFILE_BUDGET_TOKENS: usize = 4000;
pub const MIN_PROFILE_BUDGET_TOKENS: usize = 256;
pub const MAX_PROFILE_BUDGET_TOKENS: usize = 8000;

fn default_profile_budget_tokens() -> usize {
    DEFAULT_PROFILE_BUDGET_TOKENS
}

fn is_default_profile_budget_tokens(value: &usize) -> bool {
    *value == DEFAULT_PROFILE_BUDGET_TOKENS
}

pub const MIN_TURN_LIMIT: u32 = 1;
pub const MAX_TURN_LIMIT: u32 = 100;

fn default_review_max_turns() -> u32 {
    DEFAULT_MAX_TURNS
}

fn is_default_review_max_turns(value: &u32) -> bool {
    *value == DEFAULT_MAX_TURNS
}

pub const MIN_TIMEOUT_SECS: u64 = 1;
pub const MAX_TIMEOUT_SECS: u64 = 7200;

fn default_exec_max_turns() -> u32 {
    DEFAULT_EXEC_MAX_TURNS
}

fn is_default_exec_max_turns(value: &u32) -> bool {
    *value == DEFAULT_EXEC_MAX_TURNS
}

fn default_exec_timeout_secs() -> u64 {
    DEFAULT_WALL_TIMEOUT_SECS
}

fn is_default_exec_timeout_secs(value: &u64) -> bool {
    *value == DEFAULT_WALL_TIMEOUT_SECS
}

fn default_review_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn is_default_review_timeout_secs(value: &u64) -> bool {
    *value == DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mining {
    Off,
    Capture,
    Task,
    On,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub project_root: PathBuf,
    pub clients: Vec<String>,
    /// Previously selected clients whose project grants still need cleanup.
    #[serde(default)]
    pub pending_removals: Vec<String>,
    pub mining: Mining,
    pub provider: Option<String>,
    pub max_calls: u64,
    pub max_runtime: u64,
    pub refiner: bool,
    pub profile_access: bool,
    pub workflow: bool,
    /// Tokens `mmcg_profile`, the prompt hook, and the context person layer
    /// may spend when the caller does not pass an explicit budget.
    #[serde(
        default = "default_profile_budget_tokens",
        skip_serializing_if = "is_default_profile_budget_tokens"
    )]
    pub profile_budget_tokens: usize,
    /// Native turns for `review-task run` and `run-task --auto-review` when
    /// the command line does not pass an explicit budget.
    #[serde(
        default = "default_review_max_turns",
        skip_serializing_if = "is_default_review_max_turns"
    )]
    pub review_max_turns: u32,
    /// Native turns for `run-task --exec` when the command line does not pass
    /// an explicit budget.
    #[serde(
        default = "default_exec_max_turns",
        skip_serializing_if = "is_default_exec_max_turns"
    )]
    pub exec_max_turns: u32,
    /// Wall-clock seconds for `run-task --exec` when the command line does not
    /// pass an explicit limit.
    #[serde(
        default = "default_exec_timeout_secs",
        skip_serializing_if = "is_default_exec_timeout_secs"
    )]
    pub exec_timeout_secs: u64,
    /// Wall-clock seconds for `review-task run` and `run-task --auto-review`
    /// when the command line does not pass an explicit limit.
    #[serde(
        default = "default_review_timeout_secs",
        skip_serializing_if = "is_default_review_timeout_secs"
    )]
    pub review_timeout_secs: u64,
}

impl Settings {
    pub fn local(root: &Path) -> Self {
        Self {
            schema_version: 1,
            project_root: root.to_path_buf(),
            clients: Vec::new(),
            pending_removals: Vec::new(),
            mining: Mining::Off,
            provider: None,
            max_calls: 64,
            max_runtime: 3600,
            refiner: false,
            profile_access: false,
            workflow: true,
            profile_budget_tokens: DEFAULT_PROFILE_BUDGET_TOKENS,
            review_max_turns: DEFAULT_MAX_TURNS,
            exec_max_turns: DEFAULT_EXEC_MAX_TURNS,
            exec_timeout_secs: DEFAULT_WALL_TIMEOUT_SECS,
            review_timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    pub fn validate(&self, root: &Path) -> Result<(), Error> {
        if self.schema_version != 1 || self.project_root != root {
            return Err("setup settings belong to another project or schema".into());
        }
        if self.clients.len() > 2
            || self
                .clients
                .iter()
                .any(|client| !matches!(client.as_str(), "claude" | "codex"))
            || self.clients.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("setup clients must be unique and sorted: claude, codex".into());
        }
        if self.pending_removals.len() > 2
            || self.pending_removals.iter().any(|client| {
                !matches!(client.as_str(), "claude" | "codex") || self.clients.contains(client)
            })
            || self
                .pending_removals
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err("invalid pending client removals".into());
        }
        if !(1..=10_000).contains(&self.max_calls) || !(1..=86_400).contains(&self.max_runtime) {
            return Err("mining budget must be 1-10000 calls and 1-86400 seconds".into());
        }
        if !(MIN_PROFILE_BUDGET_TOKENS..=MAX_PROFILE_BUDGET_TOKENS)
            .contains(&self.profile_budget_tokens)
        {
            return Err(format!(
                "profile budget must be {MIN_PROFILE_BUDGET_TOKENS}-{MAX_PROFILE_BUDGET_TOKENS} tokens"
            )
            .into());
        }
        if !(MIN_TURN_LIMIT..=MAX_TURN_LIMIT).contains(&self.review_max_turns) {
            return Err(
                format!("review max turns must be {MIN_TURN_LIMIT}-{MAX_TURN_LIMIT}").into(),
            );
        }
        if !(MIN_TURN_LIMIT..=MAX_TURN_LIMIT).contains(&self.exec_max_turns) {
            return Err(format!("exec max turns must be {MIN_TURN_LIMIT}-{MAX_TURN_LIMIT}").into());
        }
        if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&self.exec_timeout_secs) {
            return Err(format!(
                "exec timeout must be {MIN_TIMEOUT_SECS}-{MAX_TIMEOUT_SECS} seconds"
            )
            .into());
        }
        if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&self.review_timeout_secs) {
            return Err(format!(
                "review timeout must be {MIN_TIMEOUT_SECS}-{MAX_TIMEOUT_SECS} seconds"
            )
            .into());
        }
        if self
            .provider
            .as_deref()
            .is_some_and(|value| !matches!(value, "native" | "claude" | "codex"))
        {
            return Err("supported semantic providers: native, claude, codex".into());
        }
        if (self.mining == Mining::On || self.refiner) && self.provider.is_none() {
            return Err(
                "semantic mining and refinement require an explicit --provider native, claude or codex".into(),
            );
        }
        if self.refiner && self.mining == Mining::Off {
            return Err("refinement requires hooks: choose --mining capture, task or on".into());
        }
        if self.clients.is_empty()
            && (self.mining != Mining::Off || self.profile_access || self.refiner)
        {
            return Err(
                "select --client claude, codex or all before enabling client features".into(),
            );
        }
        Ok(())
    }
}

fn read_at(
    capability: &RootCapability,
    root: &Path,
) -> Result<Option<(Settings, StableFileIdentity)>, Error> {
    let file = match bounded_fs::read_regular_file_with_capability(
        capability,
        &root.join(".mastermind/setup.json"),
        MAX_BYTES,
        MAX_BYTES,
        ReadControl::default(),
    ) {
        Ok(file) => file,
        Err(BoundedReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(error) => return Err(error.into()),
    };
    let value = crate::setup::parse_json_unique(&file.bytes).map_err(|_| "invalid setup JSON")?;
    let settings: Settings = serde_json::from_value(value)?;
    settings.validate(root)?;
    Ok(Some((settings, file.identity)))
}

/// Read-only. Missing state stays missing, including the .mastermind directory.
pub fn load(root: &Path) -> Result<Option<Settings>, Error> {
    let capability = RootCapability::open(root)?;
    Ok(read_at(&capability, root)?.map(|(settings, _)| settings))
}

/// Read the project onboarding profile-delivery budget from existing settings.
///
/// Missing or unreadable `setup.json` uses the in-memory default rather than
/// blocking profile delivery. Loading never creates onboarding state.
pub fn profile_budget_tokens(root: &Path) -> usize {
    setting(root, DEFAULT_PROFILE_BUDGET_TOKENS, |s| {
        s.profile_budget_tokens
    })
}

fn setting<T>(root: &Path, default: T, pick: fn(&Settings) -> T) -> T {
    load(root)
        .ok()
        .flatten()
        .map_or(default, |settings| pick(&settings))
}

/// Project reviewer turn budget, fail-soft like [`profile_budget_tokens`]: a
/// missing or unreadable `setup.json` reads as the default.
pub fn review_max_turns(root: &Path) -> u32 {
    setting(root, DEFAULT_MAX_TURNS, |s| s.review_max_turns)
}

/// Project executor turn budget; a missing or unreadable `setup.json` reads as the default.
pub fn exec_max_turns(root: &Path) -> u32 {
    setting(root, DEFAULT_EXEC_MAX_TURNS, |s| s.exec_max_turns)
}

/// Project executor wall-clock limit in seconds; a missing or unreadable `setup.json` reads as
/// the default.
pub fn exec_timeout_secs(root: &Path) -> u64 {
    setting(root, DEFAULT_WALL_TIMEOUT_SECS, |s| s.exec_timeout_secs)
}

/// Project reviewer wall-clock limit in seconds; a missing or unreadable `setup.json` reads as
/// the default.
pub fn review_timeout_secs(root: &Path) -> u64 {
    setting(root, DEFAULT_TIMEOUT_SECS, |s| s.review_timeout_secs)
}

pub fn profile_access(root: &Path, client: &str) -> Result<bool, Error> {
    let path = crate::miner::store::ProfileStore::db_path().ok_or("home directory unavailable")?;
    let Some(store) = crate::miner::store::ProfileStore::open_optional_read_only(&path)? else {
        return Ok(false);
    };
    Ok(store.reader_allowed(root.to_str().ok_or("project root is not UTF-8")?, client)?)
}

/// Hold this guard throughout reconciliation so two init processes cannot
/// apply different desired settings to the same project concurrently.
pub struct Session {
    capability: RootCapability,
    root: PathBuf,
    _lock: StableFileLock,
    previous: Option<(Settings, StableFileIdentity)>,
}

impl Session {
    pub fn begin(root: &Path) -> Result<Self, Error> {
        let capability = RootCapability::open(root)?;
        capability.ensure_directory(Path::new(".mastermind"))?;
        let lock = bounded_fs::try_locked_regular_file_with_capability(
            &capability,
            &root.join(".mastermind/setup.lock"),
        )?;
        let previous = read_at(&capability, root)?;
        Ok(Self {
            capability,
            root: root.into(),
            _lock: lock,
            previous,
        })
    }

    pub fn settings(&self) -> Option<&Settings> {
        self.previous.as_ref().map(|(settings, _)| settings)
    }

    pub fn save(&mut self, settings: &Settings) -> Result<(), Error> {
        settings.validate(&self.root)?;
        if self.settings() == Some(settings) {
            return Ok(());
        }
        let path = self.root.join(".mastermind/setup.json");
        let expectation = match &self.previous {
            Some((_, identity)) => AtomicWriteExpectation::File(*identity),
            None => AtomicWriteExpectation::Missing(
                bounded_fs::inspect_absent_path(&self.capability, &path, ReadControl::default())?
                    .ok_or("setup settings changed")?,
            ),
        };
        let bytes = serde_json::to_vec_pretty(settings)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("setup settings exceed the file limit".into());
        }
        bounded_fs::write_atomic_regular_file_expected_with_capability(
            &self.capability,
            &path,
            &bytes,
            true,
            expectation,
        )?;
        self.previous = read_at(&self.capability, &self.root)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_root_bound_and_saved_without_granting_access() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(load(&root).unwrap().is_none());
        assert!(!root.join(".mastermind").exists());
        let mut session = Session::begin(&root).unwrap();
        let mut settings = Settings::local(&root);
        settings.clients = vec!["codex".into()];
        settings.mining = Mining::Capture;
        session.save(&settings).unwrap();
        assert_eq!(load(&root).unwrap(), Some(settings.clone()));
        assert!(Session::begin(&root).is_err());
        settings.project_root = root.join("other");
        assert!(session.save(&settings).is_err());
    }

    #[test]
    fn invalid_or_duplicate_settings_do_not_get_repaired_implicitly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".mastermind")).unwrap();
        let path = root.join(".mastermind/setup.json");
        std::fs::write(&path, br#"{"schema_version":1,"schema_version":1}"#).unwrap();
        assert!(load(&root).is_err());
        assert!(Session::begin(&root).is_err());
        let mut settings = Settings::local(&root);
        settings.clients = vec!["claude".into()];
        settings.mining = Mining::On;
        assert!(settings.validate(&root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn linked_settings_and_state_directories_are_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.join(".mastermind")).unwrap();
        assert!(Session::begin(&root).is_err());
        assert!(load(&root).is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn profile_budget_falls_back_to_the_default_on_a_missing_or_unreadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(profile_budget_tokens(&root), DEFAULT_PROFILE_BUDGET_TOKENS);

        std::fs::create_dir(root.join(".mastermind")).unwrap();
        std::fs::write(root.join(".mastermind/setup.json"), b"not json").unwrap();
        assert!(load(&root).is_err());
        assert_eq!(profile_budget_tokens(&root), DEFAULT_PROFILE_BUDGET_TOKENS);
    }

    #[test]
    fn old_setup_file_without_the_profile_budget_key_loads_as_the_default_and_omits_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".mastermind")).unwrap();
        let legacy = serde_json::json!({
            "schema_version": 1,
            "project_root": root,
            "clients": [],
            "mining": "off",
            "provider": null,
            "max_calls": 64,
            "max_runtime": 3600,
            "refiner": false,
            "profile_access": false,
            "workflow": true,
        });
        std::fs::write(
            root.join(".mastermind/setup.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let settings = load(&root).unwrap().unwrap();
        assert_eq!(
            settings.profile_budget_tokens,
            DEFAULT_PROFILE_BUDGET_TOKENS
        );
        assert_eq!(profile_budget_tokens(&root), DEFAULT_PROFILE_BUDGET_TOKENS);
        assert!(!serde_json::to_string(&settings)
            .unwrap()
            .contains("profile_budget_tokens"));
    }

    fn write_settings(root: &Path, settings: &Settings) {
        std::fs::create_dir_all(root.join(".mastermind")).unwrap();
        std::fs::write(
            root.join(".mastermind/setup.json"),
            serde_json::to_vec(settings).unwrap(),
        )
        .unwrap();
    }

    fn assert_range<T: Copy + std::fmt::Debug>(
        set: impl Fn(&mut Settings, T),
        valid: [T; 2],
        invalid: [T; 2],
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut settings = Settings::local(&root);
        for value in valid {
            set(&mut settings, value);
            assert!(settings.validate(&root).is_ok(), "{value:?}");
        }
        for value in invalid {
            set(&mut settings, value);
            assert!(settings.validate(&root).is_err(), "{value:?}");
        }
    }

    #[test]
    fn turn_limits_outside_one_to_one_hundred_are_invalid() {
        assert_range(|s, v| s.review_max_turns = v, [1, 100], [0, 101]);
        assert_range(|s, v| s.exec_max_turns = v, [1, 100], [0, 101]);
    }

    #[test]
    fn exec_and_review_timeouts_outside_one_to_7200_seconds_are_invalid() {
        assert_range(|s, v| s.exec_timeout_secs = v, [1, 7200], [0, 7201]);
        assert_range(|s, v| s.review_timeout_secs = v, [1, 7200], [0, 7201]);
    }

    #[test]
    fn the_native_limits_read_as_the_defaults_without_a_readable_setup_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let defaults = (
            crate::review_invocation::DEFAULT_MAX_TURNS,
            crate::invocation::DEFAULT_MAX_TURNS,
            crate::invocation::DEFAULT_WALL_TIMEOUT_SECS,
            crate::review_invocation::DEFAULT_TIMEOUT_SECS,
        );
        let read = |root: &Path| {
            (
                review_max_turns(root),
                exec_max_turns(root),
                exec_timeout_secs(root),
                review_timeout_secs(root),
            )
        };
        assert_eq!(defaults, (30, 40, 1800, 600));
        assert_eq!(read(&root), defaults);

        std::fs::create_dir(root.join(".mastermind")).unwrap();
        std::fs::write(root.join(".mastermind/setup.json"), b"not json").unwrap();
        assert!(load(&root).is_err());
        assert_eq!(read(&root), defaults);

        let legacy = serde_json::json!({
            "schema_version": 1,
            "project_root": root,
            "clients": [],
            "mining": "off",
            "provider": null,
            "max_calls": 64,
            "max_runtime": 3600,
            "refiner": false,
            "profile_access": false,
            "workflow": true,
        });
        std::fs::write(
            root.join(".mastermind/setup.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        assert_eq!(read(&root), defaults);
    }

    #[test]
    fn configured_native_limits_are_read_back_and_defaults_are_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut settings = Settings::local(&root);
        let written = serde_json::to_string(&settings).unwrap();
        for key in [
            "review_max_turns",
            "exec_max_turns",
            "exec_timeout_secs",
            "review_timeout_secs",
        ] {
            assert!(!written.contains(key), "{key}");
        }
        settings.review_max_turns = 25;
        settings.exec_max_turns = 7;
        settings.exec_timeout_secs = 20;
        settings.review_timeout_secs = 45;
        write_settings(&root, &settings);
        assert_eq!(review_max_turns(&root), 25);
        assert_eq!(exec_max_turns(&root), 7);
        assert_eq!(exec_timeout_secs(&root), 20);
        assert_eq!(review_timeout_secs(&root), 45);
    }
}
