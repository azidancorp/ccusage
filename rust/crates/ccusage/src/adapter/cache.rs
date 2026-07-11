use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    env, fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    ModelBreakdown, Result, UsageSummary,
    cli::{AgentReportKind, SharedArgs},
};

const SOURCE_VALUE_CACHE_VERSION: u32 = 1;
const FILE_ROWS_CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileState {
    pub(crate) size: u64,
    pub(crate) modified_secs: u64,
    pub(crate) modified_nanos: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceValueCacheEntry<T> {
    version: u32,
    signature: String,
    value: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRowsCacheFileEntry<T> {
    state: FileState,
    rows: Vec<T>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileRowsCacheEntry<T> {
    version: u32,
    files: BTreeMap<String, FileRowsCacheFileEntry<T>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedUsageSummary {
    date: Option<String>,
    month: Option<String>,
    week: Option<String>,
    session_id: Option<String>,
    project_path: Option<String>,
    last_activity: Option<String>,
    first_activity: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    extra_total_tokens: u64,
    total_cost_bits: u64,
    credits_bits: Option<u64>,
    message_count: Option<u64>,
    models_used: Vec<String>,
    model_breakdowns: Vec<CachedModelBreakdown>,
    project: Option<String>,
    versions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedModelBreakdown {
    model_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    extra_total_tokens: u64,
    cost_bits: u64,
    missing_pricing: bool,
}

pub(crate) fn create_file_state_signature(files: &[PathBuf], extra_values: &[String]) -> String {
    let mut states = files
        .iter()
        .map(|file| {
            let path = file.to_string_lossy();
            match file_state(file) {
                Some(state) => format!(
                    "file:{path}|{}|{}|{}",
                    state.size, state.modified_secs, state.modified_nanos
                ),
                None => format!("file:{path}|missing"),
            }
        })
        .collect::<Vec<_>>();
    states.sort();
    states.dedup();

    let mut extras = extra_values
        .iter()
        .map(|value| format!("extra:{value}"))
        .collect::<Vec<_>>();
    extras.sort();

    hash_text(&[states.join("\n"), extras.join("\n")].join("\n"))
}

pub(crate) fn load_source_value_with_cache<T>(
    agent: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    cache_discriminator: &str,
    signature: &str,
    load_value: impl FnOnce() -> Result<T>,
) -> Result<T>
where
    T: Serialize + DeserializeOwned,
{
    let Some(cache_path) = source_value_cache_path(agent, kind, shared, cache_discriminator) else {
        return load_value();
    };
    if let Some(value) = read_source_value_cache(&cache_path, signature) {
        return Ok(value);
    }

    let value = load_value()?;
    write_source_value_cache(&cache_path, signature, &value);
    Ok(value)
}

pub(crate) fn read_source_value_cache_entry<T>(
    agent: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    cache_discriminator: &str,
) -> Option<(String, T)>
where
    T: DeserializeOwned,
{
    let cache_path = source_value_cache_path(agent, kind, shared, cache_discriminator)?;
    let raw = fs::read_to_string(cache_path).ok()?;
    let parsed = serde_json::from_str::<SourceValueCacheEntry<T>>(&raw).ok()?;
    (parsed.version == SOURCE_VALUE_CACHE_VERSION).then_some((parsed.signature, parsed.value))
}

pub(crate) fn write_source_value_cache_entry<T>(
    agent: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    cache_discriminator: &str,
    signature: &str,
    value: &T,
) where
    T: Serialize,
{
    let Some(cache_path) = source_value_cache_path(agent, kind, shared, cache_discriminator) else {
        return;
    };
    write_source_value_cache(&cache_path, signature, value);
}

pub(crate) fn load_usage_summaries_with_cache(
    agent: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    cache_discriminator: &str,
    signature: &str,
    load_summaries: impl FnOnce() -> Result<Vec<UsageSummary>>,
) -> Result<Vec<UsageSummary>> {
    let cached =
        load_source_value_with_cache(agent, kind, shared, cache_discriminator, signature, || {
            load_summaries().map(|rows| {
                rows.into_iter()
                    .map(CachedUsageSummary::from)
                    .collect::<Vec<_>>()
            })
        })?;
    Ok(cached.into_iter().map(UsageSummary::from).collect())
}

pub(crate) fn load_file_rows_with_cache<T>(
    agent: &str,
    cache_discriminator: &str,
    files: &[PathBuf],
    load_rows_for_files: impl FnOnce(&[PathBuf]) -> Vec<Vec<T>>,
) -> Vec<T>
where
    T: Clone + Serialize + DeserializeOwned,
{
    let mut sorted_files = files.to_vec();
    sorted_files.sort();
    sorted_files.dedup();

    let Some(cache_path) = file_rows_cache_path(agent, cache_discriminator) else {
        return load_rows_for_files(&sorted_files)
            .into_iter()
            .flatten()
            .collect();
    };

    let cached_files = read_file_rows_cache::<T>(&cache_path);
    let file_states = sorted_files
        .iter()
        .filter_map(|file| file_state(file).map(|state| (file.clone(), state)))
        .collect::<Vec<_>>();
    let missed_files = file_states
        .iter()
        .filter_map(|(file, state)| {
            let key = cache_file_key(file);
            let cached = cached_files.get(&key)?;
            (cached.state != *state).then(|| file.clone())
        })
        .chain(file_states.iter().filter_map(|(file, _)| {
            let key = cache_file_key(file);
            (!cached_files.contains_key(&key)).then(|| file.clone())
        }))
        .collect::<Vec<_>>();

    let loaded_misses = if missed_files.is_empty() {
        Vec::new()
    } else {
        load_rows_for_files(&missed_files)
    };
    let mut loaded_by_file = missed_files
        .into_iter()
        .zip(loaded_misses)
        .map(|(file, rows)| (cache_file_key(&file), rows))
        .collect::<BTreeMap<_, _>>();

    let mut next_files = BTreeMap::new();
    let mut rows = Vec::new();
    for (file, state) in file_states {
        let key = cache_file_key(&file);
        if let Some(cached) = cached_files
            .get(&key)
            .filter(|cached| cached.state == state)
            .cloned()
        {
            rows.extend(cached.rows.clone());
            next_files.insert(key, cached);
            continue;
        }

        let loaded_rows = loaded_by_file.remove(&key).unwrap_or_default();
        rows.extend(loaded_rows.clone());
        next_files.insert(
            key,
            FileRowsCacheFileEntry {
                state,
                rows: loaded_rows,
            },
        );
    }

    if next_files.len() != cached_files.len()
        || next_files.keys().any(|key| {
            cached_files.get(key).is_none_or(|cached| {
                next_files
                    .get(key)
                    .is_some_and(|next| next.state != cached.state)
            })
        })
    {
        write_file_rows_cache(&cache_path, &next_files);
    }

    rows
}

fn source_value_cache_path(
    agent: &str,
    kind: AgentReportKind,
    shared: &SharedArgs,
    cache_discriminator: &str,
) -> Option<PathBuf> {
    let key = hash_text(&format!(
        "agent={agent}\nkind={kind:?}\ndiscriminator={cache_discriminator}\n{}",
        stable_shared_options(shared)
    ));
    usage_cache_dir().map(|dir| dir.join(format!("{agent}-{kind:?}-{key}.json")))
}

fn file_rows_cache_path(agent: &str, cache_discriminator: &str) -> Option<PathBuf> {
    let key = hash_text(&format!(
        "agent={agent}\ndiscriminator={cache_discriminator}"
    ));
    usage_cache_dir().map(|dir| dir.join(format!("{agent}-files-{key}.json")))
}

fn stable_shared_options(shared: &SharedArgs) -> String {
    let model_aliases = env::var_os("CCUSAGE_MODEL_ALIASES");
    let timezone = shared.timezone.clone().unwrap_or_else(|| {
        JiffTimeZone::system()
            .iana_name()
            .unwrap_or("system-unknown")
            .to_string()
    });
    format!(
        "mode={:?}\noffline={}\nno_cost={}\nsince={:?}\nuntil={:?}\ntimezone={timezone:?}\npricing_overrides={:?}\nmodel_aliases={model_aliases:?}",
        shared.mode,
        shared.offline,
        shared.no_cost,
        shared.since,
        shared.until,
        shared.pricing_overrides,
    )
}

fn usage_cache_dir() -> Option<PathBuf> {
    #[cfg(test)]
    let _ = env::var_os("XDG_CACHE_HOME")?;

    let root = env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(".cache")))?;
    Some(root.join("ccusage").join("usage-rust"))
}

fn read_source_value_cache<T>(cache_path: &Path, signature: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    let raw = fs::read_to_string(cache_path).ok()?;
    let parsed = serde_json::from_str::<SourceValueCacheEntry<T>>(&raw).ok()?;
    if parsed.version == SOURCE_VALUE_CACHE_VERSION && parsed.signature == signature {
        Some(parsed.value)
    } else {
        None
    }
}

fn write_source_value_cache<T>(cache_path: &Path, signature: &str, value: &T)
where
    T: Serialize,
{
    let entry = SourceValueCacheEntry {
        version: SOURCE_VALUE_CACHE_VERSION,
        signature: signature.to_string(),
        value,
    };
    let Ok(raw) = serde_json::to_vec(&entry) else {
        return;
    };
    write_cache_file(cache_path, &raw);
}

fn read_file_rows_cache<T>(cache_path: &Path) -> BTreeMap<String, FileRowsCacheFileEntry<T>>
where
    T: DeserializeOwned,
{
    let Ok(raw) = fs::read_to_string(cache_path) else {
        return BTreeMap::new();
    };
    let Ok(parsed) = serde_json::from_str::<FileRowsCacheEntry<T>>(&raw) else {
        return BTreeMap::new();
    };
    if parsed.version == FILE_ROWS_CACHE_VERSION {
        parsed.files
    } else {
        BTreeMap::new()
    }
}

fn write_file_rows_cache<T>(cache_path: &Path, files: &BTreeMap<String, FileRowsCacheFileEntry<T>>)
where
    T: Clone + Serialize,
{
    let entry = FileRowsCacheEntry {
        version: FILE_ROWS_CACHE_VERSION,
        files: files.clone(),
    };
    let Ok(raw) = serde_json::to_vec(&entry) else {
        return;
    };
    write_cache_file(cache_path, &raw);
}

fn write_cache_file(cache_path: &Path, raw: &[u8]) {
    if let Some(parent) = cache_path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let _ = fs::write(cache_path, raw);
}

fn cache_file_key(file: &Path) -> String {
    file.to_string_lossy().into_owned()
}

pub(crate) fn file_state(file: &Path) -> Option<FileState> {
    let metadata = fs::metadata(file).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(FileState {
        size: metadata.len(),
        modified_secs: modified.as_secs(),
        modified_nanos: modified.subsec_nanos(),
    })
}

fn hash_text(value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

impl From<UsageSummary> for CachedUsageSummary {
    fn from(row: UsageSummary) -> Self {
        Self {
            date: row.date,
            month: row.month,
            week: row.week,
            session_id: row.session_id,
            project_path: row.project_path,
            last_activity: row.last_activity,
            first_activity: row.first_activity,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            extra_total_tokens: row.extra_total_tokens,
            total_cost_bits: row.total_cost.to_bits(),
            credits_bits: row.credits.map(f64::to_bits),
            message_count: row.message_count,
            models_used: row.models_used,
            model_breakdowns: row
                .model_breakdowns
                .into_iter()
                .map(CachedModelBreakdown::from)
                .collect(),
            project: row.project,
            versions: row.versions,
        }
    }
}

impl From<CachedUsageSummary> for UsageSummary {
    fn from(row: CachedUsageSummary) -> Self {
        Self {
            date: row.date,
            month: row.month,
            week: row.week,
            session_id: row.session_id,
            project_path: row.project_path,
            last_activity: row.last_activity,
            first_activity: row.first_activity,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_creation_tokens: row.cache_creation_tokens,
            cache_read_tokens: row.cache_read_tokens,
            extra_total_tokens: row.extra_total_tokens,
            total_cost: f64::from_bits(row.total_cost_bits),
            credits: row.credits_bits.map(f64::from_bits),
            message_count: row.message_count,
            models_used: row.models_used,
            model_breakdowns: row
                .model_breakdowns
                .into_iter()
                .map(ModelBreakdown::from)
                .collect(),
            project: row.project,
            versions: row.versions,
        }
    }
}

impl From<ModelBreakdown> for CachedModelBreakdown {
    fn from(model: ModelBreakdown) -> Self {
        Self {
            model_name: model.model_name,
            input_tokens: model.input_tokens,
            output_tokens: model.output_tokens,
            cache_creation_tokens: model.cache_creation_tokens,
            cache_read_tokens: model.cache_read_tokens,
            extra_total_tokens: model.extra_total_tokens,
            cost_bits: model.cost.to_bits(),
            missing_pricing: model.missing_pricing,
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
            missing_pricing: model.missing_pricing,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString};

    use serde::{Deserialize, Serialize};

    use crate::cli::{AgentReportKind, SharedArgs};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestRow {
        content: String,
    }

    #[test]
    fn stable_options_include_effective_system_timezone() {
        let shared = SharedArgs::default();
        let system = jiff::tz::TimeZone::system();
        let system_timezone = system.iana_name().unwrap_or("system-unknown");

        let options = super::stable_shared_options(&shared);

        assert!(options.contains(&format!("timezone={system_timezone:?}")));
    }

    #[test]
    fn source_value_cache_key_changes_with_model_aliases() {
        let fixture = ccusage_test_support::Fixture::new();
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let without_aliases = {
            let _env = ccusage_test_support::EnvVarsGuard::set_many([
                (
                    "XDG_CACHE_HOME",
                    Some(fixture.root().as_os_str().to_os_string()),
                ),
                ("CCUSAGE_MODEL_ALIASES", None),
            ]);
            super::source_value_cache_path("codex", AgentReportKind::Daily, &shared, "test-source")
                .unwrap()
        };
        let with_aliases = {
            let _env = ccusage_test_support::EnvVarsGuard::set_many([
                (
                    "XDG_CACHE_HOME",
                    Some(fixture.root().as_os_str().to_os_string()),
                ),
                (
                    "CCUSAGE_MODEL_ALIASES",
                    Some(OsString::from("private-alpha=gpt-5.2")),
                ),
            ]);
            super::source_value_cache_path("codex", AgentReportKind::Daily, &shared, "test-source")
                .unwrap()
        };

        assert_ne!(without_aliases, with_aliases);
    }

    #[test]
    fn source_value_cache_reuses_rows_until_signature_changes() {
        let fixture = ccusage_test_support::Fixture::new();
        let _cache_home = ccusage_test_support::EnvVarGuard::set("XDG_CACHE_HOME", fixture.root());
        let shared = SharedArgs {
            timezone: Some("UTC".to_string()),
            ..SharedArgs::default()
        };
        let mut load_count = 0;

        let first: Vec<TestRow> = super::load_source_value_with_cache(
            "claude",
            AgentReportKind::Daily,
            &shared,
            "test-source",
            "signature-a",
            || {
                load_count += 1;
                Ok(vec![TestRow {
                    content: "fresh-a".to_string(),
                }])
            },
        )
        .unwrap();
        let second: Vec<TestRow> = super::load_source_value_with_cache(
            "claude",
            AgentReportKind::Daily,
            &shared,
            "test-source",
            "signature-a",
            || {
                load_count += 1;
                Ok(vec![TestRow {
                    content: "miss".to_string(),
                }])
            },
        )
        .unwrap();
        let third: Vec<TestRow> = super::load_source_value_with_cache(
            "claude",
            AgentReportKind::Daily,
            &shared,
            "test-source",
            "signature-b",
            || {
                load_count += 1;
                Ok(vec![TestRow {
                    content: "fresh-b".to_string(),
                }])
            },
        )
        .unwrap();

        assert_eq!(first[0].content, "fresh-a");
        assert_eq!(second[0].content, "fresh-a");
        assert_eq!(third[0].content, "fresh-b");
        assert_eq!(load_count, 2);
    }

    #[test]
    fn file_rows_cache_reloads_only_changed_files() {
        let cache_fixture = ccusage_test_support::Fixture::new();
        let _cache_home =
            ccusage_test_support::EnvVarGuard::set("XDG_CACHE_HOME", cache_fixture.root());
        let fixture = ccusage_test_support::Fixture::new();
        let a = fixture.write_file("a.jsonl", "first");
        let b = fixture.write_file("b.jsonl", "second");
        let files = vec![a.clone(), b.clone()];
        let mut load_counts = BTreeMap::<String, usize>::new();

        let mut load = |misses: &[std::path::PathBuf]| {
            misses
                .iter()
                .map(|file| {
                    let key = file.to_string_lossy().into_owned();
                    *load_counts.entry(key.clone()).or_default() += 1;
                    vec![TestRow {
                        content: std::fs::read_to_string(file).unwrap(),
                    }]
                })
                .collect::<Vec<_>>()
        };

        let first = super::load_file_rows_with_cache("codex", "test-files", &files, &mut load);
        let second = super::load_file_rows_with_cache("codex", "test-files", &files, &mut load);
        std::fs::write(&b, "second-updated").unwrap();
        let third = super::load_file_rows_with_cache("codex", "test-files", &files, &mut load);

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        assert!(third.iter().any(|row| row.content == "second-updated"));
        assert_eq!(load_counts.get(a.to_string_lossy().as_ref()), Some(&1));
        assert_eq!(load_counts.get(b.to_string_lossy().as_ref()), Some(&2));
    }
}
