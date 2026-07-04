use std::{collections::BTreeSet, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage, cli::CostMode,
    format_date_tz, format_rfc3339_millis,
};

use super::proto::{ProtoFields, parse_fields};

const USER_STEP_TYPE: i64 = 14;
const PLANNER_STEP_TYPE: i64 = 15;
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
const CHARS_PER_TOKEN: f64 = 3.8;
const INPUT_COST_PER_TOKEN: f64 = 1.50 / 1_000_000.0;
const CACHE_READ_COST_PER_TOKEN: f64 = 0.15 / 1_000_000.0;
const OUTPUT_COST_PER_TOKEN: f64 = 9.00 / 1_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AntigravityStepKind {
    User,
    Planner,
    Context,
}

#[derive(Debug, Clone)]
pub(super) struct AntigravityStep {
    pub(super) idx: i64,
    pub(super) kind: AntigravityStepKind,
    pub(super) timestamp: Option<TimestampMs>,
    pub(super) input_chars: u64,
    pub(super) output_chars: u64,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub(super) struct AntigravityUsageEvent {
    timestamp: TimestampMs,
    session_id: String,
    step_index: i64,
    pub(super) input_chars: u64,
    pub(super) cache_read_chars: u64,
    pub(super) output_chars: u64,
    input_tokens: u64,
    cache_read_tokens: u64,
    output_tokens: u64,
    cost: f64,
}

pub(super) fn parse_step_payload(
    idx: i64,
    step_type: i64,
    payload: &[u8],
    fallback_timestamp: TimestampMs,
) -> AntigravityStep {
    let fields = parse_fields(payload);
    let kind = step_kind(step_type);
    let timestamp = timestamp_from_fields(&fields).or(Some(fallback_timestamp));
    let (input_chars, output_chars) = match kind {
        AntigravityStepKind::User => (sum_exact_paths(&fields, &["19.2"]), 0),
        AntigravityStepKind::Planner => {
            let output_chars = sum_exact_paths(&fields, &["20.3", "20.7.2", "20.7.3"]);
            (0, output_chars)
        }
        AntigravityStepKind::Context => (context_chars(step_type, &fields), 0),
    };
    AntigravityStep {
        idx,
        kind,
        timestamp,
        input_chars,
        output_chars,
    }
}

fn step_kind(step_type: i64) -> AntigravityStepKind {
    match step_type {
        USER_STEP_TYPE => AntigravityStepKind::User,
        PLANNER_STEP_TYPE => AntigravityStepKind::Planner,
        _ => AntigravityStepKind::Context,
    }
}

pub(super) fn estimate_usage_events(
    steps: &[AntigravityStep],
    session_id: &str,
    fallback_timestamp: TimestampMs,
) -> Vec<AntigravityUsageEvent> {
    let mut events = Vec::new();
    let mut context_chars = 0u64;
    let mut new_context_chars = 0u64;
    let mut force_full_uncached = true;

    for step in steps {
        match step.kind {
            AntigravityStepKind::User => {
                context_chars = context_chars.saturating_add(step.input_chars);
                new_context_chars = new_context_chars.saturating_add(step.input_chars);
                force_full_uncached = true;
            }
            AntigravityStepKind::Context => {
                context_chars = context_chars.saturating_add(step.input_chars);
                new_context_chars = new_context_chars.saturating_add(step.input_chars);
            }
            AntigravityStepKind::Planner => {
                let step_input_chars = step.input_chars;
                let prompt_chars = context_chars.saturating_add(step_input_chars);
                let input_chars = if force_full_uncached {
                    prompt_chars
                } else {
                    new_context_chars.saturating_add(step_input_chars)
                };
                let cache_read_chars = if force_full_uncached {
                    0
                } else {
                    prompt_chars.saturating_sub(input_chars)
                };
                if prompt_chars != 0 || step.output_chars != 0 {
                    events.push(usage_event(
                        session_id,
                        step.idx,
                        step.timestamp.unwrap_or(fallback_timestamp),
                        input_chars,
                        cache_read_chars,
                        step.output_chars,
                    ));
                }
                context_chars = prompt_chars.saturating_add(step.output_chars);
                new_context_chars = step.output_chars;
                force_full_uncached = false;
            }
        }
    }

    events
}

fn usage_event(
    session_id: &str,
    step_index: i64,
    timestamp: TimestampMs,
    input_chars: u64,
    cache_read_chars: u64,
    output_chars: u64,
) -> AntigravityUsageEvent {
    let input_tokens = chars_to_tokens(input_chars);
    let cache_read_tokens = chars_to_tokens(cache_read_chars);
    let output_tokens = chars_to_tokens(output_chars);
    let cost = input_tokens as f64 * INPUT_COST_PER_TOKEN
        + cache_read_tokens as f64 * CACHE_READ_COST_PER_TOKEN
        + output_tokens as f64 * OUTPUT_COST_PER_TOKEN;
    AntigravityUsageEvent {
        timestamp,
        session_id: session_id.to_string(),
        step_index,
        input_chars,
        cache_read_chars,
        output_chars,
        input_tokens,
        cache_read_tokens,
        output_tokens,
        cost,
    }
}

fn chars_to_tokens(chars: u64) -> u64 {
    (chars as f64 / CHARS_PER_TOKEN).round() as u64
}

pub(super) fn usage_event_to_loaded(
    event: AntigravityUsageEvent,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: event.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let data = UsageEntry {
        session_id: Some(event.session_id.clone()),
        timestamp: format_rfc3339_millis(event.timestamp),
        version: None,
        message: UsageMessage {
            usage,
            model: Some(DEFAULT_MODEL.to_string()),
            id: Some(format!(
                "antigravity:{}:{}",
                event.session_id, event.step_index
            )),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };
    let cost = match mode {
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => event.cost,
    };
    LoadedEntry {
        date: format_date_tz(event.timestamp, tz),
        timestamp: event.timestamp,
        project: Arc::from("antigravity"),
        session_id: Arc::from(event.session_id.as_str()),
        project_path: Arc::from("Antigravity"),
        cost,
        extra_total_tokens: 0,
        credits: None,
        message_count: Some(1),
        model: Some(DEFAULT_MODEL.to_string()),
        usage_limit_reset_time: None,
        missing_pricing_model: None,
        data,
    }
}

fn context_chars(step_type: i64, fields: &ProtoFields) -> u64 {
    match step_type {
        5 => sum_exact_paths(fields, &["10.23.2", "10.26"]),
        7 => sum_exact_paths(fields, &["13.3"]),
        8 => sum_exact_paths(fields, &["14.1", "14.4"]),
        9 => sum_exact_paths(fields, &["15.1", "15.3"]),
        17 => sum_prefix_paths(fields, &["24.3."]),
        21 => sum_prefix_paths(fields, &["28.21", "28.22"]),
        23 => sum_exact_paths(fields, &["30.4"]),
        33 => sum_exact_paths(fields, &["42.5"]),
        98 => 0,
        101 => {
            let primary = sum_exact_paths(fields, &["114.2.2"]);
            if primary == 0 {
                sum_exact_paths(fields, &["114.1"])
            } else {
                primary
            }
        }
        127 => sum_exact_paths(fields, &["31.2"]),
        132 => sum_exact_paths(fields, &["140.2.1"]),
        138 => sum_exact_paths(fields, &["154.1", "147.2.22"]),
        _ => 0,
    }
}

fn sum_exact_paths(fields: &ProtoFields, paths: &[&str]) -> u64 {
    sum_unique_strings(fields, |path| paths.contains(&path))
}

fn sum_prefix_paths(fields: &ProtoFields, prefixes: &[&str]) -> u64 {
    sum_unique_strings(fields, |path| {
        prefixes.iter().any(|prefix| path.starts_with(prefix))
    })
}

fn sum_unique_strings(fields: &ProtoFields, mut matches_path: impl FnMut(&str) -> bool) -> u64 {
    let mut seen = BTreeSet::new();
    fields
        .strings()
        .iter()
        .filter(|string| matches_path(&string.path))
        .filter(|string| seen.insert(string.value.as_str()))
        .map(|string| string.value.chars().count() as u64)
        .sum()
}

fn timestamp_from_fields(fields: &ProtoFields) -> Option<TimestampMs> {
    for seconds_path in ["5.1.1", "5.6.1", "5.7.1"] {
        let Some(seconds) = fields.varint(seconds_path) else {
            continue;
        };
        let seconds = i64::try_from(seconds).ok()?;
        let nanos_path = seconds_path
            .strip_suffix(".1")
            .map(|prefix| format!("{prefix}.2"))?;
        let nanos = fields.varint(&nanos_path).unwrap_or(0);
        let millis = seconds
            .checked_mul(1_000)?
            .checked_add(i64::try_from(nanos / 1_000_000).ok()?)?;
        return Some(TimestampMs::from_millis(millis));
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::TimestampMs;

    use super::*;

    fn step(
        idx: i64,
        kind: AntigravityStepKind,
        input_chars: u64,
        output_chars: u64,
    ) -> AntigravityStep {
        AntigravityStep {
            idx,
            kind,
            timestamp: Some(TimestampMs::UNIX_EPOCH),
            input_chars,
            output_chars,
        }
    }

    #[test]
    fn resets_cached_context_after_every_user_message() {
        let steps = vec![
            step(0, AntigravityStepKind::User, 100, 0),
            step(1, AntigravityStepKind::Planner, 0, 20),
            step(2, AntigravityStepKind::Context, 30, 0),
            step(3, AntigravityStepKind::Planner, 0, 10),
            step(4, AntigravityStepKind::User, 40, 0),
            step(5, AntigravityStepKind::Planner, 0, 5),
        ];

        let events = estimate_usage_events(&steps, "session-a", TimestampMs::UNIX_EPOCH);

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].input_chars, 100);
        assert_eq!(events[0].cache_read_chars, 0);
        assert_eq!(events[1].input_chars, 50);
        assert_eq!(events[1].cache_read_chars, 100);
        assert_eq!(events[2].input_chars, 200);
        assert_eq!(events[2].cache_read_chars, 0);
        assert_eq!(
            events.iter().map(|event| event.input_chars).sum::<u64>(),
            350
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event.cache_read_chars)
                .sum::<u64>(),
            100
        );
        assert_eq!(
            events.iter().map(|event| event.output_chars).sum::<u64>(),
            35
        );
    }

    #[test]
    fn treats_planner_output_as_uncached_delta_for_the_next_planner_call() {
        let steps = vec![
            step(0, AntigravityStepKind::User, 10, 0),
            step(1, AntigravityStepKind::Planner, 0, 20),
            step(2, AntigravityStepKind::Planner, 0, 1),
        ];

        let events = estimate_usage_events(&steps, "session-a", TimestampMs::UNIX_EPOCH);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].input_chars, 10);
        assert_eq!(events[0].cache_read_chars, 0);
        assert_eq!(events[1].input_chars, 20);
        assert_eq!(events[1].cache_read_chars, 10);
    }

    #[test]
    fn prices_gemini_35_flash_standard_heuristic_rates() {
        let event = usage_event("session-a", 1, TimestampMs::UNIX_EPOCH, 3_800, 7_600, 1_900);

        assert_eq!(event.input_tokens, 1_000);
        assert_eq!(event.cache_read_tokens, 2_000);
        assert_eq!(event.output_tokens, 500);
        assert!((event.cost - 0.0063).abs() < f64::EPSILON);
    }
}
