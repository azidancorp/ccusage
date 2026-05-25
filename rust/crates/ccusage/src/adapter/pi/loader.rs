use std::{collections::HashSet, path::PathBuf};

use crate::{cli::SharedArgs, collect_files_with_extension, parse_tz, LoadedEntry, Result};

use super::{parser, paths};

pub(crate) fn load_entries(
    shared: &SharedArgs,
    custom_path: Option<&str>,
) -> Result<Vec<LoadedEntry>> {
    crate::progress::track_usage_load(crate::progress::UsageLoadAgent::Pi, shared.json, || {
        load_entries_inner(shared, custom_path)
    })
}

fn load_entries_inner(shared: &SharedArgs, custom_path: Option<&str>) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for path in paths::paths(custom_path)? {
        let mut files = Vec::new();
        collect_files_with_extension(&path, "jsonl", &mut files);
        for file in files {
            for entry in parser::read_session_file(&file, tz.as_ref())? {
                let id = parser::entry_id(&entry);
                if seen.insert(id) {
                    entries.push(entry);
                }
            }
        }
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(entries)
}

pub(crate) fn source_files(custom_path: Option<&str>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in paths::paths(custom_path)? {
        collect_files_with_extension(&path, "jsonl", &mut files);
    }
    files.sort();
    files.dedup();
    Ok(files)
}
