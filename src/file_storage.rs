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

/// Конфигурируемые бюджеты чтения артефактов восстановления.
///
/// Значения по умолчанию являются эксплуатационной политикой, а не пределами
/// формата SGF. Их можно увеличить через open_with_limits после оценки памяти.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtifactReadLimits {
    pub max_snapshot_bytes: u64,
    pub max_journal_bytes: u64,
    pub max_manifest_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ArtifactReadLimits {
    fn default() -> Self {
        Self {
            max_snapshot_bytes: 256 * 1024 * 1024,
            max_journal_bytes: 512 * 1024 * 1024,
            max_manifest_bytes: 16 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024,
        }
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum FileRecoveryStorageError {
    Io(io::Error),
    InvalidCurrent,
    InvalidLockFile,
    InvalidGenerationDirectory,
    UnreferencedGenerationExists,
    ManifestGenerationMismatch {
        pointer: u64,
        manifest: u64,
    },
    ManifestDigestMismatch,
    InvalidRecoveryArtifact(&'static str),
    ArtifactReadLimitExceeded {
        artifact: &'static str,
        size: u64,
        limit: u64,
    },
    ArtifactSizeOverflow,
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
            Self::InvalidRecoveryArtifact(name) => {
                write!(f, "recovery artifact is not a regular file: {name}")
            }
            Self::ArtifactReadLimitExceeded {
                artifact,
                size,
                limit,
            } => write!(
                f,
                "recovery artifact {artifact} size {size} exceeds configured read limit {limit}"
            ),
            Self::ArtifactSizeOverflow => write!(f, "recovery artifact size sum overflow"),
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
    read_limits: ArtifactReadLimits,
}

impl FileRecoveryStorage {
    /// Открывает локальное хранилище с эксплуатационными бюджетами по умолчанию.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, FileRecoveryStorageError> {
        Self::open_with_limits(root, ArtifactReadLimits::default())
    }

    /// Открывает хранилище с явными бюджетами чтения артефактов.
    ///
    /// Бюджеты ограничивают выделение памяти при восстановлении. Они не меняют
    /// формат пакета и могут быть увеличены вызывающей стороной.
    pub fn open_with_limits(
        root: impl AsRef<Path>,
        read_limits: ArtifactReadLimits,
    ) -> Result<Self, FileRecoveryStorageError> {
        Self::open_with_limits_and_hook(root, read_limits, |_| Ok(()))
    }

    #[cfg(test)]
    fn open_with_hook<F>(root: impl AsRef<Path>, hook: F) -> Result<Self, FileRecoveryStorageError>
    where
        F: FnMut(CommitStage) -> io::Result<()>,
    {
        Self::open_with_limits_and_hook(root, ArtifactReadLimits::default(), hook)
    }

    fn open_with_limits_and_hook<F>(
        root: impl AsRef<Path>,
        read_limits: ArtifactReadLimits,
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
        Ok(Self { root, read_limits })
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

        let snapshot_path = generation_dir.join("snapshot.bin");
        let journal_path = generation_dir.join("journal.bin");
        let manifest_path = generation_dir.join("manifest.bin");

        // Inspect all opened artifacts and reject per-file/aggregate sizes before
        // allocating any artifact buffer.
        let (snapshot_file, snapshot_size) = open_recovery_artifact(&snapshot_path, "snapshot")?;
        let (journal_file, journal_size) = open_recovery_artifact(&journal_path, "journal")?;
        let (manifest_file, manifest_size) = open_recovery_artifact(&manifest_path, "manifest")?;

        check_artifact_limit(
            "snapshot",
            snapshot_size,
            self.read_limits.max_snapshot_bytes,
        )?;
        check_artifact_limit("journal", journal_size, self.read_limits.max_journal_bytes)?;
        check_artifact_limit(
            "manifest",
            manifest_size,
            self.read_limits.max_manifest_bytes,
        )?;

        let total_size = snapshot_size
            .checked_add(journal_size)
            .and_then(|size| size.checked_add(manifest_size))
            .ok_or(FileRecoveryStorageError::ArtifactSizeOverflow)?;
        check_artifact_limit("aggregate", total_size, self.read_limits.max_total_bytes)?;

        // Enforce the aggregate budget again during actual reads. The files
        // may grow after metadata inspection; per-file limits alone would allow
        // the combined buffers to exceed max_total_bytes in that race.
        let snapshot_limit = self
            .read_limits
            .max_snapshot_bytes
            .min(self.read_limits.max_total_bytes);
        let snapshot_error_artifact = if snapshot_limit < self.read_limits.max_snapshot_bytes {
            "aggregate"
        } else {
            "snapshot"
        };
        let snapshot = read_bounded_artifact(
            snapshot_file,
            snapshot_error_artifact,
            snapshot_size,
            snapshot_limit,
        )?;

        let after_snapshot = self
            .read_limits
            .max_total_bytes
            .checked_sub(snapshot.len() as u64)
            .ok_or(FileRecoveryStorageError::ArtifactSizeOverflow)?;
        let journal_limit = self.read_limits.max_journal_bytes.min(after_snapshot);
        let journal_error_artifact = if journal_limit < self.read_limits.max_journal_bytes {
            "aggregate"
        } else {
            "journal"
        };
        let journal = read_bounded_artifact(
            journal_file,
            journal_error_artifact,
            journal_size,
            journal_limit,
        )?;

        let after_journal = after_snapshot
            .checked_sub(journal.len() as u64)
            .ok_or(FileRecoveryStorageError::ArtifactSizeOverflow)?;
        let manifest_limit = self.read_limits.max_manifest_bytes.min(after_journal);
        let manifest_error_artifact = if manifest_limit < self.read_limits.max_manifest_bytes {
            "aggregate"
        } else {
            "manifest"
        };
        let manifest_bytes = read_bounded_artifact(
            manifest_file,
            manifest_error_artifact,
            manifest_size,
            manifest_limit,
        )?;

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
        hook: F,
    ) -> Result<(), FileRecoveryStorageError>
    where
        F: FnMut(CommitStage) -> io::Result<()>,
    {
        let token = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.publish_with_token(package, token, hook)
    }

    fn publish_with_token<F>(
        &self,
        package: RecoveryPackage,
        token: u64,
        mut hook: F,
    ) -> Result<(), FileRecoveryStorageError>
    where
        F: FnMut(CommitStage) -> io::Result<()>,
    {
        let generation = package.manifest.generation;
        let generations_dir = self.root.join("generations");
        // Keep the unique staging directory as a direct child of the validated root.
        let stage_dir = self.root.join(format!(
            ".staging-g-{generation:020}-p{}-{token}",
            std::process::id()
        ));
        let final_dir = self.generation_dir(generation);
        let current_temp = self
            .root
            .join(format!(".CURRENT-p{}-{token}", std::process::id()));
        let mut stage_created = false;
        let mut current_temp_created = false;

        let result = (|| {
            // Cleanup below is permitted only after this operation created the path.
            fs::create_dir(&stage_dir)?;
            stage_created = true;
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

            // Reserve the temporary CURRENT name before publishing the generation.
            // A stale name collision must not leave a newly published orphan generation.
            let mut current_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&current_temp)?;
            current_temp_created = true;
            hook(CommitStage::CurrentTempReserved)?;

            fs::rename(&stage_dir, &final_dir)?;
            stage_created = false;
            hook(CommitStage::GenerationPublished)?;
            sync_directory(&generations_dir)?;
            hook(CommitStage::GenerationsDirectorySynced)?;

            let pointer = CurrentPointer {
                generation,
                manifest_digest: Sha256::digest(&manifest_bytes).into(),
            };
            current_file.write_all(&pointer.encode())?;
            current_file.sync_all()?;
            hook(CommitStage::CurrentSynced)?;
            fs::rename(&current_temp, self.root.join("CURRENT"))?;
            current_temp_created = false;
            hook(CommitStage::CurrentPublished)?;
            sync_directory(&self.root)?;
            hook(CommitStage::RootDirectorySynced)?;
            Ok(())
        })();

        if result.is_err() {
            if stage_created {
                let _ = fs::remove_dir_all(&stage_dir);
            }
            if current_temp_created {
                let _ = fs::remove_file(&current_temp);
            }
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
    CurrentTempReserved,
    GenerationPublished,
    GenerationsDirectorySynced,
    CurrentSynced,
    CurrentPublished,
    RootDirectorySynced,
}

fn open_recovery_artifact(
    path: &Path,
    name: &'static str,
) -> Result<(File, u64), FileRecoveryStorageError> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(FileRecoveryStorageError::InvalidRecoveryArtifact(name));
    }
    Ok((file, metadata.len()))
}

fn check_artifact_limit(
    artifact: &'static str,
    size: u64,
    limit: u64,
) -> Result<(), FileRecoveryStorageError> {
    if size > limit {
        return Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
            artifact,
            size,
            limit,
        });
    }
    Ok(())
}

fn read_bounded_artifact(
    file: File,
    artifact: &'static str,
    initial_size: u64,
    limit: u64,
) -> Result<Vec<u8>, FileRecoveryStorageError> {
    if initial_size > limit {
        return Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
            artifact,
            size: initial_size,
            limit,
        });
    }

    let capacity = usize::try_from(initial_size)
        .map_err(|_| FileRecoveryStorageError::ArtifactSizeOverflow)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|error| {
        FileRecoveryStorageError::Io(io::Error::new(
            io::ErrorKind::Other,
            format!("cannot reserve recovery artifact buffer: {error}"),
        ))
    })?;

    // Read in bounded chunks so Vec growth cannot overshoot the configured
    // budget. Once the limit is reached, probe one byte without appending it.
    let mut reader = file;
    let mut chunk = [0_u8; 8192];
    loop {
        let current_size = u64::try_from(bytes.len())
            .map_err(|_| FileRecoveryStorageError::ArtifactSizeOverflow)?;
        if current_size == limit {
            let mut extra = [0_u8; 1];
            if reader.read(&mut extra)? != 0 {
                return Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
                    artifact,
                    size: limit.saturating_add(1),
                    limit,
                });
            }
            break;
        }

        let remaining = limit - current_size;
        let chunk_size = remaining.min(chunk.len() as u64) as usize;
        let read = reader.read(&mut chunk[..chunk_size])?;
        if read == 0 {
            break;
        }

        bytes.try_reserve_exact(read).map_err(|error| {
            FileRecoveryStorageError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("cannot grow recovery artifact buffer: {error}"),
            ))
        })?;
        bytes.extend_from_slice(&chunk[..read]);
    }

    Ok(bytes)
}

fn create_dir_all_synced(path: &Path) -> Result<(), FileRecoveryStorageError> {
    // Resolve and sync every ancestor, not only components created by this call.
    // A concurrent opener may observe a directory after create_dir but before
    // the creating process syncs its parent. Re-syncing existing components
    // closes that durability window before this opener can publish CURRENT.
    let parent = path.parent().filter(|value| !value.as_os_str().is_empty());

    if let Some(parent) = parent {
        if parent != path {
            create_dir_all_synced(parent)?;
        }
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(FileRecoveryStorageError::InvalidGenerationDirectory);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).or_else(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })?;
            ensure_real_directory(path)?;
        }
        Err(error) => return Err(error.into()),
    }

    // For relative single-component paths the parent is ".", which is the
    // correct directory whose entry must be made durable.
    let sync_parent = parent.unwrap_or(Path::new("."));
    if sync_parent != path {
        sync_directory(sync_parent)?;
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
    fn recovery_artifact_per_file_budget_is_enforced_before_read() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
        storage.commit_generation(None, package(1)).unwrap();

        let limits = ArtifactReadLimits {
            max_snapshot_bytes: 0,
            ..ArtifactReadLimits::default()
        };
        let bounded = FileRecoveryStorage::open_with_limits(directory.path(), limits).unwrap();

        assert!(matches!(
            bounded.load_current(),
            Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
                artifact: "snapshot",
                ..
            })
        ));
    }

    #[test]
    fn recovery_artifact_aggregate_budget_is_enforced_before_read() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
        storage.commit_generation(None, package(1)).unwrap();

        let limits = ArtifactReadLimits {
            max_total_bytes: 0,
            ..ArtifactReadLimits::default()
        };
        let bounded = FileRecoveryStorage::open_with_limits(directory.path(), limits).unwrap();

        assert!(matches!(
            bounded.load_current(),
            Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
                artifact: "aggregate",
                ..
            })
        ));
    }

    #[test]
    fn recovery_artifact_growth_during_read_is_still_bounded() {
        let directory = TestDirectory::new();
        let mut storage = FileRecoveryStorage::open(directory.path()).unwrap();
        storage.commit_generation(None, package(1)).unwrap();

        let journal = directory
            .path()
            .join("generations")
            .join("g-00000000000000000001")
            .join("journal.bin");
        let file = OpenOptions::new().write(true).open(&journal).unwrap();
        file.set_len(128).unwrap();

        let limits = ArtifactReadLimits {
            max_journal_bytes: 64,
            max_total_bytes: 1024,
            ..ArtifactReadLimits::default()
        };
        let bounded = FileRecoveryStorage::open_with_limits(directory.path(), limits).unwrap();

        assert!(matches!(
            bounded.load_current(),
            Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
                artifact: "journal",
                ..
            })
        ));
    }

    #[test]
    fn bounded_reader_handles_zero_budget_without_buffer_growth() {
        let directory = TestDirectory::new();
        let empty_path = directory.path().join("empty.bin");
        fs::write(&empty_path, b"").unwrap();
        let empty = read_bounded_artifact(File::open(&empty_path).unwrap(), "manifest", 0, 0)
            .unwrap();
        assert!(empty.is_empty());

        let nonempty_path = directory.path().join("nonempty.bin");
        fs::write(&nonempty_path, b"\x01").unwrap();
        assert!(matches!(
            read_bounded_artifact(
                File::open(&nonempty_path).unwrap(),
                "manifest",
                0,
                0
            ),
            Err(FileRecoveryStorageError::ArtifactReadLimitExceeded {
                artifact: "manifest",
                size: 1,
                limit: 0
            })
        ));
    }

    #[test]
    fn bounded_reader_enforces_remaining_aggregate_budget_after_file_growth() {
        let directory = TestDirectory::new();
        let path = directory.path().join("growing.bin");
        fs::write(&path, vec![7_u8; 128]).unwrap();
        let file = File::open(&path).unwrap();

        let error = read_bounded_artifact(file, "aggregate", 0, 64).unwrap_err();
        assert!(matches!(
            error,
            FileRecoveryStorageError::ArtifactReadLimitExceeded {
                artifact: "aggregate",
                size: 65,
                limit: 64
            }
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
            CommitStage::CurrentTempReserved,
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

    #[test]
    fn preexisting_staging_collision_is_not_removed() {
        let directory = TestDirectory::new();
        let storage = FileRecoveryStorage::open(directory.path()).unwrap();
        let token = u64::MAX;
        let stage_dir = directory.path().join(format!(
            ".staging-g-{:020}-p{}-{token}",
            1,
            std::process::id()
        ));
        fs::create_dir(&stage_dir).unwrap();
        fs::write(stage_dir.join("preserve.bin"), b"previous-process").unwrap();

        assert!(storage
            .publish_with_token(package(1), token, |_| Ok(()))
            .is_err());
        assert_eq!(
            fs::read(stage_dir.join("preserve.bin")).unwrap(),
            b"previous-process"
        );
    }

    #[test]
    fn preexisting_current_temp_collision_is_not_removed() {
        let directory = TestDirectory::new();
        let storage = FileRecoveryStorage::open(directory.path()).unwrap();
        let token = u64::MAX - 1;
        let current_temp = directory
            .path()
            .join(format!(".CURRENT-p{}-{token}", std::process::id()));
        fs::write(&current_temp, b"previous-process").unwrap();

        assert!(storage
            .publish_with_token(package(1), token, |_| Ok(()))
            .is_err());
        assert_eq!(fs::read(&current_temp).unwrap(), b"previous-process");
        assert!(!storage.generation_dir(1).exists());
        assert!(!directory
            .path()
            .join(format!(
                ".staging-g-{:020}-p{}-{token}",
                1,
                std::process::id()
            ))
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

        assert_eq!(
            fs::read(outside.path().join("preserve.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(
            storage.load_current().unwrap().unwrap().manifest.generation,
            1
        );
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
