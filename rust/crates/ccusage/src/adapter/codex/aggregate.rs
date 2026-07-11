use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

use jiff::tz::TimeZone as JiffTimeZone;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::{
    CodexGroup, CodexTokenUsageEvent, PricingMap, Result,
    adapter::cache::FileState,
    cli::{AgentReportKind, CodexSpeed, SharedArgs, WeekDay},
    fast::FxHashSet,
    format_date_tz, parse_ts_timestamp, parse_tz, wants_json, week_start,
};

use super::{loader, parser, paths, report::calculate_codex_event_cost};

type CodexEventKey = (u64, usize, i64, u64, usize, u64, u64, u64, u64, u64);
type CodexDedupeShards = [Mutex<FxHashSet<CodexEventKey>>];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCodexGroups {
    file_states: BTreeMap<String, FileState>,
    parser_states: BTreeMap<String, CachedCodexParserState>,
    complete_lines: BTreeMap<String, bool>,
    seen: Vec<CodexEventKey>,
    groups: BTreeMap<String, CodexGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCodexParserState {
    previous_totals: Option<crate::CodexRawUsage>,
    current_model: Option<String>,
    current_model_is_fallback: bool,
}

impl From<parser::CodexParserState> for CachedCodexParserState {
    fn from(state: parser::CodexParserState) -> Self {
        Self {
            previous_totals: state.previous_totals,
            current_model: state.current_model,
            current_model_is_fallback: state.current_model_is_fallback,
        }
    }
}

impl From<CachedCodexParserState> for parser::CodexParserState {
    fn from(state: CachedCodexParserState) -> Self {
        Self {
            previous_totals: state.previous_totals,
            current_model: state.current_model,
            current_model_is_fallback: state.current_model_is_fallback,
        }
    }
}

pub(crate) fn load_groups(
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<BTreeMap<String, CodexGroup>> {
    let sources = paths::codex_usage_sources()?;
    let files = source_files_from_sources(&sources);
    let file_states = file_states_for_files(&files);
    let signature =
        crate::adapter::cache::create_file_state_signature(&files, &[format!("speed={speed:?}")]);
    if let Some((cached_signature, cached)) = crate::adapter::cache::read_source_value_cache_entry::<
        CachedCodexGroups,
    >("codex", kind, shared, "codex-groups-v2")
    {
        if cached_signature == signature {
            return Ok(cached.groups);
        }
        if let Some(updated) = update_cached_groups_for_appends(
            cached,
            &sources,
            &file_states,
            shared,
            kind,
            pricing,
            speed,
        )? {
            crate::adapter::cache::write_source_value_cache_entry(
                "codex",
                kind,
                shared,
                "codex-groups-v2",
                &signature,
                &updated,
            );
            return Ok(updated.groups);
        }
    }

    let loaded = load_groups_cache_value(&sources, file_states, shared, kind, pricing, speed)?;
    crate::adapter::cache::write_source_value_cache_entry(
        "codex",
        kind,
        shared,
        "codex-groups-v2",
        &signature,
        &loaded,
    );
    Ok(loaded.groups)
}

fn load_groups_uncached_with_seen(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<(BTreeMap<String, CodexGroup>, Vec<CodexEventKey>)> {
    if sources.len() == 1 && !wants_json(shared) {
        return load_groups_from_directory_with_seen(&sources[0].dir, shared, kind, pricing, speed);
    }
    load_groups_from_sources_with_seen(sources, shared, kind, pricing, speed)
}

fn load_groups_cache_value(
    sources: &[paths::CodexUsageSource],
    file_states: BTreeMap<String, FileState>,
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<CachedCodexGroups> {
    let (groups, seen) = load_groups_uncached_with_seen(sources, shared, kind, pricing, speed)?;
    let parser_states = parser_states_for_recent_files(sources, &file_states, 8)?;
    let complete_lines = parser_states
        .keys()
        .filter_map(|file| {
            let path = Path::new(file);
            file_ends_with_newline(path).map(|complete| (file.clone(), complete))
        })
        .collect();
    Ok(CachedCodexGroups {
        file_states,
        parser_states,
        complete_lines,
        seen,
        groups,
    })
}

fn update_cached_groups_for_appends(
    mut cached: CachedCodexGroups,
    sources: &[paths::CodexUsageSource],
    current_states: &BTreeMap<String, FileState>,
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<Option<CachedCodexGroups>> {
    if cached
        .file_states
        .keys()
        .any(|file| !current_states.contains_key(file))
    {
        return Ok(None);
    }

    let source_dirs = source_dirs_by_file(sources);
    let mut changed = Vec::new();
    for (file, current) in current_states {
        match cached.file_states.get(file) {
            Some(previous) if previous == current => {}
            Some(previous)
                if current.size > previous.size
                    && cached.complete_lines.get(file).copied().unwrap_or(false)
                    && cached.parser_states.contains_key(file) =>
            {
                changed.push((file.clone(), previous.size))
            }
            None => changed.push((file.clone(), 0)),
            Some(_) => return Ok(None),
        }
    }
    if changed.is_empty() {
        cached.file_states = current_states.clone();
        return Ok(Some(cached));
    }
    if changed.len() > 16 {
        return Ok(None);
    }

    let timezone = parse_tz(shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    let mut seen = cached.seen.iter().copied().collect::<FxHashSet<_>>();
    for (file, offset) in changed {
        let Some(sessions_dir) = source_dirs.get(&file) else {
            return Ok(None);
        };
        let path = Path::new(&file);
        let parser_state = if offset == 0 {
            parser::CodexParserState::default()
        } else {
            let Some(state) = cached.parser_states.remove(&file) else {
                return Ok(None);
            };
            state.into()
        };
        let next_state = parser::visit_codex_session_file_from_offset(
            sessions_dir,
            path,
            offset,
            parser_state,
            |event| {
                add_event_to_groups_with_seen_set(
                    &event,
                    kind,
                    timezone.as_ref(),
                    shared,
                    pricing,
                    speed,
                    &mut seen,
                    &mut cached.groups,
                )
            },
        )?;
        cached
            .parser_states
            .insert(file.clone(), CachedCodexParserState::from(next_state));
        if let Some(complete) = file_ends_with_newline(path) {
            cached.complete_lines.insert(file.clone(), complete);
        }
        if let Some(state) = current_states.get(&file) {
            cached.file_states.insert(file, *state);
        }
    }
    cached.seen = seen.into_iter().collect();
    Ok(Some(cached))
}

fn source_files_from_sources(sources: &[paths::CodexUsageSource]) -> Vec<PathBuf> {
    let mut files = if let [source] = sources {
        paths::collect_codex_usage_files(&source.dir)
    } else {
        paths::collect_deduped_codex_usage_files(sources)
            .into_iter()
            .flat_map(|group| group.files)
            .collect::<Vec<_>>()
    };
    files.sort();
    files.dedup();
    files
}

fn file_states_for_files(files: &[PathBuf]) -> BTreeMap<String, FileState> {
    files
        .iter()
        .filter_map(|file| {
            crate::adapter::cache::file_state(file)
                .map(|state| (file.to_string_lossy().into_owned(), state))
        })
        .collect()
}

fn source_dirs_by_file(sources: &[paths::CodexUsageSource]) -> BTreeMap<String, PathBuf> {
    if let [source] = sources {
        return paths::collect_codex_usage_files(&source.dir)
            .into_iter()
            .map(|file| (file.to_string_lossy().into_owned(), source.dir.clone()))
            .collect();
    }
    paths::collect_deduped_codex_usage_files(sources)
        .into_iter()
        .flat_map(|group| {
            group
                .files
                .into_iter()
                .map(move |file| (file.to_string_lossy().into_owned(), group.dir.clone()))
        })
        .collect()
}

fn parser_states_for_recent_files(
    sources: &[paths::CodexUsageSource],
    file_states: &BTreeMap<String, FileState>,
    limit: usize,
) -> Result<BTreeMap<String, CachedCodexParserState>> {
    let source_dirs = source_dirs_by_file(sources);
    let mut files = file_states.iter().collect::<Vec<_>>();
    files.sort_by(|(_, left), (_, right)| {
        right
            .modified_secs
            .cmp(&left.modified_secs)
            .then_with(|| right.modified_nanos.cmp(&left.modified_nanos))
            .then_with(|| right.size.cmp(&left.size))
    });
    let mut states = BTreeMap::new();
    for (file, _) in files.into_iter().take(limit) {
        let Some(sessions_dir) = source_dirs.get(file) else {
            continue;
        };
        let state = parser::visit_codex_session_file_from_offset(
            sessions_dir,
            Path::new(file),
            0,
            parser::CodexParserState::default(),
            |_| Ok(()),
        )?;
        states.insert(file.clone(), CachedCodexParserState::from(state));
    }
    Ok(states)
}

fn file_ends_with_newline(path: &Path) -> Option<bool> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return Some(true);
    }
    file.seek(SeekFrom::End(-1)).ok()?;
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).ok()?;
    Some(byte[0] == b'\n')
}

#[cfg(test)]
fn load_groups_from_sources(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<BTreeMap<String, CodexGroup>> {
    Ok(load_groups_from_sources_with_seen(sources, shared, kind, pricing, speed)?.0)
}

fn load_groups_from_sources_with_seen(
    sources: &[paths::CodexUsageSource],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<(BTreeMap<String, CodexGroup>, Vec<CodexEventKey>)> {
    let mut groups = BTreeMap::new();
    let seen = create_dedupe_shards();
    for group in paths::collect_deduped_codex_usage_files(sources) {
        merge_groups(
            &mut groups,
            aggregate_files_with_dedupe(
                &group.dir,
                &group.files,
                shared,
                kind,
                pricing,
                speed,
                &seen,
            )?,
        );
    }
    let seen = collect_seen_keys(&seen);
    Ok((groups, seen))
}

#[cfg(test)]
pub(super) fn load_groups_from_directory(
    sessions_dir: &Path,
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<BTreeMap<String, CodexGroup>> {
    Ok(load_groups_from_directory_with_seen(sessions_dir, shared, kind, pricing, speed)?.0)
}

fn load_groups_from_directory_with_seen(
    sessions_dir: &Path,
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<(BTreeMap<String, CodexGroup>, Vec<CodexEventKey>)> {
    let files = paths::collect_codex_usage_files(sessions_dir);
    let seen = create_dedupe_shards();
    let groups = aggregate_files(sessions_dir, &files, shared, kind, pricing, speed, &seen)?;
    let seen = collect_seen_keys(&seen);
    Ok((groups, seen))
}

fn aggregate_files_with_dedupe(
    sessions_dir: &Path,
    files: &[PathBuf],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
    seen: &CodexDedupeShards,
) -> Result<BTreeMap<String, CodexGroup>> {
    aggregate_files(sessions_dir, files, shared, kind, pricing, speed, seen)
}

fn aggregate_files(
    sessions_dir: &Path,
    files: &[PathBuf],
    shared: &SharedArgs,
    kind: AgentReportKind,
    pricing: &PricingMap,
    speed: CodexSpeed,
    seen: &CodexDedupeShards,
) -> Result<BTreeMap<String, CodexGroup>> {
    let mut groups = BTreeMap::new();
    let timezone = parse_tz(shared.timezone.as_deref()).or_else(|| Some(JiffTimeZone::system()));
    let events = loader::load_codex_events_for_files(sessions_dir, files, shared.single_thread);
    for event in events {
        add_event_to_groups(
            &event,
            kind,
            timezone.as_ref(),
            shared,
            pricing,
            speed,
            seen,
            &mut groups,
        )?;
    }
    Ok(groups)
}

fn add_event_to_groups(
    event: &CodexTokenUsageEvent,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    pricing: &PricingMap,
    speed: CodexSpeed,
    seen: &CodexDedupeShards,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
        return Ok(());
    };
    let model = crate::model_aliases::resolve_model_name(model);
    let timestamp = parse_ts_timestamp(&event.timestamp)
        .ok_or_else(|| crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp)))?;
    if !insert_event_key(event, timestamp, model.as_ref(), kind, seen) {
        return Ok(());
    }
    add_deduped_event_to_groups(
        event,
        model.as_ref(),
        timestamp,
        kind,
        timezone,
        shared,
        pricing,
        speed,
        groups,
    )
}

fn add_event_to_groups_with_seen_set(
    event: &CodexTokenUsageEvent,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    pricing: &PricingMap,
    speed: CodexSpeed,
    seen: &mut FxHashSet<CodexEventKey>,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
        return Ok(());
    };
    let model = crate::model_aliases::resolve_model_name(model);
    let timestamp = parse_ts_timestamp(&event.timestamp)
        .ok_or_else(|| crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp)))?;
    if !seen.insert(codex_event_key(event, timestamp, model.as_ref(), kind)) {
        return Ok(());
    }
    add_deduped_event_to_groups(
        event,
        model.as_ref(),
        timestamp,
        kind,
        timezone,
        shared,
        pricing,
        speed,
        groups,
    )
}

fn add_deduped_event_to_groups(
    event: &CodexTokenUsageEvent,
    model: &str,
    timestamp: crate::TimestampMs,
    kind: AgentReportKind,
    timezone: Option<&JiffTimeZone>,
    shared: &SharedArgs,
    pricing: &PricingMap,
    speed: CodexSpeed,
    groups: &mut BTreeMap<String, CodexGroup>,
) -> Result<()> {
    let date = format_date_tz(timestamp, timezone);
    if shared.since.is_some() || shared.until.is_some() {
        let date_key = date.replace('-', "");
        if shared.since.as_ref().is_some_and(|since| &date_key < since)
            || shared.until.as_ref().is_some_and(|until| &date_key > until)
        {
            return Ok(());
        }
    }
    let period = match kind {
        AgentReportKind::Daily => date,
        AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
        AgentReportKind::Monthly => date[..7].to_string(),
        AgentReportKind::Session => event.session_id.clone(),
    };
    let group = groups.entry(period).or_default();
    accumulate_codex_event_into_group(group, event, model, timestamp, pricing, speed);
    Ok(())
}

fn accumulate_codex_event_into_group(
    group: &mut CodexGroup,
    event: &CodexTokenUsageEvent,
    model: &str,
    timestamp: crate::TimestampMs,
    pricing: &PricingMap,
    speed: CodexSpeed,
) {
    let cost = calculate_codex_event_cost(model, event, timestamp, pricing, speed);
    group.input_tokens += event.input_tokens;
    group.cached_input_tokens += event.cached_input_tokens;
    group.output_tokens += event.output_tokens;
    group.reasoning_output_tokens += event.reasoning_output_tokens;
    group.total_tokens += event.total_tokens;
    group.cost = Some(group.cost.unwrap_or_default() + cost);
    if group
        .last_activity
        .as_deref()
        .is_none_or(|current| event.timestamp.as_str() > current)
    {
        group.last_activity = Some(event.timestamp.clone());
    }

    let model_usage = group.models.entry(model.to_string()).or_default();
    model_usage.input_tokens += event.input_tokens;
    model_usage.cached_input_tokens += event.cached_input_tokens;
    model_usage.output_tokens += event.output_tokens;
    model_usage.reasoning_output_tokens += event.reasoning_output_tokens;
    model_usage.total_tokens += event.total_tokens;
    model_usage.cost = Some(model_usage.cost.unwrap_or_default() + cost);
    // Each event is one request, so its input size decides the pricing tier
    // here; the summed totals cannot recover per-request context sizes. The
    // boundary is per model (OpenAI's 272K models and any future tier with a
    // different threshold) rather than a single global constant, matching the
    // threshold used to price the long-context buckets.
    if event.input_tokens > crate::pricing::long_context_split_threshold(model) {
        model_usage.long_context_input_tokens += event.input_tokens;
        model_usage.long_context_cached_input_tokens += event.cached_input_tokens;
        model_usage.long_context_output_tokens += event.output_tokens;
    }
    model_usage.is_fallback |= event.is_fallback_model;
}

fn create_dedupe_shards() -> Vec<Mutex<FxHashSet<CodexEventKey>>> {
    let shard_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    (0..shard_count.max(1))
        .map(|_| Mutex::new(FxHashSet::default()))
        .collect()
}

fn collect_seen_keys(seen: &CodexDedupeShards) -> Vec<CodexEventKey> {
    seen.iter()
        .flat_map(|shard| shard.lock().unwrap().iter().copied().collect::<Vec<_>>())
        .collect()
}

fn insert_event_key(
    event: &CodexTokenUsageEvent,
    timestamp: crate::TimestampMs,
    model: &str,
    kind: AgentReportKind,
    seen: &CodexDedupeShards,
) -> bool {
    let key = codex_event_key(event, timestamp, model, kind);
    let mut hasher = FxHasher::default();
    key.hash(&mut hasher);
    let shard_index = hasher.finish() as usize % seen.len();
    seen[shard_index].lock().unwrap().insert(key)
}

fn codex_event_key(
    event: &CodexTokenUsageEvent,
    timestamp: crate::TimestampMs,
    model: &str,
    kind: AgentReportKind,
) -> CodexEventKey {
    let (session_hash, session_len) = if kind == AgentReportKind::Session {
        (hash_text(&event.session_id), event.session_id.len())
    } else {
        (0, 0)
    };
    (
        session_hash,
        session_len,
        timestamp.as_millis(),
        hash_text(model),
        model.len(),
        event.input_tokens,
        event.cached_input_tokens,
        event.output_tokens,
        event.reasoning_output_tokens,
        event.total_tokens,
    )
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

fn merge_groups(target: &mut BTreeMap<String, CodexGroup>, source: BTreeMap<String, CodexGroup>) {
    for (period, group) in source {
        let target_group = target.entry(period).or_default();
        target_group.input_tokens += group.input_tokens;
        target_group.cached_input_tokens += group.cached_input_tokens;
        target_group.output_tokens += group.output_tokens;
        target_group.reasoning_output_tokens += group.reasoning_output_tokens;
        target_group.total_tokens += group.total_tokens;
        if let Some(cost) = group.cost {
            target_group.cost = Some(target_group.cost.unwrap_or_default() + cost);
        }
        if target_group.last_activity.as_deref().is_none_or(|current| {
            group
                .last_activity
                .as_deref()
                .is_some_and(|next| next > current)
        }) {
            target_group.last_activity = group.last_activity;
        }
        for (model, usage) in group.models {
            let target_usage = target_group.models.entry(model).or_default();
            target_usage.input_tokens += usage.input_tokens;
            target_usage.cached_input_tokens += usage.cached_input_tokens;
            target_usage.output_tokens += usage.output_tokens;
            target_usage.reasoning_output_tokens += usage.reasoning_output_tokens;
            target_usage.total_tokens += usage.total_tokens;
            if let Some(cost) = usage.cost {
                target_usage.cost = Some(target_usage.cost.unwrap_or_default() + cost);
            }
            target_usage.long_context_input_tokens += usage.long_context_input_tokens;
            target_usage.long_context_cached_input_tokens += usage.long_context_cached_input_tokens;
            target_usage.long_context_output_tokens += usage.long_context_output_tokens;
            target_usage.is_fallback |= usage.is_fallback;
        }
    }
}

#[cfg(test)]
pub(crate) fn aggregate_events(
    events: &[CodexTokenUsageEvent],
    kind: AgentReportKind,
    timezone: Option<&str>,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> Result<BTreeMap<String, CodexGroup>> {
    let mut groups = BTreeMap::new();
    let timezone = parse_tz(timezone).or_else(|| Some(JiffTimeZone::system()));
    for event in events {
        let Some(model) = event.model.as_deref().filter(|model| !model.is_empty()) else {
            continue;
        };
        let timestamp = parse_ts_timestamp(&event.timestamp).ok_or_else(|| {
            crate::cli_error(format!("Invalid Codex timestamp: {}", event.timestamp))
        })?;
        let date = format_date_tz(timestamp, timezone.as_ref());
        let period = match kind {
            AgentReportKind::Daily => date,
            AgentReportKind::Weekly => week_start(&date, WeekDay::Monday).unwrap_or(date),
            AgentReportKind::Monthly => date[..7].to_string(),
            AgentReportKind::Session => event.session_id.clone(),
        };
        let group = groups.entry(period).or_insert_with(CodexGroup::default);
        let model = crate::model_aliases::resolve_model_name(model);
        accumulate_codex_event_into_group(group, event, model.as_ref(), timestamp, pricing, speed);
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    use ccusage_test_support::{EnvVarsGuard, Fixture, fs_fixture};
    use serde_json::json;

    use crate::{
        adapter::codex::paths::CodexUsageSource, model_aliases::set_model_aliases_for_tests,
    };

    #[test]
    fn same_size_rewrite_rebuilds_codex_groups() {
        let usage_line = |input_tokens: u64| {
            json!({
                "timestamp": "2026-07-09T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.2",
                        "last_token_usage": {
                            "input_tokens": input_tokens,
                            "cached_input_tokens": 0,
                            "output_tokens": 10,
                            "reasoning_output_tokens": 0,
                            "total_tokens": input_tokens + 10,
                        },
                    },
                },
            })
            .to_string()
                + "\n"
        };
        let first_line = usage_line(100);
        let second_line = usage_line(200);
        assert_eq!(first_line.len(), second_line.len());

        let fixture = fs_fixture!({
            "codex/sessions/session.jsonl": &first_line,
        });
        let cache = Fixture::new();
        let _env = EnvVarsGuard::set_many([
            ("CODEX_HOME", Some(fixture.path("codex").into_os_string())),
            (
                "XDG_CACHE_HOME",
                Some(cache.root().as_os_str().to_os_string()),
            ),
        ]);
        let shared = SharedArgs {
            json: true,
            offline: true,
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let pricing = PricingMap::default();

        let first = load_groups(
            &shared,
            AgentReportKind::Daily,
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        std::fs::write(fixture.path("codex/sessions/session.jsonl"), second_line).unwrap();
        let second = load_groups(
            &shared,
            AgentReportKind::Daily,
            &pricing,
            CodexSpeed::Standard,
        )
        .unwrap();

        assert_eq!(first["2026-07-09"].input_tokens, 100);
        assert_eq!(second["2026-07-09"].input_tokens, 200);
    }

    #[test]
    fn dedupes_copied_token_usage_across_session_files() {
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &usage_line,
            "sessions/goal.jsonl": &usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
                &PricingMap::default(),
                CodexSpeed::Standard,
            )
            .unwrap();

            assert_eq!(groups.len(), 1);
            let group = groups.get("2026-05-29").unwrap();
            assert_eq!(group.input_tokens, 1_000);
            assert_eq!(group.cached_input_tokens, 100);
            assert_eq!(group.output_tokens, 200);
            assert_eq!(group.reasoning_output_tokens, 20);
            assert_eq!(group.total_tokens, 1_200);
        }
    }

    #[test]
    fn tracks_long_context_token_split_per_request() {
        let usage_line = |input: u64, cached: u64, output: u64| {
            json!({
                "timestamp": "2026-07-09T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "model": "gpt-5.6-sol",
                        "last_token_usage": {
                            "input_tokens": input,
                            "cached_input_tokens": cached,
                            "output_tokens": output,
                            "reasoning_output_tokens": 0,
                            "total_tokens": input + output,
                        },
                    },
                },
            })
            .to_string()
        };
        // One request above the 272K input threshold and one below it.
        let long_line = usage_line(280_000, 20_000, 500);
        let short_line = usage_line(100_000, 50_000, 300);
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &format!("{long_line}\n{short_line}"),
        });
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };

        let groups = load_groups_from_directory(
            &fixture.path("sessions"),
            &shared,
            AgentReportKind::Daily,
            &PricingMap::default(),
            CodexSpeed::Standard,
        )
        .unwrap();

        let group = groups.get("2026-07-09").unwrap();
        let usage = group.models.get("gpt-5.6-sol").unwrap();
        assert_eq!(usage.input_tokens, 380_000);
        assert_eq!(usage.cached_input_tokens, 70_000);
        assert_eq!(usage.output_tokens, 800);
        assert_eq!(usage.long_context_input_tokens, 280_000);
        assert_eq!(usage.long_context_cached_input_tokens, 20_000);
        assert_eq!(usage.long_context_output_tokens, 500);
    }

    #[test]
    fn dedupes_copied_token_usage_after_model_alias_resolution() {
        let _aliases = set_model_aliases_for_tests([("private-alpha", "gpt-5.2")]);
        let private_usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "private-alpha",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let canonical_usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &private_usage_line,
            "sessions/goal.jsonl": &canonical_usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Daily,
                &PricingMap::default(),
                CodexSpeed::Standard,
            )
            .unwrap();

            let group = groups.get("2026-05-29").unwrap();
            assert_eq!(group.input_tokens, 1_000);
            assert_eq!(group.models.len(), 1);
            assert_eq!(group.models["gpt-5.2"].input_tokens, 1_000);
        }
    }

    #[test]
    fn keeps_matching_token_usage_in_distinct_session_groups() {
        let usage_line = json!({
            "timestamp": "2026-05-29T08:01:00.000Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "gpt-5.2",
                    "last_token_usage": {
                        "input_tokens": 1_000,
                        "cached_input_tokens": 100,
                        "output_tokens": 200,
                        "reasoning_output_tokens": 20,
                        "total_tokens": 1_200,
                    },
                },
            },
        })
        .to_string();
        let fixture = fs_fixture!({
            "sessions/root.jsonl": &usage_line,
            "sessions/goal.jsonl": &usage_line,
        });
        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };

            let groups = load_groups_from_directory(
                &fixture.path("sessions"),
                &shared,
                AgentReportKind::Session,
                &PricingMap::default(),
                CodexSpeed::Standard,
            )
            .unwrap();

            assert_eq!(groups.len(), 2);
            assert_eq!(groups["root"].input_tokens, 1_000);
            assert_eq!(groups["goal"].input_tokens, 1_000);
        }
    }

    #[test]
    fn aggregates_active_copy_when_archived_file_has_same_relative_path() {
        let active_usage = [
            json!({
                "timestamp": "2026-05-12T08:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-12T08:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 111,
                            "cached_input_tokens": 10,
                            "output_tokens": 20,
                            "reasoning_output_tokens": 1,
                            "total_tokens": 131,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let archived_usage = [
            json!({
                "timestamp": "2026-05-12T09:00:00.000Z",
                "type": "turn_context",
                "payload": {
                    "model": "gpt-5.2",
                },
            })
            .to_string(),
            json!({
                "timestamp": "2026-05-12T09:01:00.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {
                            "input_tokens": 999,
                            "cached_input_tokens": 90,
                            "output_tokens": 80,
                            "reasoning_output_tokens": 7,
                            "total_tokens": 1_079,
                        },
                    },
                },
            })
            .to_string(),
        ]
        .join("\n");
        let fixture = fs_fixture!({
            "codex/sessions/duplicate.jsonl": active_usage,
            "codex/archived_sessions/duplicate.jsonl": archived_usage,
            "codex/archived_sessions/archived-only.jsonl": [
                json!({
                    "timestamp": "2026-05-13T08:00:00.000Z",
                    "type": "turn_context",
                    "payload": {
                        "model": "gpt-5.2",
                    },
                })
                .to_string(),
                json!({
                    "timestamp": "2026-05-13T08:01:00.000Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 222,
                                "cached_input_tokens": 20,
                                "output_tokens": 30,
                                "reasoning_output_tokens": 2,
                                "total_tokens": 252,
                            },
                        },
                    },
                })
                .to_string(),
            ]
            .join("\n"),
        });

        for single_thread in [true, false] {
            let shared = SharedArgs {
                single_thread,
                ..SharedArgs::default()
            };
            let sources = vec![
                CodexUsageSource::new_for_test(
                    fixture.path("codex/sessions"),
                    fixture.path("codex"),
                ),
                CodexUsageSource::new_for_test(
                    fixture.path("codex/archived_sessions"),
                    fixture.path("codex"),
                ),
            ];
            let groups = load_groups_from_sources(
                &sources,
                &shared,
                AgentReportKind::Daily,
                &PricingMap::default(),
                CodexSpeed::Standard,
            )
            .unwrap();

            assert_eq!(groups.len(), 2);
            assert_eq!(groups["2026-05-12"].input_tokens, 111);
            assert_eq!(groups["2026-05-13"].input_tokens, 222);
        }
    }
}
