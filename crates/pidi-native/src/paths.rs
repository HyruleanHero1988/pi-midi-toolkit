//! Canonical user-data locations for the appliance kiosk.
//!
//! Specific env vars (`PIDI_SONGS_DIR`, …) win. Otherwise a non-empty
//! `PIDI_DATA_ROOT` is joined with the usual subdirectory names. Host/dev
//! without a data root stays cwd-relative (`songs/`, `phrases/`, …) so tests
//! and `cargo run` keep working.

use std::path::PathBuf;

pub fn env_path(key: &str) -> Option<PathBuf> {
    match std::env::var(key) {
        Ok(p) if !p.trim().is_empty() => Some(PathBuf::from(p)),
        _ => None,
    }
}

pub fn data_root() -> Option<PathBuf> {
    env_path("PIDI_DATA_ROOT")
}

/// `PIDI_DATA_ROOT/<name>` when a data root is set, else a cwd-relative `name`.
pub fn join_data_or_cwd(name: &str) -> PathBuf {
    match data_root() {
        Some(root) => root.join(name),
        None => PathBuf::from(name),
    }
}

/// Specific env var, then `PIDI_DATA_ROOT/<subdir>`, then cwd-relative `subdir`.
pub fn resolve_dir(specific_env: &str, subdir: &str) -> PathBuf {
    env_path(specific_env).unwrap_or_else(|| join_data_or_cwd(subdir))
}

/// Specific env var, then `PIDI_DATA_ROOT/<filename>`, then cwd-relative file.
pub fn resolve_file(specific_env: &str, filename: &str) -> PathBuf {
    env_path(specific_env).unwrap_or_else(|| join_data_or_cwd(filename))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn specific_env_wins_over_data_root() {
        let _lock = LOCK.lock().unwrap();
        let _root = EnvGuard::set("PIDI_DATA_ROOT", "/data/pidi");
        let _songs = EnvGuard::set("PIDI_SONGS_DIR", "/custom/songs");
        assert_eq!(
            resolve_dir("PIDI_SONGS_DIR", "songs"),
            PathBuf::from("/custom/songs")
        );
    }

    #[test]
    fn data_root_joins_subdir_and_file() {
        let _lock = LOCK.lock().unwrap();
        let _songs = EnvGuard::unset("PIDI_SONGS_DIR");
        let _settings = EnvGuard::unset("PIDI_SETTINGS_PATH");
        let _root = EnvGuard::set("PIDI_DATA_ROOT", "/data/pidi");
        assert_eq!(
            resolve_dir("PIDI_SONGS_DIR", "songs"),
            PathBuf::from("/data/pidi/songs")
        );
        assert_eq!(
            resolve_file("PIDI_SETTINGS_PATH", "settings.json"),
            PathBuf::from("/data/pidi/settings.json")
        );
    }

    #[test]
    fn cwd_relative_without_data_root() {
        let _lock = LOCK.lock().unwrap();
        let _songs = EnvGuard::unset("PIDI_SONGS_DIR");
        let _root = EnvGuard::unset("PIDI_DATA_ROOT");
        assert_eq!(
            resolve_dir("PIDI_SONGS_DIR", "songs"),
            PathBuf::from("songs")
        );
    }
}
