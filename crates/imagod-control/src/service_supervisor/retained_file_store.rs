//! File-backed retained logs for stopped services.
//!
//! Runtime behavior:
//! - one retained snapshot file per service
//! - atomic replace on update (`tmp -> rename`)
//! - startup rebuilds index from existing files and removes stale temp files

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use imago_protocol::{ErrorCode, from_cbor, to_cbor};
use imagod_common::ImagodError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{STAGE_LOGS, ServiceLogEvent, ServiceLogStream};

const STORE_FILE_EXTENSION: &str = "cbor";
const STORED_RETAINED_LOG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StoredRetainedLog {
    version: u32,
    #[serde(default)]
    service_name: String,
    events: Vec<StoredEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StoredEvent {
    stream: StoredServiceLogStream,
    timestamp_unix_ms: u64,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum StoredServiceLogStream {
    Stdout,
    Stderr,
}

impl From<ServiceLogStream> for StoredServiceLogStream {
    fn from(value: ServiceLogStream) -> Self {
        match value {
            ServiceLogStream::Stdout => Self::Stdout,
            ServiceLogStream::Stderr => Self::Stderr,
        }
    }
}

impl From<StoredServiceLogStream> for ServiceLogStream {
    fn from(value: StoredServiceLogStream) -> Self {
        match value {
            StoredServiceLogStream::Stdout => Self::Stdout,
            StoredServiceLogStream::Stderr => Self::Stderr,
        }
    }
}

#[derive(Debug, Clone)]
struct RetainedFileEntry {
    service_name: String,
    file_path: PathBuf,
    file_size_bytes: usize,
    updated_at_unix_ms: u64,
}

#[derive(Debug)]
pub(super) struct RetainedFileLogStore {
    retained_dir: PathBuf,
    capacity_bytes: usize,
    total_bytes: usize,
    entries: BTreeMap<String, RetainedFileEntry>,
}

impl RetainedFileLogStore {
    pub(super) fn new(storage_root: &Path, capacity_bytes: usize) -> Result<Self, ImagodError> {
        let retained_dir = storage_root.join("runtime").join("retained-logs");
        initialize_retained_dir(&retained_dir)?;
        let mut store = Self {
            retained_dir,
            capacity_bytes: capacity_bytes.max(1),
            total_bytes: 0,
            entries: BTreeMap::new(),
        };
        store.rebuild_index_from_disk()?;
        store.evict_if_needed()?;
        Ok(store)
    }

    #[cfg(test)]
    pub(super) fn capacity_bytes(&self) -> usize {
        self.capacity_bytes
    }

    pub(super) fn service_names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    pub(super) fn upsert(
        &mut self,
        service_name: &str,
        snapshot_events: &[ServiceLogEvent],
    ) -> Result<(), ImagodError> {
        let stored_log = StoredRetainedLog {
            version: STORED_RETAINED_LOG_VERSION,
            service_name: service_name.to_string(),
            events: snapshot_events
                .iter()
                .map(|event| StoredEvent {
                    stream: event.stream.into(),
                    timestamp_unix_ms: event.timestamp_unix_ms,
                    bytes: event.bytes.clone(),
                })
                .collect(),
        };
        let payload = to_cbor(&stored_log).map_err(|err| {
            retained_store_error(format!(
                "failed to encode retained logs for service '{service_name}': {err}"
            ))
        })?;
        fs::create_dir_all(&self.retained_dir).map_err(|err| {
            retained_store_error(format!(
                "failed to create retained logs dir {}: {err}",
                self.retained_dir.display()
            ))
        })?;

        let file_path = self.file_path_for(service_name);
        let tmp_path = self.tmp_path_for(service_name);
        fs::write(&tmp_path, &payload).map_err(|err| {
            retained_store_error(format!(
                "failed to write retained logs temp file {}: {err}",
                tmp_path.display()
            ))
        })?;
        if let Err(err) = fs::rename(&tmp_path, &file_path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(retained_store_error(format!(
                "failed to replace retained logs file {}: {err}",
                file_path.display()
            )));
        }

        if let Some(previous) = self.entries.remove(service_name) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.file_size_bytes);
        }
        let file_size_bytes = payload.len();
        self.total_bytes = self.total_bytes.saturating_add(file_size_bytes);
        self.entries.insert(
            service_name.to_string(),
            RetainedFileEntry {
                service_name: service_name.to_string(),
                file_path,
                file_size_bytes,
                updated_at_unix_ms: now_unix_ms(),
            },
        );
        self.evict_if_needed()
    }

    pub(super) fn snapshot_events(
        &self,
        service_name: &str,
    ) -> Result<Option<Vec<ServiceLogEvent>>, ImagodError> {
        let Some(entry) = self.entries.get(service_name) else {
            return Ok(None);
        };
        let payload = match fs::read(&entry.file_path) {
            Ok(payload) => payload,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(retained_store_error(format!(
                    "failed to read retained logs file {}: {err}",
                    entry.file_path.display()
                )));
            }
        };
        let stored: StoredRetainedLog = from_cbor(&payload).map_err(|err| {
            retained_store_error(format!(
                "failed to decode retained logs for service '{service_name}': {err}"
            ))
        })?;
        if stored.version != STORED_RETAINED_LOG_VERSION {
            return Err(retained_store_error(format!(
                "unsupported retained logs version {} for service '{service_name}'",
                stored.version
            )));
        }

        let events: Vec<ServiceLogEvent> = stored
            .events
            .into_iter()
            .map(|event| ServiceLogEvent {
                stream: event.stream.into(),
                bytes: event.bytes,
                timestamp_unix_ms: event.timestamp_unix_ms,
            })
            .collect();
        Ok(Some(events))
    }

    #[cfg(test)]
    pub(super) fn retained_dir(&self) -> &Path {
        &self.retained_dir
    }

    #[cfg(test)]
    pub(super) fn file_path_for_test(&self, service_name: &str) -> PathBuf {
        self.file_path_for(service_name)
    }

    #[cfg(test)]
    pub(super) fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    fn rebuild_index_from_disk(&mut self) -> Result<(), ImagodError> {
        self.entries.clear();
        self.total_bytes = 0;

        let read_dir = fs::read_dir(&self.retained_dir).map_err(|err| {
            retained_store_error(format!(
                "failed to read retained logs dir {}: {err}",
                self.retained_dir.display()
            ))
        })?;

        for entry_result in read_dir {
            let entry = entry_result.map_err(|err| {
                retained_store_error(format!(
                    "failed to read entry in retained logs dir {}: {err}",
                    self.retained_dir.display()
                ))
            })?;
            let path = entry.path();
            if !is_retained_store_file(&path) {
                continue;
            }

            let payload = fs::read(&path).map_err(|err| {
                retained_store_error(format!(
                    "failed to read retained logs file {}: {err}",
                    path.display()
                ))
            })?;
            let stored: StoredRetainedLog = match from_cbor(&payload) {
                Ok(stored) => stored,
                Err(_) => {
                    let _ = fs::remove_file(&path);
                    continue;
                }
            };
            if stored.version != STORED_RETAINED_LOG_VERSION {
                let _ = fs::remove_file(&path);
                continue;
            }
            let Some(service_name) = stored_service_name(&stored, &path) else {
                let _ = fs::remove_file(&path);
                continue;
            };

            let metadata = fs::metadata(&path).map_err(|err| {
                retained_store_error(format!(
                    "failed to stat retained logs file {}: {err}",
                    path.display()
                ))
            })?;
            let file_size_bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
            let updated_at_unix_ms = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_millis()).ok())
                .unwrap_or_else(now_unix_ms);

            if let Some(previous) = self.entries.insert(
                service_name.clone(),
                RetainedFileEntry {
                    service_name,
                    file_path: path,
                    file_size_bytes,
                    updated_at_unix_ms,
                },
            ) {
                self.total_bytes = self.total_bytes.saturating_sub(previous.file_size_bytes);
            }
            self.total_bytes = self.total_bytes.saturating_add(file_size_bytes);
        }

        Ok(())
    }

    fn evict_if_needed(&mut self) -> Result<(), ImagodError> {
        while self.total_bytes > self.capacity_bytes {
            let Some(oldest_service_name) = self
                .entries
                .iter()
                .min_by(|(_, left), (_, right)| {
                    left.updated_at_unix_ms
                        .cmp(&right.updated_at_unix_ms)
                        .then_with(|| left.service_name.cmp(&right.service_name))
                })
                .map(|(name, _)| name.clone())
            else {
                break;
            };

            let Some(removed) = self.entries.remove(&oldest_service_name) else {
                continue;
            };
            self.total_bytes = self.total_bytes.saturating_sub(removed.file_size_bytes);
            if let Err(err) = fs::remove_file(&removed.file_path)
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(retained_store_error(format!(
                    "failed to remove retained logs file {}: {err}",
                    removed.file_path.display()
                )));
            }
        }
        Ok(())
    }

    fn file_path_for(&self, service_name: &str) -> PathBuf {
        let stem = stable_service_file_stem(service_name);
        self.retained_dir
            .join(format!("{stem}.{STORE_FILE_EXTENSION}"))
    }

    fn tmp_path_for(&self, service_name: &str) -> PathBuf {
        let stem = stable_service_file_stem(service_name);
        self.retained_dir.join(format!(
            ".{stem}.tmp-{}.{}",
            uuid::Uuid::new_v4().simple(),
            STORE_FILE_EXTENSION
        ))
    }
}

fn initialize_retained_dir(dir: &Path) -> Result<(), ImagodError> {
    fs::create_dir_all(dir).map_err(|err| {
        retained_store_error(format!(
            "failed to create retained logs dir {}: {err}",
            dir.display()
        ))
    })?;
    let read_dir = fs::read_dir(dir).map_err(|err| {
        retained_store_error(format!(
            "failed to read retained logs dir {}: {err}",
            dir.display()
        ))
    })?;
    for entry_result in read_dir {
        let entry = entry_result.map_err(|err| {
            retained_store_error(format!(
                "failed to read entry in retained logs dir {}: {err}",
                dir.display()
            ))
        })?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();
        if !file_name_str.contains(".tmp-") {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| {
                retained_store_error(format!(
                    "failed to remove temporary retained logs dir {}: {err}",
                    path.display()
                ))
            })?;
        } else {
            fs::remove_file(&path).map_err(|err| {
                retained_store_error(format!(
                    "failed to remove temporary retained log file {}: {err}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn is_retained_store_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext == STORE_FILE_EXTENSION)
}

fn stored_service_name(stored: &StoredRetainedLog, path: &Path) -> Option<String> {
    if !stored.service_name.trim().is_empty() {
        return Some(stored.service_name.clone());
    }
    let stem = path.file_stem()?.to_string_lossy();
    let (legacy_name, _) = stem.rsplit_once('.')?;
    if legacy_name.trim().is_empty() {
        return None;
    }
    Some(legacy_name.to_string())
}

fn stable_service_file_stem(service_name: &str) -> String {
    let mut service_component: String = service_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if service_component.is_empty() {
        service_component.push_str("service");
    }
    if service_component.len() > 48 {
        service_component.truncate(48);
    }

    let mut hasher = Sha256::new();
    hasher.update(service_name.as_bytes());
    let digest = hasher.finalize();
    let hash = hex::encode(&digest[..8]);
    format!("{service_component}.{hash}")
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn retained_store_error(message: String) -> ImagodError {
    ImagodError::new(ErrorCode::Internal, STAGE_LOGS, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_test_root(prefix: &str) -> PathBuf {
        let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
        PathBuf::from(format!("/tmp/iss-retained-file-{prefix}-{id}"))
    }

    fn sample_events(prefix: &str, ts: u64) -> Vec<ServiceLogEvent> {
        vec![
            ServiceLogEvent {
                stream: ServiceLogStream::Stdout,
                bytes: format!("{prefix}-out\n").into_bytes(),
                timestamp_unix_ms: ts,
            },
            ServiceLogEvent {
                stream: ServiceLogStream::Stderr,
                bytes: format!("{prefix}-err\n").into_bytes(),
                timestamp_unix_ms: ts + 1,
            },
        ]
    }

    #[test]
    fn write_read_roundtrip_preserves_events_and_joined_bytes() {
        let root = new_test_root("roundtrip");
        let mut store = RetainedFileLogStore::new(&root, 1024).expect("store should initialize");
        let events = sample_events("a", 10);
        store
            .upsert("svc-a", &events)
            .expect("upsert should persist snapshot");

        let loaded_events = store
            .snapshot_events("svc-a")
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        assert_eq!(loaded_events, events);
        assert_eq!(
            loaded_events
                .iter()
                .flat_map(|event| event.bytes.clone())
                .collect::<Vec<_>>(),
            b"a-out\na-err\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn upsert_replaces_service_file_atomically() {
        let root = new_test_root("atomic-replace");
        let mut store = RetainedFileLogStore::new(&root, 2048).expect("store should initialize");
        store
            .upsert("svc-a", &sample_events("old", 1))
            .expect("first upsert should succeed");
        let path = store.file_path_for_test("svc-a");
        assert!(path.exists(), "retained file should be created");
        store
            .upsert("svc-a", &sample_events("new", 2))
            .expect("second upsert should replace existing file");

        let snapshot_events = store
            .snapshot_events("svc-a")
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        let joined = snapshot_events
            .iter()
            .flat_map(|event| event.bytes.clone())
            .collect::<Vec<_>>();
        assert!(String::from_utf8_lossy(&joined).contains("new-out"));

        let lingering_tmp = fs::read_dir(store.retained_dir())
            .expect("retained dir should be readable")
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp-"));
        assert!(!lingering_tmp, "temporary files should be cleaned up");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_file_returns_internal_error() {
        let root = new_test_root("corrupt");
        let mut store = RetainedFileLogStore::new(&root, 2048).expect("store should initialize");
        store
            .upsert("svc-a", &sample_events("ok", 1))
            .expect("upsert should succeed");
        let path = store.file_path_for_test("svc-a");
        fs::write(&path, b"not-cbor").expect("corrupt payload should be written");

        let err = store
            .snapshot_events("svc-a")
            .expect_err("corrupt payload should map to internal error");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.stage, STAGE_LOGS);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn evicts_oldest_entry_by_update_time_when_capacity_exceeded() {
        let root = new_test_root("evict");
        let sample_payload_size = to_cbor(&StoredRetainedLog {
            version: STORED_RETAINED_LOG_VERSION,
            service_name: "size".to_string(),
            events: sample_events("size", 1)
                .into_iter()
                .map(|event| StoredEvent {
                    stream: event.stream.into(),
                    timestamp_unix_ms: event.timestamp_unix_ms,
                    bytes: event.bytes,
                })
                .collect(),
        })
        .expect("sample retained payload should encode")
        .len();
        let mut store = RetainedFileLogStore::new(&root, sample_payload_size * 2 + 8)
            .expect("store should initialize");
        store
            .upsert("svc-a", &sample_events("a", 1))
            .expect("first upsert should succeed");
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .upsert("svc-b", &sample_events("b", 2))
            .expect("second upsert should succeed");
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .upsert("svc-c", &sample_events("c", 3))
            .expect("third upsert should succeed");

        let names = store.service_names();
        assert!(
            names.len() < 3,
            "capacity should evict at least one older retained entry"
        );
        assert!(
            store
                .snapshot_events("svc-c")
                .expect("latest snapshot read should succeed")
                .is_some(),
            "newest snapshot should remain after eviction"
        );
        assert!(
            store.total_bytes() <= store.capacity_bytes(),
            "store should stay within configured capacity"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restart_rebuilds_index_from_existing_retained_files() {
        let root = new_test_root("restart-rebuild");
        {
            let mut store = RetainedFileLogStore::new(&root, 4096).expect("store should init");
            store
                .upsert("svc-persist", &sample_events("persist", 10))
                .expect("upsert should persist snapshot");
        }

        let store = RetainedFileLogStore::new(&root, 4096).expect("store should re-init");
        let names = store.service_names();
        assert_eq!(names, vec!["svc-persist".to_string()]);
        let events = store
            .snapshot_events("svc-persist")
            .expect("snapshot read should succeed")
            .expect("snapshot should exist");
        let bytes = events
            .iter()
            .flat_map(|event| event.bytes.clone())
            .collect::<Vec<_>>();
        assert!(
            String::from_utf8_lossy(&bytes).contains("persist-out"),
            "persisted bytes should be available after restart"
        );
        assert_eq!(events, sample_events("persist", 10));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn startup_cleans_tmp_files_but_keeps_retained_files() {
        let root = new_test_root("cleanup-tmp");
        let retained_dir = root.join("runtime").join("retained-logs");
        fs::create_dir_all(&retained_dir).expect("retained dir should exist");

        let mut store = RetainedFileLogStore::new(&root, 4096).expect("store should init");
        store
            .upsert("svc-a", &sample_events("stable", 1))
            .expect("upsert should succeed");

        let dangling_tmp_file = retained_dir.join(".dangling.tmp-test.cbor");
        fs::write(&dangling_tmp_file, b"tmp").expect("tmp file should be creatable");
        let dangling_tmp_dir = retained_dir.join(".dangling.tmp-dir");
        fs::create_dir_all(&dangling_tmp_dir).expect("tmp dir should be creatable");

        let store = RetainedFileLogStore::new(&root, 4096).expect("store should re-init");
        assert!(
            !dangling_tmp_file.exists() && !dangling_tmp_dir.exists(),
            "startup should remove dangling tmp artifacts"
        );
        assert!(
            store
                .snapshot_events("svc-a")
                .expect("snapshot read should succeed")
                .is_some(),
            "retained snapshot should survive startup cleanup"
        );

        let _ = fs::remove_dir_all(root);
    }
}
