use std::{
    collections::HashSet,
    env,
    path::{Component, Path, PathBuf},
};

use crate::{Result, collect_files_with_extension};

pub(super) const KIMI_DATA_DIR_ENV: &str = "KIMI_DATA_DIR";
pub(super) const KIMI_MODEL_NAME_ENV: &str = "KIMI_MODEL_NAME";
pub(super) const KIMI_SESSIONS_DIR_NAME: &str = "sessions";
pub(super) const KIMI_AGENTS_DIR_NAME: &str = "agents";
pub(super) const KIMI_SUBAGENTS_DIR_NAME: &str = "subagents";
pub(super) const KIMI_WIRE_FILE_NAME: &str = "wire.jsonl";
pub(super) const KIMI_CONFIG_JSON_FILE_NAME: &str = "config.json";
pub(super) const KIMI_CONFIG_TOML_FILE_NAME: &str = "config.toml";
pub(super) const MAIN_STREAM_ID: &str = "main";

#[derive(Debug, Clone)]
pub(super) struct KimiWireContext {
    pub(super) session_id: String,
    pub(super) stream_id: String,
}

pub(super) fn paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(env_paths) = env::var(KIMI_DATA_DIR_ENV) {
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
        for dir in [".kimi", ".kimi-code"] {
            let path = home.join(dir);
            if path.is_dir() && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

pub(super) fn discover_wire_files() -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for kimi_path in paths()? {
        let sessions_path = kimi_path.join(KIMI_SESSIONS_DIR_NAME);
        let mut candidates = Vec::new();
        collect_files_with_extension(&sessions_path, "jsonl", &mut candidates);
        files.extend(
            candidates
                .into_iter()
                .filter(|file| is_kimi_wire_file(&sessions_path, file)),
        );
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn is_kimi_wire_file(sessions_path: &Path, file_path: &Path) -> bool {
    if file_path.file_name().and_then(|name| name.to_str()) != Some(KIMI_WIRE_FILE_NAME) {
        return false;
    }
    let Ok(relative) = file_path.strip_prefix(sessions_path) else {
        return false;
    };
    relative_wire_context(relative).is_some()
}

pub(super) fn wire_context_from_path(file_path: &Path) -> Option<KimiWireContext> {
    file_path
        .ancestors()
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()) == Some(KIMI_SESSIONS_DIR_NAME)
        })
        .filter_map(|sessions_path| {
            let relative = file_path.strip_prefix(sessions_path).ok()?;
            relative_wire_context(relative)
        })
        .last()
}

pub(super) fn root_from_wire_path(file_path: &Path) -> Option<PathBuf> {
    file_path
        .ancestors()
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()) == Some(KIMI_SESSIONS_DIR_NAME)
        })
        .filter_map(|sessions_path| {
            let relative = file_path.strip_prefix(sessions_path).ok()?;
            relative_wire_context(relative)?;
            sessions_path.parent().map(Path::to_path_buf)
        })
        .last()
}

fn relative_wire_context(path: &Path) -> Option<KimiWireContext> {
    let parts = path_normal_components(path);
    wire_context_from_parts(&parts)
}

fn path_normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(ToString::to_string),
            _ => None,
        })
        .collect()
}

fn wire_context_from_parts(parts: &[String]) -> Option<KimiWireContext> {
    if parts.len() < 3 || parts.last().map(String::as_str) != Some(KIMI_WIRE_FILE_NAME) {
        return None;
    }
    let session_id = parts.get(1)?.trim();
    if session_id.is_empty() {
        return None;
    }
    let nested_parts = &parts[2..parts.len() - 1];
    let stream_id = nested_stream_id(nested_parts)?;
    Some(KimiWireContext {
        session_id: session_id.to_string(),
        stream_id,
    })
}

fn nested_stream_id(parts: &[String]) -> Option<String> {
    if parts.is_empty() {
        return Some(MAIN_STREAM_ID.to_string());
    }
    if let [directory, agent_id] = parts
        && directory == KIMI_AGENTS_DIR_NAME
    {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return None;
        }
        return Some(format!("agent:{agent_id}"));
    }
    if parts.len() % 2 != 0 {
        return None;
    }
    let mut stream_id = MAIN_STREAM_ID.to_string();
    for pair in parts.chunks_exact(2) {
        if pair[0] != KIMI_SUBAGENTS_DIR_NAME {
            return None;
        }
        let subagent_id = pair[1].trim();
        if subagent_id.is_empty() {
            return None;
        }
        stream_id = combine_stream_id(&stream_id, &format!("subagent:{subagent_id}"));
    }
    Some(stream_id)
}

pub(super) fn combine_stream_id(parent_stream_id: &str, child_stream_id: &str) -> String {
    if child_stream_id == MAIN_STREAM_ID {
        return parent_stream_id.to_string();
    }
    if parent_stream_id == MAIN_STREAM_ID {
        return child_stream_id.to_string();
    }
    format!("{parent_stream_id}/{child_stream_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ccusage_test_support::{EnvVarGuard, fs_fixture};

    #[test]
    fn discovers_wire_jsonl_files_under_sessions_group_session() {
        let fixture = fs_fixture!({
            "sessions/group/session/wire.jsonl": "{}\n",
            "sessions/group/session/subagents/agent-1/wire.jsonl": "{}\n",
            "sessions/group/session/subagents/agent-1/extra/wire.jsonl": "{}\n",
            "sessions/group/session/other.jsonl": "{}\n",
            "sessions/nested/path/session/wire.jsonl": "{}\n",
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let files = discover_wire_files().unwrap();

        assert_eq!(
            files,
            vec![
                fixture.path("sessions/group/session/subagents/agent-1/wire.jsonl"),
                fixture.path("sessions/group/session/wire.jsonl")
            ]
        );
    }

    #[test]
    fn discovers_both_old_and_new_layouts_and_skips_non_wire_files() {
        let fixture = fs_fixture!({
            "sessions/ws/session-c/agents/agent-1/wire.jsonl": "{}\n",
            "sessions/ws/session-c/agents/agent-1/other.jsonl": "{}\n",
            "sessions/ws/session-c/not-agents/agent-1/wire.jsonl": "{}\n",
            "sessions/group/session-d/wire.jsonl": "{}\n",
        });
        let _cleanup = EnvVarGuard::set(KIMI_DATA_DIR_ENV, fixture.root());
        let files = discover_wire_files().unwrap();

        assert_eq!(
            files,
            vec![
                fixture.path("sessions/group/session-d/wire.jsonl"),
                fixture.path("sessions/ws/session-c/agents/agent-1/wire.jsonl"),
            ]
        );

        let context = wire_context_from_path(&files[1]).unwrap();
        assert_eq!(context.session_id, "session-c");
        assert_eq!(context.stream_id, "agent:agent-1");
    }
}
