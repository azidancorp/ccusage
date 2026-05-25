use std::{
    collections::{hash_map::DefaultHasher, BTreeMap},
    env, fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    cli::{AgentReportKind, SharedArgs},
    Result,
};

const SOURCE_VALUE_CACHE_VERSION: u32 = 1;
const FILE_ROWS_CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct FileState {
    size: u64,
    modified_secs: u64,
    modified_nanos: u32,
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
    format!(
        "mode={:?}\noffline={}\nsince={:?}\nuntil={:?}\ntimezone={:?}",
        shared.mode, shared.offline, shared.since, shared.until, shared.timezone
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
    if let Some(parent) = cache_path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = fs::write(cache_path, raw);
}

fn cache_file_key(file: &Path) -> String {
    file.to_string_lossy().into_owned()
}

fn file_state(file: &Path) -> Option<FileState> {
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        env, fs,
        path::{Path, PathBuf},
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::cli::SharedArgs;

    static XDG_CACHE_HOME_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestRow {
        file: String,
        content: String,
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("ccusage-cache-{name}-{nanos}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn with_cache_home<T>(name: &str, run: impl FnOnce(&Path) -> T) -> T {
        let _guard = XDG_CACHE_HOME_LOCK.lock().unwrap();
        let cache_home = temp_dir(name);
        let previous = env::var_os("XDG_CACHE_HOME");
        env::set_var("XDG_CACHE_HOME", &cache_home);
        let result = run(&cache_home);
        if let Some(previous) = previous {
            env::set_var("XDG_CACHE_HOME", previous);
        } else {
            env::remove_var("XDG_CACHE_HOME");
        }
        let _ = fs::remove_dir_all(cache_home);
        result
    }

    #[test]
    fn source_value_cache_reuses_rows_until_signature_changes() {
        with_cache_home("source", |_| {
            let shared = SharedArgs {
                timezone: Some("UTC".to_string()),
                ..SharedArgs::default()
            };
            let mut signature = "signature-a".to_string();
            let mut load_count = 0;

            let first: Vec<TestRow> = load_source_value_with_cache(
                "kimi",
                AgentReportKind::Daily,
                &shared,
                "test-source",
                &signature,
                || {
                    load_count += 1;
                    Ok(vec![TestRow {
                        file: "a".to_string(),
                        content: signature.clone(),
                    }])
                },
            )
            .unwrap();
            let second: Vec<TestRow> = load_source_value_with_cache(
                "kimi",
                AgentReportKind::Daily,
                &shared,
                "test-source",
                &signature,
                || {
                    load_count += 1;
                    Ok(vec![TestRow {
                        file: "a".to_string(),
                        content: "miss".to_string(),
                    }])
                },
            )
            .unwrap();

            signature = "signature-b".to_string();
            let third: Vec<TestRow> = load_source_value_with_cache(
                "kimi",
                AgentReportKind::Daily,
                &shared,
                "test-source",
                &signature,
                || {
                    load_count += 1;
                    Ok(vec![TestRow {
                        file: "a".to_string(),
                        content: signature.clone(),
                    }])
                },
            )
            .unwrap();

            assert_eq!(first[0].content, "signature-a");
            assert_eq!(second[0].content, "signature-a");
            assert_eq!(third[0].content, "signature-b");
            assert_eq!(load_count, 2);
        });
    }

    #[test]
    fn file_rows_cache_reloads_only_changed_files() {
        with_cache_home("files", |_| {
            let dir = temp_dir("inputs");
            let a = dir.join("a.jsonl");
            let b = dir.join("b.jsonl");
            fs::write(&a, "first").unwrap();
            fs::write(&b, "second").unwrap();
            let files = vec![a.clone(), b.clone()];
            let mut load_counts = BTreeMap::<String, usize>::new();

            let mut load = |misses: &[PathBuf]| {
                misses
                    .iter()
                    .map(|file| {
                        let key = file.to_string_lossy().into_owned();
                        *load_counts.entry(key.clone()).or_default() += 1;
                        vec![TestRow {
                            file: key,
                            content: fs::read_to_string(file).unwrap(),
                        }]
                    })
                    .collect::<Vec<_>>()
            };

            let first = load_file_rows_with_cache("codex", "test-files", &files, &mut load);
            let second = load_file_rows_with_cache("codex", "test-files", &files, &mut load);
            fs::write(&b, "second-updated").unwrap();
            let third = load_file_rows_with_cache("codex", "test-files", &files, &mut load);

            assert_eq!(first.len(), 2);
            assert_eq!(second.len(), 2);
            assert_eq!(
                third
                    .iter()
                    .find(|row| row.file == b.to_string_lossy())
                    .map(|row| row.content.as_str()),
                Some("second-updated")
            );
            assert_eq!(load_counts.get(a.to_string_lossy().as_ref()), Some(&1));
            assert_eq!(load_counts.get(b.to_string_lossy().as_ref()), Some(&2));
            let _ = fs::remove_dir_all(dir);
        });
    }
}
