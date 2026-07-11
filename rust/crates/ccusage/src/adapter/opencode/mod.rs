pub(crate) mod loader;
mod parser;
mod paths;
mod report;

#[cfg(test)]
pub(crate) use report::report_json;
pub(crate) use report::{
    agent_summary_json, first_column, report_from_rows, summarize_entries, summary_period,
};

use crate::{
    Result, UsageSummary,
    adapter::cache,
    cli::{AgentCommandArgs, AgentReportKind, SharedArgs},
    filter_loaded_entries_by_date, print_json_or_jq, print_usage_table, sort_summaries, wants_json,
};

pub(crate) fn run(args: AgentCommandArgs) -> Result<()> {
    let shared = args.shared;
    let mut rows = load_summaries_with_cache(&shared, args.kind)?;
    sort_summaries(&mut rows, &shared.order, |row| summary_period(row));
    if wants_json(&shared) {
        return print_json_or_jq(
            report_from_rows(&rows, args.kind),
            shared.jq.as_deref(),
            shared.no_cost,
        );
    }
    print_usage_table(
        "OpenCode Token Usage Report",
        first_column(args.kind),
        &rows,
        &shared,
        false,
        None,
    )?;
    Ok(())
}

fn load_summaries_with_cache(
    shared: &SharedArgs,
    kind: AgentReportKind,
) -> Result<Vec<UsageSummary>> {
    let files = loader::source_files()?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let signature =
        cache::create_file_state_signature(&files, &[format!("opencode_kind={kind:?}")]);
    cache::load_usage_summaries_with_cache(
        "opencode",
        kind,
        shared,
        "opencode-summaries-v1",
        &signature,
        || {
            let mut entries = loader::load_entries(shared)?;
            filter_loaded_entries_by_date(&mut entries, shared);
            summarize_entries(&entries, kind)
        },
    )
}
