use std::{fs, path::Path};

use crate::{LoadedEntry, PricingMap, Result, TimestampMs, cli::SharedArgs, parse_tz};

use super::{
    parser::{AntigravityStep, estimate_usage_events, parse_step_payload, usage_event_to_loaded},
    paths::discover_conversation_dbs,
};

pub(crate) fn load_entries(shared: &SharedArgs, _pricing: &PricingMap) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent::Antigravity,
        shared.json,
        || load_entries_inner(shared),
    )
}

fn load_entries_inner(shared: &SharedArgs) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    for db_path in discover_conversation_dbs()? {
        let session_id = db_path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let fallback_timestamp = file_modified_timestamp(&db_path);
        let steps = load_steps(&db_path, fallback_timestamp, shared);
        let events = estimate_usage_events(&steps, &session_id, fallback_timestamp);
        entries.extend(
            events
                .into_iter()
                .map(|event| usage_event_to_loaded(event, tz.as_ref(), shared.mode)),
        );
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

fn load_steps(
    db_path: &Path,
    fallback_timestamp: TimestampMs,
    shared: &SharedArgs,
) -> Vec<AntigravityStep> {
    let Ok(connection) =
        sqlite::Connection::open_with_flags(db_path, sqlite::OpenFlags::new().with_read_only())
    else {
        crate::debug_log(
            shared,
            format!(
                "Failed to open Antigravity conversation database: {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };
    let Ok(mut statement) =
        connection.prepare("SELECT idx, step_type, step_payload FROM steps ORDER BY idx")
    else {
        crate::debug_log(
            shared,
            format!(
                "Failed to read Antigravity conversation database: {}",
                db_path.display()
            ),
        );
        return Vec::new();
    };
    let mut steps = Vec::new();
    loop {
        match statement.next() {
            Ok(sqlite::State::Row) => {
                let idx = statement.read::<i64, _>(0).unwrap_or(0);
                let step_type = statement.read::<i64, _>(1).unwrap_or(0);
                let payload = statement.read::<Vec<u8>, _>(2).unwrap_or_default();
                steps.push(parse_step_payload(
                    idx,
                    step_type,
                    &payload,
                    fallback_timestamp,
                ));
            }
            Ok(sqlite::State::Done) => break,
            Err(_) => {
                crate::debug_log(
                    shared,
                    format!(
                        "Failed to query Antigravity conversation database: {}",
                        db_path.display()
                    ),
                );
                break;
            }
        }
    }
    steps
}

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .map(TimestampMs::from_millis)
        .unwrap_or(TimestampMs::UNIX_EPOCH)
}
