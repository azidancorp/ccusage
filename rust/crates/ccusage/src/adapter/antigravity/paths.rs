use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

use crate::{Result, collect_files_with_extension};

pub(super) const ANTIGRAVITY_DATA_DIR_ENV: &str = "ANTIGRAVITY_DATA_DIR";
const ANTIGRAVITY_ROOT: &str = "antigravity-cli";
const CONVERSATIONS_DIR: &str = "conversations";

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(ANTIGRAVITY_DATA_DIR_ENV) {
        for raw in env_paths
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(raw);
            if path.is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        return Ok(paths);
    }

    if let Some(home) = crate::home::home_dir() {
        let path = home.join(".gemini").join(ANTIGRAVITY_ROOT);
        if path.is_dir() && seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

pub(crate) fn discover_conversation_dbs() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths()? {
        collect_files_with_extension(&conversation_dir(&path), "db", &mut files);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn conversation_dir(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some(CONVERSATIONS_DIR) {
        path.to_path_buf()
    } else {
        path.join(CONVERSATIONS_DIR)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use ccusage_test_support::EnvVarGuard;

    fn temp_antigravity_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("ccusage-antigravity-{name}-{nanos}"))
    }

    #[test]
    fn discovers_conversation_databases_under_antigravity_root() {
        let dir = temp_antigravity_dir("discover");
        fs::create_dir_all(dir.join("conversations/nested")).unwrap();
        fs::write(dir.join("conversations/session-a.db"), b"").unwrap();
        fs::write(dir.join("conversations/nested/session-b.db"), b"").unwrap();
        fs::write(dir.join("conversations/session-a.db-wal"), b"").unwrap();
        let _cleanup = EnvVarGuard::set(ANTIGRAVITY_DATA_DIR_ENV, &dir);

        let files = discover_conversation_dbs().unwrap();

        assert_eq!(
            files,
            vec![
                dir.join("conversations/nested/session-b.db"),
                dir.join("conversations/session-a.db"),
            ]
        );
    }
}
