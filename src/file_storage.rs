//! Локальный файловый backend immutable recovery generations.
//!
//! Первая реализация ориентирована на Unix: атомарная замена CURRENT выполняется
//! rename в пределах одной файловой системы. Доменное ядро не зависит от этого модуля.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::recovery::{
    validate_generation_transition, RecoveryError, RecoveryManifest, RecoveryPackage,
    RecoveryStorage, RecoveryStorageError,
};

const CURRENT_MAGIC: &[u8; 4] = b"SGFC";
const CURRENT_VERSION: u16 = 1;
const DIGEST_LEN: usize = 32;
const CURRENT_PAYLOAD_LEN: usize = 4 + 2 + 8 + DIGEST_LEN;
const CURRENT_LEN: usize = CURRENT_PAYLOAD_LEN + DIGEST_LEN;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum FileRecoveryStorageError {
    Io(io::Error),
    InvalidCurrent,
    InvalidLockFile,
    InvalidGenerationDirectory,
    UnreferencedGenerationExists,
    ManifestGenerationMismatch { pointer: u64, manifest: u64 },
    ManifestDigestMismatch,
    Recovery(RecoveryError),
}

impl std::fmt::Display for FileRecoveryStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "file recovery storage I/O error: {error}"),
            Self::InvalidCurrent => write!(f, "invalid SGFC current pointer"),
            Self::InvalidLockFile => write!(f, "writer lock path is not a regular file"),
            Self::InvalidGenerationDirectory => {
                write!(f, "invalid recovery generation directory")
            }
            Self::UnreferencedGenerationExists => {
                write!(f, "generation data exists but CURRENT pointer is missing")
            }
            Self::ManifestGenerationMismatch { pointer, manifest } => write!(
                f,
                "CURRENT generation {pointer} does not match manifest generation {manifest}"
            ),
            Self::ManifestDigestMismatch => write!(f, "CURRENT manifest digest mismatch"),
            Self::Recovery(error) => write!(f, "recovery package validation failed: {error:?}"),
        }
    }
}

impl std::error::Error for FileRecoveryStorageError {}

impl From<io::Error> for FileRecoveryStorageError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<RecoveryError> for FileRecoveryStorageError {
    fn from(value: RecoveryError) -> Self {
        Self::Recovery(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CurrentPointer {
    generation: u64,
    manifest_digest: [u8; DIGEST_LEN],
}

impl CurrentPointer {
    fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CURRENT_LEN);
        bytes.extend_from_slice(CURRENT_MAGIC);
        bytes.extend_from_slice(&CURRENT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        bytes.extend_from_slice(&self.manifest_digest);
        let digest = Sha256::digest(&bytes);
        bytes.extend_from_slice(&digest);
        bytes
    }

    fn decode(bytes: &[u8]) -> Result<Self, FileRecoveryStorageError> {
        if bytes.len() != CURRENT_LEN {
            return Err(FileRecoveryStorageError::InvalidCurrent);
        }
        let (payload, supplied_digest) = bytes.split_at(CURRENT_PAYLOAD_LEN);
        if Sha256::digest(payload).as_slice() != supplied_digest {
            return Err(FileRecoveryStorageError::InvalidCurrent);
        }
        if &payload[..4] != CURRENT_MAGIC {
            return Err(FileRecoveryStorageError::InvalidCurrent);
        }
        let version = u16::from_le_bytes(
            payload[4..6]
                .try_into()
                .map_err(|_| FileRecoveryStorageError::InvalidCurrent)?,
        );
        if version != CURRENT_VERSION {
            return Err(FileRecoveryStorageError::InvalidCurrent);
        }
        let generation = u64::from_le_bytes(
            payload[6..14]
                .try_into()
                .map_err(|_| FileRecoveryStorageError::InvalidCurrent)?,
        );
        let manifest_digest = payload[14..CURRENT_PAYLOAD_LEN]
            .try_into()
            .map_err(|_| FileRecoveryStorageError::InvalidCurrent)?;
        Ok(Self {
            generation,
            manifest_digest,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FileRecoveryStorage {
    root: PathBuf,
}

impl FileRecoveryStorage {
    /// Открывает локальное хранилище и создаёт необходимые каталоги.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, FileRecoveryStorageError> {
        Self::open_with_hook(root, |_| Ok(()))
    }

    fn open_with_hook<F>(
        root: impl AsRef<Path>,
        mut hook: F,
    ) -> Result<Self, FileRecoveryStorageError>
    where
        F: FnMut(CommitStage) -> io::Result<()>,
    {
        let root = root.as_ref().to_path_buf();
        create_dir_all_synced(&root)?;
        ensure_real_directory(&root)?;
        create_dir_all_synced(&root.join("generations"))?;
        ensure_real_directory(&root.join("generations"))?;

        // Persist both directory entries before any commit can publish CURRENT.
        // Otherwise a power loss could preserve CURRENT while losing "generations/".
        sync_directory(&root)?;
        hook(CommitStage::LayoutDirectoriesSynced)?;

        let lock_path = root.join(".writer.lock");
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let lock_metadata = fs::symlink_metadata(&lock_path)?;
        if !lock_metadata.file_type().is_file() || lock_metadata.file_type().is_symlink() {
            return Err(FileRecoveryStorageError::InvalidLockFile);
        }
        Ok(Self { root })
    }

    fn generation_dir(&self, generation: u64) -> PathBuf {
        self.root
            .join("generations")
            .join(format!("g-{generation:020}"))
    }

    fn read_current(&self) -> Result<Option<RecoveryPackage>, FileRecoveryStorageError> {
        let current_path = self.root.join("CURRENT");
        let current_metadata = match fs::symlink_metadata(&current_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !current_metadata.file_type().is_file() || current_metadata.file_type().is_symlink() {
            return Err(FileRecoveryStorageError::InvalidCurrent);
        }
        // CURRENT has a fixed-size encoding. Bound the read before allocation.
        let current_file = File::open(&current_path)?;
        let mut current_bytes = Vec::with_capacity(CURRENT_LEN + 1);
        current_file
            .take((CURRENT_LEN + 1) as u64)
            .read_to_end(&mut current_bytes)?;
        let pointer = CurrentPointer::decode(&current_bytes)?;
        let generation_dir = self.generation_dir(pointer.generation);

        let metadata = fs::symlink_metadata(&generation_dir)?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(FileRecoveryStorageError::InvalidGenerationDirectory);
        }

        let snapshot = fs::read(generation_dir.join("snapshot.bin"))?;
        let journal = fs::read(generation_dir.join("journal.bin"))?;
        let manifest_bytes = fs::read(generation_dir.join("manifest.bin"))?;

        if Sha256::digest(&manifest_bytes).as_slice() != pointer.manifest_digest {
            return Err(FileRecoveryStorageError::ManifestDigestMismatch);
        }
        let manifest = RecoveryManifest::decode(&manifest_bytes)?;
        if manifest.generation != pointer.generation {
            return Err(FileRecoveryStorageError::ManifestGenerationMismatch {
                pointer: pointer.generation,
                manifest: manifest.generation,
            });
        }

        let package = RecoveryPackage {
            snapshot,
            journal,
            manifest,
        };
        package
            .validate()
            .map_err(FileRecoveryStorageError::Recovery)?;
        Ok(Some(package))
    }

    fn publish(&self, package: RecoveryPackage) -> Result<(), FileRecoveryStorageError> {
        self.publish_with_hook(package, |_| Ok(()))
    }

    fn publish_with_hook<F>(
        &self,
        package: RecoveryPackage,
        mut hook: F,
    ) -> Result<(), FileRecoveryStorageError>
    where
        F: FnMut(CommitStage) -> io::Result<()>,
    {
        let generation = package.manifest.generation;
        let generations_dir = self.root.join("generations");
        let token = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        // Keep the unique staging directory as a direct child of the validated root.
        let stage_dir = self.root.join(format!(
            ".staging-g-{generation:020}-p{}-{token}",
            std::process::id()
        ));
        let final_dir = self.generation_dir(generation);
        let current_temp = self
            .root
            .join(format!(".CURRENT-p{}-{token}", std::process::id()));

        let result = (|| {
            fs::create_dir(&stage_dir)?;
            hook(CommitStage::StagingCreated)?;
            write_new_sync(&stage_dir.join("snapshot.bin"), &package.snapshot)?;
            hook(CommitStage::SnapshotSynced)?;
            write_new_sync(&stage_dir.join("journal.bin"), &package.journal)?;
            hook(CommitStage::JournalSynced)?;
            let manifest_bytes = package.manifest.encode();
            write_new_sync(&stage_dir.join("manifest.bin"), &manifest_bytes)?;
            hook(CommitStage::ManifestSynced)?;
            sync_directory(&stage_dir)?;
            hook(CommitStage::StagingDirectorySynced)?;

            // Never replace an existing generation directory. It may contain
            // a complete recovery package newer than a restored/stale CURRENT.
            match fs::symlink_metadata(&final_dir) {
                Ok(_) => return Err(FileRecoveryStorageError::UnreferencedGenerationExists),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }

            fs::rename(&stage_dir, &final_dir)?;
            hook(CommitStage::GenerationPublished)?;
            sync_directory(&generations_dir)?;
            hook(CommitStage::GenerationsDirectorySynced)?;

            let pointer = CurrentPointer {
                generation,
                manifest_digest: Sha256::digest(&manifest_bytes).into(),
            };

            write_new_sync(&current_temp, &pointer.encode())?;
            hook(CommitStage::CurrentSynced)?;
            fs::rename(&current_temp, self.root.join("CURRENT"))?;
            hook(CommitStage::CurrentPublished)?;
            sync_directory(&self.root)?;
            hook(CommitStage::RootDirectorySynced)?;
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_dir_all(&stage_dir);
            let _ = fs::remove_file(&current_temp);
        }
        result
    }
}

impl RecoveryStorage for FileRecoveryStorage {
    type Error = FileRecoveryStorageError;

    fn load_current(&self) -> Result<Option<RecoveryPackage>, Self::Error> {
        self.read_current()
    }

    fn commit_generation(
        &mut self,
        expected_current_generation: Option<u64>,
        package: RecoveryPackage,
    ) -> Result<(), RecoveryStorageError<Self::Error>> {
        let lock_path = self.root.join(".writer.lock");
        let mut lock = filelock::new(&lock_path)
            .map_err(FileRecoveryStorageError::Io)
            .map_err(RecoveryStorageError::Backend)?;
        let _guard = lock
            .lock()
            .map_err(FileRecoveryStorageError::Io)
            .map_err(RecoveryStorageError::Backend)?;

        // Staging cleanup is intentionally disabled: path-based traversal is
        // vulnerable to replacement races. Reclamation needs pinned no-follow handles.
        let current = self.read_current().map_err(RecoveryStorageError::Backend)?;
        let actual_generation = current.as_ref().map(|value| value.manifest.generation);
        if actual_generation.is_none()
            && directory_has_entries(&self.root.join("generations"))
                .map_err(RecoveryStorageError::Backend)?
        {
            return Err(RecoveryStorageError::Backend(
                FileRecoveryStorageError::UnreferencedGenerationExists,
            ));
        }
        validate_generation_transition(
            expected_current_generation,
            actual_generation,
            package.manifest.generation,
        )
        .map_err(RecoveryStorageError::Generation)?;

        package
            .validate()
            .map_err(RecoveryStorageError::InvalidPackage)?;

        self.publish(package).map_err(RecoveryStorageError::Backend)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitStage {
    LayoutDirectoriesSynced,
    StagingCreated,
    SnapshotSynced,
    JournalSynced,
    ManifestSynced,
    StagingDirectorySynced,
    GenerationPublished,
    GenerationsDirectorySynced,
    CurrentSynced,
    CurrentPublished,
    RootDirectorySynced,
}

fn create_dir_all_synced(path: &Path) -> Result<(), FileRecoveryStorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                return Ok(());
            }
            return Err(FileRecoveryStorageError::InvalidGenerationDirectory);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_dir_all_synced(parent)?;

    match fs::create_dir(path) {
        Ok(()) => sync_directory(parent)?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => ensure_real_directory(path)?,
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> Result<(), FileRecoveryStorageError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(FileRecoveryStorageError::InvalidGenerationDirectory)
    }
}

fn directory_has_entries(path: &Path) -> Result<bool, FileRecoveryStorageError> {
    ensure_real_directory(path)?;
    Ok(fs::read_dir(path)?.next().is_some())
}

fn write_new_sync(path: &Path, bytes: &[u8]) -> Result<(), io::Error> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn sync_directory(path: &Path) -> Result<(), io::Error> {
    File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        encode_record, EventValidator, JournalCheckpoint, MarketEvent, Quote,
        RecoveryGenerationError, RecoveryManifest,
    };
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("sgf-file-storage-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn quote(id: &str, sequence: u64) -> MarketEvent {
        MarketEvent::quote(
            id,
            "feed",
            "BTCUSD",
            sequence as i64,
            sequence as i64 + 1,
            Some(sequence),
            Quote {
                bid_ticks: 100,
                ask_ticks: 101,
                price_scale: 10,
            },
        )
    }

    fn package(generation: u64) -> RecoveryPackage {
        let snapshot = EventValidator::new().encode_snapshot().unwrap();
        let anchor = JournalCheckpoint::default();
        let journal = encode_record(&anchor, &quote("e1", 1)).unwrap();
        let expected_end = JournalCheckpoint {
            next_offset: 1,
            last_hash: journal[journal.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let manifest = RecoveryManifest {
            generation,
            snapshot_digest: Sha256::digest(&snapshot).into(),
            journal_bytes: journal.len() as u64,
            anchor,
            expected_end,
        };
        RecoveryPackage {
            snapshot,
            journal,
            manifest,
        }
    }

    #[test]
    fn layout_directories_are_synced_before_open_continues() {
        let directory = TestDirectory::new();
        let mut hook_called = false;

        let result = FileRecoveryStorage::open_with_hook(directory.path(), |stage| {
            assert_eq!(stage, CommitStage::LayoutDirectoriesSynced);
            assert!(directory.path().join("generations").is_dir());
            hook_called = true;
            Err(io::Error::new(
                io::ErrorKind::Other,
                "injected after layout sync",
            ))
        });

        assert!(result.is_err());
        assert!(hook_called);
        assert!(!directory.path().join(".writer.lock").exists());
    }

    #[test]
    fn commits_and_loads_complete_generation() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();

        storage.commit_generation(None, package(1)).unwrap();

        let loaded = storage.load_current().unwrap().unwrap();
        assert_eq!(loaded.manifest.generation, 1);
        loaded.validate().unwrap();
    }

    #[test]
    fn rejects_stale_writer_without_replacing_current_generation() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();

        storage.commit_generation(None, package(1)).unwrap();
        storage.commit_generation(Some(1), package(2)).unwrap();

        let error = storage.commit_generation(Some(1), package(2)).unwrap_err();
        assert!(matches!(
            error,
            RecoveryStorageError::Generation(RecoveryGenerationError::GenerationConflict {
                expected_current: Some(1),
                actual_current: Some(2)
            })
        ));

        assert_eq!(
            storage.load_current().unwrap().unwrap().manifest.generation,
            2
        );
    }

    #[test]
    fn concurrent_writers_cannot_publish_the_same_successor() {
        let directory = TestDirectory::new();
        let mut initial = FileRecoveryStorage::open(directory.path()).unwrap();
        initial.commit_generation(None, package(1)).unwrap();

        let root = directory.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut storage = FileRecoveryStorage::open(root).unwrap();
                    barrier.wait();
                    storage.commit_generation(Some(1), package(2))
                })
            })
            .collect();

        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(RecoveryStorageError::Generation(
                        RecoveryGenerationError::GenerationConflict { .. }
                    ))
                ))
                .count(),
            1
        );

        let storage = FileRecoveryStorage::open(directory.path()).unwrap();
        assert_eq!(
            storage.load_current().unwrap().unwrap().manifest.generation,
            2
        );
    }

    #[test]
    fn oversized_current_pointer_is_rejected_after_bounded_read() {
        let directory = TestDirectory::new();
        let storage = FileRecoveryStorage::open(directory.path()).unwrap();
        fs::write(
            directory.path().join("CURRENT"),
            vec![0_u8; CURRENT_LEN + 1024 * 1024],
        )
        .unwrap();

        assert!(matches!(
            storage.load_current(),
            Err(FileRecoveryStorageError::InvalidCurrent)
        ));
    }

    #[test]
    fn corrupt_current_pointer_fails_closed() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
        storage.commit_generation(None, package(1)).unwrap();

        fs::write(directory.path().join("CURRENT"), b"corrupt").unwrap();

        assert!(matches!(
            storage.load_current(),
            Err(FileRecoveryStorageError::InvalidCurrent)
        ));
    }
    #[test]
    fn interrupted_commit_never_exposes_a_partial_generation() {
        let stages = [
            CommitStage::StagingCreated,
            CommitStage::SnapshotSynced,
            CommitStage::JournalSynced,
            CommitStage::ManifestSynced,
            CommitStage::StagingDirectorySynced,
            CommitStage::GenerationPublished,
            CommitStage::GenerationsDirectorySynced,
            CommitStage::CurrentSynced,
            CommitStage::CurrentPublished,
            CommitStage::RootDirectorySynced,
        ];

        for fail_stage in stages {
            let directory = TestDirectory::new();
            let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
            storage.commit_generation(None, package(1)).unwrap();

            let result = storage.publish_with_hook(package(2), |stage| {
                if stage == fail_stage {
                    Err(io::Error::new(io::ErrorKind::Other, "injected crash"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err(), "stage {fail_stage:?} must inject failure");

            let current = storage.load_current().unwrap().unwrap();
            let expected_generation = if matches!(
                fail_stage,
                CommitStage::CurrentPublished | CommitStage::RootDirectorySynced
            ) {
                2
            } else {
                1
            };
            assert_eq!(current.manifest.generation, expected_generation);

            if matches!(
                fail_stage,
                CommitStage::GenerationPublished
                    | CommitStage::GenerationsDirectorySynced
                    | CommitStage::CurrentSynced
            ) {
                // Preserve a complete but unpublished successor; fail closed.
                assert!(storage.commit_generation(Some(1), package(2)).is_err());
                assert!(storage.generation_dir(2).join("manifest.bin").exists());
                assert_eq!(
                    storage.load_current().unwrap().unwrap().manifest.generation,
                    1
                );
            } else if expected_generation == 1 {
                storage.commit_generation(Some(1), package(2)).unwrap();
                assert_eq!(
                    storage.load_current().unwrap().unwrap().manifest.generation,
                    2
                );
            }
        }
    }
    #[test]
    fn missing_current_does_not_allow_overwriting_existing_generation() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
        storage.commit_generation(None, package(1)).unwrap();

        fs::remove_file(directory.path().join("CURRENT")).unwrap();

        assert!(matches!(
            storage.commit_generation(None, package(1)),
            Err(RecoveryStorageError::Backend(
                FileRecoveryStorageError::UnreferencedGenerationExists
            ))
        ));
        assert!(directory
            .path()
            .join("generations")
            .join("g-00000000000000000001")
            .join("manifest.bin")
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn staging_symlink_is_never_followed_or_modified() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let outside = TestDirectory::new();
        fs::write(outside.path().join("preserve.txt"), b"keep").unwrap();
        symlink(outside.path(), directory.path().join(".staging")).unwrap();

        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
        storage.commit_generation(None, package(1)).unwrap();

        assert_eq!(fs::read(outside.path().join("preserve.txt")).unwrap(), b"keep");
        assert_eq!(storage.load_current().unwrap().unwrap().manifest.generation, 1);
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_writer_lock_file() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let outside = TestDirectory::new();
        let external_lock_target = outside.path().join("external.lock");
        fs::write(&external_lock_target, b"external").unwrap();
        symlink(&external_lock_target, directory.path().join(".writer.lock")).unwrap();

        assert!(matches!(
            FileRecoveryStorage::open(directory.path()),
            Err(FileRecoveryStorageError::InvalidLockFile)
        ));
        assert_eq!(fs::read(&external_lock_target).unwrap(), b"external");
    }
}
