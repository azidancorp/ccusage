use std::{collections::BTreeMap, env, path::PathBuf, sync::mpsc, thread};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    adapter::{
        amp, antigravity, claude, codebuff, codex, copilot, droid, gemini, goose, hermes, kilo,
        kimi, openclaw, opencode, pi, qwen,
    },
    cli::{AgentReportKind, CodexSpeed, SharedArgs, WeekDay},
    filter_loaded_entries_by_date, json_float, summarize_by_key, summarize_summaries_by_bucket,
    BucketKind, CodexGroup, LoadedEntry, ModelBreakdown, PricingMap, Result, SessionAccumulator,
    UsageSummary,
};

use super::{
    report::sort_rows,
    types::{AgentLoadSpec, AgentRows, AllAccumulator, AllLoadResult, AllRow, LoadedAgentRows},
};

pub(super) fn load_rows(kind: AgentReportKind, shared: &SharedArgs) -> Result<AllLoadResult> {
    let mut progress = crate::progress::UsageLoadProgress::new(
        crate::log_level() != Some(0)
            && crate::progress::should_show_usage_load_progress(
                shared.json,
                crate::progress::usage_load_output_is_tty(),
            ),
    );
    let pricing = PricingMap::load(shared.offline, crate::log_level() != Some(0));
    let load_kind = match kind {
        AgentReportKind::Session => AgentReportKind::Session,
        AgentReportKind::Daily | AgentReportKind::Weekly | AgentReportKind::Monthly => {
            AgentReportKind::Daily
        }
    };
    let loader_shared = SharedArgs {
        json: true,
        ..shared.clone()
    };
    let loaded = load_agent_rows_parallel(
        vec![
            AgentLoadSpec {
                index: 0,
                agent: "claude",
                progress_agent: crate::progress::UsageLoadAgent::Claude,
                load: Box::new(|| {
                    load_agent_rows_cached("claude", load_kind, &loader_shared, || {
                        load_session_capable_summary_agent_rows(
                            "claude",
                            load_kind,
                            &loader_shared,
                            claude::load_entries,
                            summarize_entries,
                        )
                    })
                }),
            },
            AgentLoadSpec {
                index: 1,
                agent: "codex",
                progress_agent: crate::progress::UsageLoadAgent::Codex,
                load: Box::new(|| {
                    load_agent_rows_cached("codex", load_kind, &loader_shared, || {
                        load_codex_rows(load_kind, &loader_shared, &pricing)
                    })
                }),
            },
            AgentLoadSpec {
                index: 2,
                agent: "opencode",
                progress_agent: crate::progress::UsageLoadAgent::OpenCode,
                load: Box::new(|| {
                    load_agent_rows_cached("opencode", load_kind, &loader_shared, || {
                        load_summary_agent_rows(
                            "opencode",
                            load_kind,
                            &loader_shared,
                            || opencode::loader::load_entries(&loader_shared),
                            opencode::summarize_entries,
                        )
                    })
                }),
            },
            AgentLoadSpec {
                index: 3,
                agent: "amp",
                progress_agent: crate::progress::UsageLoadAgent::Amp,
                load: Box::new(|| {
                    load_agent_rows_cached("amp", load_kind, &loader_shared, || {
                        load_priced_summary_agent_rows(
                            "amp",
                            load_kind,
                            &loader_shared,
                            &pricing,
                            amp::load_entries,
                            amp::summarize_entries,
                        )
                    })
                }),
            },
            AgentLoadSpec {
                index: 4,
                agent: "droid",
                progress_agent: crate::progress::UsageLoadAgent::Droid,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "droid",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        droid::load_entries,
                        droid::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 5,
                agent: "codebuff",
                progress_agent: crate::progress::UsageLoadAgent::Codebuff,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "codebuff",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        codebuff::load_entries,
                        codebuff::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 6,
                agent: "hermes",
                progress_agent: crate::progress::UsageLoadAgent::Hermes,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "hermes",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        hermes::load_entries,
                        hermes::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 7,
                agent: "pi",
                progress_agent: crate::progress::UsageLoadAgent::Pi,
                load: Box::new(|| {
                    load_agent_rows_cached("pi", load_kind, &loader_shared, || {
                        load_session_capable_summary_agent_rows(
                            "pi",
                            load_kind,
                            &loader_shared,
                            pi::load_entries,
                            pi::summarize_entries,
                        )
                    })
                }),
            },
            AgentLoadSpec {
                index: 8,
                agent: "goose",
                progress_agent: crate::progress::UsageLoadAgent::Goose,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "goose",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        goose::load_entries,
                        goose::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 9,
                agent: "openclaw",
                progress_agent: crate::progress::UsageLoadAgent::OpenClaw,
                load: Box::new(|| {
                    load_summary_agent_rows(
                        "openclaw",
                        load_kind,
                        &loader_shared,
                        || openclaw::load_entries(&loader_shared, None),
                        openclaw::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 10,
                agent: "kilo",
                progress_agent: crate::progress::UsageLoadAgent::Kilo,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "kilo",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        kilo::load_entries,
                        kilo::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 11,
                agent: "copilot",
                progress_agent: crate::progress::UsageLoadAgent::Copilot,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "copilot",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        copilot::load_entries,
                        copilot::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 12,
                agent: "gemini",
                progress_agent: crate::progress::UsageLoadAgent::Gemini,
                load: Box::new(|| {
                    load_priced_summary_agent_rows(
                        "gemini",
                        load_kind,
                        &loader_shared,
                        &pricing,
                        gemini::load_entries,
                        gemini::summarize_entries,
                    )
                }),
            },
            AgentLoadSpec {
                index: 13,
                agent: "antigravity",
                progress_agent: crate::progress::UsageLoadAgent::Antigravity,
                load: Box::new(|| {
                    load_agent_rows_cached("antigravity", load_kind, &loader_shared, || {
                        load_priced_summary_agent_rows(
                            "antigravity",
                            load_kind,
                            &loader_shared,
                            &pricing,
                            antigravity::load_entries,
                            antigravity::summarize_entries,
                        )
                    })
                }),
            },
            AgentLoadSpec {
                index: 14,
                agent: "kimi",
                progress_agent: crate::progress::UsageLoadAgent::Kimi,
                load: Box::new(|| {
                    load_agent_rows_cached("kimi", load_kind, &loader_shared, || {
                        load_priced_summary_agent_rows(
                            "kimi",
                            load_kind,
                            &loader_shared,
                            &pricing,
                            kimi::load_entries,
                            kimi::summarize_entries,
                        )
                    })
                }),
            },
            AgentLoadSpec {
                index: 15,
                agent: "qwen",
                progress_agent: crate::progress::UsageLoadAgent::Qwen,
                load: Box::new(|| load_qwen_rows(load_kind, &loader_shared)),
            },
        ],
        &mut progress,
    )?;
    let mut detected_agents = Vec::new();
    let mut rows = Vec::new();
    for loaded in loaded {
        append_agent_rows(
            &mut rows,
            &mut detected_agents,
            loaded.agent,
            loaded.agent_rows,
        );
    }
    if kind == AgentReportKind::Session {
        for row in &mut rows {
            row.metadata_agents = None;
        }
        sort_rows(&mut rows, &shared.order);
        return Ok(AllLoadResult {
            rows,
            detected_agents,
        });
    }

    let mut aggregated = aggregate_rows(rows, kind);
    sort_rows(&mut aggregated, &shared.order);
    Ok(AllLoadResult {
        rows: aggregated,
        detected_agents,
    })
}

pub(super) fn load_agent_rows_parallel(
    specs: Vec<AgentLoadSpec<'_>>,
    progress: &mut crate::progress::UsageLoadProgress,
) -> Result<Vec<LoadedAgentRows>> {
    for spec in &specs {
        progress.start(spec.progress_agent);
    }

    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let mut handles = Vec::with_capacity(specs.len());
        for spec in specs {
            let sender = sender.clone();
            handles.push((
                spec.index,
                spec.progress_agent,
                scope.spawn(move || {
                    let result = (spec.load)();
                    let _ = sender.send((spec.index, spec.agent, spec.progress_agent, result));
                }),
            ));
        }
        drop(sender);

        let mut loaded = Vec::with_capacity(handles.len());
        let mut errors = Vec::new();
        for (index, agent, progress_agent, result) in receiver {
            match result {
                Ok(agent_rows) => {
                    progress.succeed(progress_agent);
                    loaded.push(LoadedAgentRows {
                        index,
                        agent,
                        agent_rows,
                    });
                }
                Err(error) => {
                    progress.fail(progress_agent);
                    errors.push((index, error));
                }
            }
        }

        for (index, progress_agent, handle) in handles {
            if handle.join().is_err() {
                progress.fail(progress_agent);
                errors.push((index, crate::cli_error("agent loader panicked")));
            }
        }

        errors.sort_by_key(|(index, _)| *index);
        if let Some((_, error)) = errors.into_iter().next() {
            return Err(error);
        }

        loaded.sort_by_key(|loaded| loaded.index);
        Ok(loaded)
    })
}

fn append_agent_rows(
    rows: &mut Vec<AllRow>,
    detected_agents: &mut Vec<&'static str>,
    agent: &'static str,
    agent_rows: AgentRows,
) {
    if agent_rows.detected {
        detected_agents.push(agent);
    }
    rows.extend(agent_rows.rows);
}

#[derive(Serialize, Deserialize)]
struct CachedAgentRows {
    rows: Vec<CachedAllRow>,
    detected: bool,
}

#[derive(Serialize, Deserialize)]
struct CachedAllRow {
    period: String,
    agent: String,
    models_used: Vec<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    total_tokens: u64,
    total_cost_bits: u64,
    metadata: Option<Value>,
    metadata_agents: Option<Vec<String>>,
    agent_breakdowns: Option<Vec<CachedAllRow>>,
    model_breakdowns: Vec<CachedModelBreakdown>,
}

#[derive(Serialize, Deserialize)]
struct CachedModelBreakdown {
    model_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    extra_total_tokens: u64,
    cost_bits: u64,
}

fn load_agent_rows_cached(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    load_rows: impl FnOnce() -> Result<AgentRows>,
) -> Result<AgentRows> {
    let Some(signature) = agent_source_signature(agent)? else {
        return load_rows();
    };
    let cached = crate::adapter::cache::load_source_value_with_cache(
        agent,
        kind,
        shared,
        "all-agent-rows-v3",
        &signature,
        || load_rows().map(CachedAgentRows::from),
    )?;
    Ok(cached.into())
}

fn agent_source_signature(agent: &str) -> Result<Option<String>> {
    let mut extra_values = Vec::new();
    let files = match agent {
        "claude" => claude_source_files()?,
        "codex" => codex::source_files()?,
        "opencode" => opencode::loader::source_files()?,
        "amp" => amp::source_files()?,
        "antigravity" => antigravity::source_files()?,
        "pi" => pi::source_files(None)?,
        "kimi" => {
            extra_values.push(format!(
                "KIMI_MODEL_NAME={}",
                env::var("KIMI_MODEL_NAME").unwrap_or_default()
            ));
            kimi::source_files()?
        }
        _ => return Ok(None),
    };
    Ok(Some(crate::adapter::cache::create_file_state_signature(
        &files,
        &extra_values,
    )))
}

fn claude_source_files() -> Result<Vec<PathBuf>> {
    let paths = claude::claude_paths()?;
    Ok(claude::usage_files(&paths, None))
}

impl From<AgentRows> for CachedAgentRows {
    fn from(rows: AgentRows) -> Self {
        Self {
            detected: rows.detected,
            rows: rows.rows.iter().map(CachedAllRow::from).collect(),
        }
    }
}

impl From<CachedAgentRows> for AgentRows {
    fn from(rows: CachedAgentRows) -> Self {
        Self {
            detected: rows.detected,
            rows: rows.rows.into_iter().map(AllRow::from).collect(),
        }
    }
}

impl From<&AllRow> for CachedAllRow {
    fn from(row: &AllRow) -> Self {
        Self {
            period: row.period.clone(),
            agent: row.agent.to_string(),
            models_used: row.models_used.clone(),
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            total_tokens: row.total_tokens,
            total_cost_bits: row.total_cost.to_bits(),
            metadata: row.metadata.clone(),
            metadata_agents: row
                .metadata_agents
                .as_ref()
                .map(|agents| agents.iter().map(|agent| (*agent).to_string()).collect()),
            agent_breakdowns: row.agent_breakdowns.as_ref().map(|rows| {
                rows.iter()
                    .map(CachedAllRow::from)
                    .collect::<Vec<CachedAllRow>>()
            }),
            model_breakdowns: row
                .model_breakdowns
                .iter()
                .map(CachedModelBreakdown::from)
                .collect(),
        }
    }
}

impl From<CachedAllRow> for AllRow {
    fn from(row: CachedAllRow) -> Self {
        Self {
            period: row.period,
            agent: owned_agent_to_static(row.agent),
            models_used: row.models_used,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            total_tokens: row.total_tokens,
            total_cost: f64::from_bits(row.total_cost_bits),
            metadata: row.metadata,
            metadata_agents: row.metadata_agents.map(|agents| {
                agents
                    .into_iter()
                    .map(owned_agent_to_static)
                    .collect::<Vec<&'static str>>()
            }),
            agent_breakdowns: row
                .agent_breakdowns
                .map(|rows| rows.into_iter().map(AllRow::from).collect()),
            model_breakdowns: row
                .model_breakdowns
                .into_iter()
                .map(ModelBreakdown::from)
                .collect(),
        }
    }
}

impl From<&ModelBreakdown> for CachedModelBreakdown {
    fn from(model: &ModelBreakdown) -> Self {
        Self {
            model_name: model.model_name.clone(),
            input_tokens: model.input_tokens,
            output_tokens: model.output_tokens,
            cache_creation_tokens: model.cache_creation_tokens,
            cache_read_tokens: model.cache_read_tokens,
            extra_total_tokens: model.extra_total_tokens,
            cost_bits: model.cost.to_bits(),
        }
    }
}

impl From<CachedModelBreakdown> for ModelBreakdown {
    fn from(model: CachedModelBreakdown) -> Self {
        Self {
            model_name: model.model_name,
            input_tokens: model.input_tokens,
            output_tokens: model.output_tokens,
            cache_creation_tokens: model.cache_creation_tokens,
            cache_read_tokens: model.cache_read_tokens,
            extra_total_tokens: model.extra_total_tokens,
            cost: f64::from_bits(model.cost_bits),
        }
    }
}

fn owned_agent_to_static(agent: String) -> &'static str {
    match agent.as_str() {
        "all" => "all",
        "claude" => "claude",
        "codex" => "codex",
        "opencode" => "opencode",
        "amp" => "amp",
        "droid" => "droid",
        "codebuff" => "codebuff",
        "hermes" => "hermes",
        "pi" => "pi",
        "goose" => "goose",
        "openclaw" => "openclaw",
        "kilo" => "kilo",
        "copilot" => "copilot",
        "gemini" => "gemini",
        "antigravity" => "antigravity",
        "kimi" => "kimi",
        "qwen" => "qwen",
        _ => Box::leak(agent.into_boxed_str()),
    }
}

fn load_summary_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    load_entries: impl FnOnce() -> Result<Vec<LoadedEntry>>,
    summarize_entries: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<AgentRows> {
    let mut entries = load_entries()?;
    let detected = !entries.is_empty();
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = summarize_entries(&entries, kind)?;
    Ok(AgentRows {
        rows: summary_rows(agent, summaries),
        detected,
    })
}

fn load_session_capable_summary_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    load_entries: impl FnOnce(&SharedArgs, Option<&str>) -> Result<Vec<LoadedEntry>>,
    summarize_entries: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<AgentRows> {
    let mut entries = load_entries(shared, None)?;
    let detected = !entries.is_empty();
    let summaries = if kind == AgentReportKind::Session {
        let mut summaries = summarize_entry_sessions(&entries, shared.timezone.as_deref())?;
        filter_session_summaries(&mut summaries, shared);
        summaries
    } else {
        filter_loaded_entries_by_date(&mut entries, shared);
        summarize_entries(&entries, kind)?
    };
    Ok(AgentRows {
        rows: summary_rows(agent, summaries),
        detected,
    })
}

fn load_codex_rows(
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
) -> Result<AgentRows> {
    let mut events = codex::load_codex_events(shared)?;
    let detected = !events.is_empty();
    codex::filter_events_by_date(&mut events, shared)?;
    let speed = CodexSpeed::Auto;
    let groups =
        codex::aggregate_events(&events, kind, shared.timezone.as_deref(), pricing, speed)?;
    Ok(AgentRows {
        rows: groups
            .iter()
            .map(|(period, group)| codex_group_row(period, group, pricing, speed))
            .collect(),
        detected,
    })
}

fn load_priced_summary_agent_rows(
    agent: &'static str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    pricing: &PricingMap,
    load_entries: impl FnOnce(&SharedArgs, &PricingMap) -> Result<Vec<LoadedEntry>>,
    summarize_entries: impl FnOnce(&[LoadedEntry], AgentReportKind) -> Result<Vec<UsageSummary>>,
) -> Result<AgentRows> {
    load_summary_agent_rows(
        agent,
        kind,
        shared,
        || load_entries(shared, pricing),
        summarize_entries,
    )
}

fn load_qwen_rows(kind: AgentReportKind, shared: &SharedArgs) -> Result<AgentRows> {
    let mut entries = qwen::load_entries(shared)?;
    let detected = !entries.is_empty() || qwen::has_data();
    if kind == AgentReportKind::Session {
        let mut summaries = qwen::summarize_entries(&entries, kind)?;
        filter_session_summaries(&mut summaries, shared);
        return Ok(AgentRows {
            rows: summary_rows("qwen", summaries),
            detected,
        });
    }
    filter_loaded_entries_by_date(&mut entries, shared);
    let summaries = qwen::summarize_entries(&entries, kind)?;
    Ok(AgentRows {
        rows: summary_rows("qwen", summaries),
        detected,
    })
}

fn summarize_entries(entries: &[LoadedEntry], kind: AgentReportKind) -> Result<Vec<UsageSummary>> {
    match kind {
        AgentReportKind::Daily => summarize_by_key(
            entries,
            |entry| entry.date.clone(),
            |date| (date.to_string(), None),
        ),
        AgentReportKind::Monthly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Monthly,
                WeekDay::Sunday,
            ))
        }
        AgentReportKind::Weekly => {
            let daily = summarize_entries(entries, AgentReportKind::Daily)?;
            Ok(summarize_summaries_by_bucket(
                &daily,
                BucketKind::Weekly,
                WeekDay::Monday,
            ))
        }
        AgentReportKind::Session => summarize_by_key(
            entries,
            |entry| entry.session_id.to_string(),
            |session_id| (session_id.to_string(), None),
        )
        .map(|mut rows| {
            for row in &mut rows {
                row.session_id = row.date.take();
            }
            rows
        }),
    }
}

fn summarize_entry_sessions(
    entries: &[LoadedEntry],
    timezone: Option<&str>,
) -> Result<Vec<UsageSummary>> {
    let mut groups = BTreeMap::<(String, String), SessionAccumulator>::new();
    for entry in entries {
        groups
            .entry((entry.project_path.to_string(), entry.session_id.to_string()))
            .or_default()
            .add_entry(entry);
    }
    groups
        .into_values()
        .map(|group| group.into_summary(timezone))
        .collect()
}

fn filter_session_summaries(rows: &mut Vec<UsageSummary>, shared: &SharedArgs) {
    if shared.since.is_some() || shared.until.is_some() {
        rows.retain(|row| {
            let date = row
                .last_activity
                .as_deref()
                .unwrap_or_default()
                .replace('-', "");
            shared.since.as_ref().is_none_or(|since| &date >= since)
                && shared.until.as_ref().is_none_or(|until| &date <= until)
        });
    }
}

fn summary_rows(agent: &'static str, summaries: Vec<UsageSummary>) -> Vec<AllRow> {
    summaries
        .into_iter()
        .filter_map(|summary| {
            let period = summary
                .date
                .as_ref()
                .or(summary.week.as_ref())
                .or(summary.month.as_ref())
                .or(summary.session_id.as_ref())?
                .clone();
            let total_tokens = summary.total_tokens();
            if total_tokens == 0 {
                return None;
            }
            let metadata = summary_metadata(agent, &summary);
            Some(AllRow {
                period,
                agent,
                models_used: summary.models_used,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                cache_creation_tokens: summary.cache_creation_tokens,
                cache_read_tokens: summary.cache_read_tokens,
                total_tokens,
                total_cost: summary.total_cost,
                metadata,
                metadata_agents: Some(vec![agent]),
                agent_breakdowns: None,
                model_breakdowns: summary.model_breakdowns,
            })
        })
        .collect()
}

fn summary_metadata(agent: &'static str, summary: &UsageSummary) -> Option<Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(credits) = summary.credits {
        metadata.insert("credits".to_string(), json_float(credits));
    }
    if summary.session_id.is_some() {
        if let Some(last_activity) = summary.last_activity.as_ref() {
            metadata.insert("lastActivity".to_string(), json!(last_activity));
        }
        if agent == "pi" {
            if let Some(project_path) = summary.project_path.as_ref() {
                metadata.insert("projectPath".to_string(), json!(project_path));
            }
        }
    }
    if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    }
}

pub(super) fn codex_group_row(
    period: &str,
    group: &CodexGroup,
    pricing: &PricingMap,
    speed: CodexSpeed,
) -> AllRow {
    let mut model_breakdowns: Vec<ModelBreakdown> = group
        .models
        .iter()
        .map(|(model, usage)| {
            let input =
                codex::non_cached_input_tokens(usage.input_tokens, usage.cached_input_tokens);
            ModelBreakdown {
                model_name: model.clone(),
                input_tokens: input,
                output_tokens: usage.output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: usage.cached_input_tokens,
                extra_total_tokens: 0,
                cost: codex::calculate_codex_model_cost(model, usage, pricing, speed),
            }
        })
        .collect();
    model_breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    AllRow {
        period: period.to_string(),
        agent: "codex",
        models_used: group.models.keys().cloned().collect(),
        input_tokens: codex::non_cached_input_tokens(group.input_tokens, group.cached_input_tokens),
        output_tokens: group.output_tokens,
        cache_creation_tokens: 0,
        cache_read_tokens: group.cached_input_tokens,
        total_tokens: group.total_tokens,
        total_cost: codex::calculate_group_cost(group, pricing, speed),
        metadata: Some(json!({
            "lastActivity": group.last_activity,
            "reasoningOutputTokens": group.reasoning_output_tokens,
        })),
        metadata_agents: Some(vec!["codex"]),
        agent_breakdowns: None,
        model_breakdowns,
    }
}

pub(super) fn aggregate_rows(rows: Vec<AllRow>, kind: AgentReportKind) -> Vec<AllRow> {
    let mut groups = BTreeMap::<String, AllAccumulator>::new();
    for mut row in rows {
        let period = match kind {
            AgentReportKind::Daily => row.period.clone(),
            AgentReportKind::Monthly => row
                .period
                .get(..7)
                .map_or_else(|| row.period.clone(), str::to_string),
            AgentReportKind::Weekly => crate::week_start(&row.period, WeekDay::Monday)
                .unwrap_or_else(|| row.period.clone()),
            AgentReportKind::Session => row.period.clone(),
        };
        row.period = period.clone();
        groups.entry(period).or_default().add(row);
    }
    groups
        .into_iter()
        .map(|(period, group)| group.into_row(period))
        .collect()
}
