use std::{collections::HashSet, path::PathBuf};

use crate::{cli::SharedArgs, parse_tz, LoadedEntry, PricingMap, Result};

use super::{
    parser::{kimi_entry_key, kimi_entry_to_loaded, read_wire_file},
    paths::{discover_wire_files, paths, KIMI_CONFIG_JSON_FILE_NAME, KIMI_CONFIG_TOML_FILE_NAME},
};

pub(crate) fn load_entries(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Kimi, shared.json, || {
        load_entries_inner(shared, pricing)
    })
}

fn load_entries_inner(shared: &SharedArgs, pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for file in discover_wire_files()? {
        for entry in read_wire_file(&file)? {
            let key = kimi_entry_key(&entry);
            if seen.insert(key) {
                entries.push(kimi_entry_to_loaded(
                    entry,
                    tz.as_ref(),
                    shared.mode,
                    pricing,
                ));
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

pub(crate) fn source_files() -> Result<Vec<PathBuf>> {
    let mut files = discover_wire_files()?;
    for path in paths()? {
        files.push(path.join(KIMI_CONFIG_TOML_FILE_NAME));
        files.push(path.join(KIMI_CONFIG_JSON_FILE_NAME));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf};

    use super::super::paths::{KIMI_DATA_DIR_ENV, KIMI_MODEL_NAME_ENV};
    use super::*;

    struct EnvDirGuard {
        dir: PathBuf,
    }

    impl EnvDirGuard {
        fn set(dir: PathBuf) -> Self {
            env::set_var(KIMI_DATA_DIR_ENV, &dir);
            Self { dir }
        }
    }

    impl Drop for EnvDirGuard {
        fn drop(&mut self) {
            env::remove_var(KIMI_DATA_DIR_ENV);
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    struct EnvModelGuard;

    impl EnvModelGuard {
        fn set(model: &str) -> Self {
            env::set_var(KIMI_MODEL_NAME_ENV, model);
            Self
        }
    }

    impl Drop for EnvModelGuard {
        fn drop(&mut self) {
            env::remove_var(KIMI_MODEL_NAME_ENV);
        }
    }

    fn temp_kimi_dir(name: &str) -> PathBuf {
        let mut path = env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("ccusage-kimi-{name}-{nanos}"));
        path
    }

    fn write_kimi_usage_file(kimi_dir: &std::path::Path, timestamp: &str) {
        fs::create_dir_all(kimi_dir.join("sessions/group/session-a")).unwrap();
        fs::write(
            kimi_dir.join("sessions/group/session-a/wire.jsonl"),
            format!(
                r#"{{"timestamp":{timestamp},"message":{{"type":"StatusUpdate","payload":{{"token_usage":{{"input_other":1000000,"output":1000000,"input_cache_read":1000000}},"message_id":"msg-1"}}}}}}"#
            ),
        )
        .unwrap();
    }

    fn assert_cost_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn loads_status_update_token_usage_from_wire_files() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("wire");
        fs::create_dir_all(kimi_dir.join("sessions/group/session-a")).unwrap();
        fs::write(kimi_dir.join("config.json"), r#"{"model":"kimi-k2"}"#).unwrap();
        fs::write(
            kimi_dir.join("sessions/group/session-a/wire.jsonl"),
            [
                r#"{"type":"metadata","protocol_version":"1.3"}"#,
                r#"{"timestamp":1770983426.420942,"message":{"type":"TurnBegin","payload":{"user_input":"hello"}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"msg-1"}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].date, "2026-02-13");
        assert_eq!(entries[0].session_id.as_ref(), "session-a");
        assert_eq!(entries[0].model.as_deref(), Some("kimi-k2"));
        assert_eq!(entries[0].data.message.usage.input_tokens, 100);
        assert_eq!(entries[0].data.message.usage.output_tokens, 50);
        assert_eq!(
            entries[0].data.message.usage.cache_creation_input_tokens,
            20
        );
        assert_eq!(entries[0].data.message.usage.cache_read_input_tokens, 10);
    }

    #[test]
    fn loads_subagent_event_token_usage_from_parent_wire_files() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("subagent-event");
        fs::create_dir_all(kimi_dir.join("sessions/group/session-a")).unwrap();
        fs::write(kimi_dir.join("config.json"), r#"{"model":"kimi-k2"}"#).unwrap();
        fs::write(
            kimi_dir.join("sessions/group/session-a/wire.jsonl"),
            [
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"shared-id"}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"shared-id"}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"SubagentEvent","payload":{"task_tool_call_id":"tool-1","event":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"shared-id"}}}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"SubagentEvent","payload":{"agent_id":"agent-2","parent_tool_call_id":"tool-2","subagent_type":"worker","event":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"shared-id"}}}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 3);
        assert!(entries
            .iter()
            .all(|entry| entry.session_id.as_ref() == "session-a"));
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.input_tokens)
                .sum::<u64>(),
            300
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.output_tokens)
                .sum::<u64>(),
            150
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.cache_creation_input_tokens)
                .sum::<u64>(),
            60
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.cache_read_input_tokens)
                .sum::<u64>(),
            30
        );
    }

    #[test]
    fn loads_nested_subagent_wire_files_under_parent_session() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("nested-subagent");
        fs::create_dir_all(kimi_dir.join("sessions/group/session-a/subagents/agent-1")).unwrap();
        fs::write(kimi_dir.join("config.json"), r#"{"model":"kimi-k2"}"#).unwrap();
        fs::write(
            kimi_dir.join("sessions/group/session-a/wire.jsonl"),
            [
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"main-1"}}}"#,
                r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"main-1"}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            kimi_dir.join("sessions/group/session-a/subagents/agent-1/wire.jsonl"),
            r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"main-1"}}}"#,
        )
        .unwrap();
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let entries = load_entries(&shared, &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.session_id.as_ref() == "session-a"));
        assert!(entries
            .iter()
            .all(|entry| entry.model.as_deref() == Some("kimi-k2")));
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.input_tokens)
                .sum::<u64>(),
            200
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.output_tokens)
                .sum::<u64>(),
            100
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.cache_creation_input_tokens)
                .sum::<u64>(),
            40
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.data.message.usage.cache_read_input_tokens)
                .sum::<u64>(),
            20
        );
    }

    #[test]
    fn reads_model_config_when_ids_are_named_sessions() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("sessions-name");
        fs::create_dir_all(kimi_dir.join("sessions/group/sessions/subagents/sessions")).unwrap();
        fs::write(kimi_dir.join("config.json"), r#"{"model":"kimi-k2"}"#).unwrap();
        fs::write(
            kimi_dir.join("sessions/group/sessions/subagents/sessions/wire.jsonl"),
            r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50},"message_id":"msg-1"}}}"#,
        )
        .unwrap();
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id.as_ref(), "sessions");
        assert_eq!(entries[0].model.as_deref(), Some("kimi-k2"));
    }

    #[test]
    fn loads_config_toml_display_slug_and_prices_migrating_model() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("config-toml");
        fs::create_dir_all(&kimi_dir).unwrap();
        fs::write(
            kimi_dir.join("config.toml"),
            [
                r#"default_model = "kimi-code/kimi-for-coding""#,
                r#"[models."kimi-code/kimi-for-coding"]"#,
                r#"model = "kimi-for-coding""#,
            ]
            .join("\n"),
        )
        .unwrap();
        write_kimi_usage_file(&kimi_dir, "1776698890.072");
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("kimi-for-coding"));
        assert_cost_close(entries[0].cost, 5.11);
    }

    #[test]
    fn preserves_provider_qualified_model_from_config_toml() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("config-toml-provider");
        fs::create_dir_all(&kimi_dir).unwrap();
        fs::write(
            kimi_dir.join("config.toml"),
            [
                r#"default_model = "moonshot/kimi-k2.6""#,
                r#"[models."moonshot/kimi-k2.6"]"#,
                r#"model = "moonshot/kimi-k2.6""#,
            ]
            .join("\n"),
        )
        .unwrap();
        write_kimi_usage_file(&kimi_dir, "1776643200");
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("moonshot/kimi-k2.6"));
        assert_cost_close(entries[0].cost, 5.11);
    }

    #[test]
    fn preserves_provider_qualified_model_from_legacy_config_json() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("legacy-config-provider");
        fs::create_dir_all(&kimi_dir).unwrap();
        fs::write(
            kimi_dir.join("config.json"),
            r#"{"model":"moonshot/kimi-k2.6"}"#,
        )
        .unwrap();
        write_kimi_usage_file(&kimi_dir, "1776643200");
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("moonshot/kimi-k2.6"));
        assert_cost_close(entries[0].cost, 5.11);
    }

    #[test]
    fn preserves_provider_qualified_model_from_env() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("env-provider");
        write_kimi_usage_file(&kimi_dir, "1776643200");
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let _model = EnvModelGuard::set("moonshot/kimi-k2.6");
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("moonshot/kimi-k2.6"));
        assert_cost_close(entries[0].cost, 5.11);
    }

    #[test]
    fn skips_malformed_and_zero_token_wire_lines() {
        let _guard = super::super::KIMI_DATA_DIR_LOCK.lock().unwrap();
        let kimi_dir = temp_kimi_dir("zero");
        fs::create_dir_all(kimi_dir.join("sessions/group/session-a")).unwrap();
        fs::write(
            kimi_dir.join("sessions/group/session-a/wire.jsonl"),
            [
                "not json",
                r#"{"timestamp":1770983427,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":0,"output":0,"input_cache_read":0,"input_cache_creation":0}}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let _cleanup = EnvDirGuard::set(kimi_dir);
        let entries = load_entries(&SharedArgs::default(), &PricingMap::load_embedded()).unwrap();

        assert!(entries.is_empty());
    }
}
