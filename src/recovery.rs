//! Контейнер согласованного recovery generation: snapshot + journal + manifest.

use sha2::{Digest, Sha256};

use crate::{EventValidator, JournalCheckpoint, JournalError, ReplayReport};

const MAGIC: &[u8; 4] = b"SGFM";
const VERSION: u16 = 1;
const DIGEST_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidDigest,
    InvalidLength,
    InvalidSnapshotDigest,
    JournalLengthMismatch,
    InvalidCheckpointRange,
    LengthOverflow,
    Journal(JournalError),
}

/// Ошибка проверки перехода между поколениями.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryGenerationError {
    /// Текущее поколение изменилось после чтения вызывающим кодом.
    GenerationConflict {
        expected_current: Option<u64>,
        actual_current: Option<u64>,
    },
    /// Невозможно увеличить номер текущего поколения.
    GenerationOverflow,
    /// Новое поколение не является непосредственным преемником текущего.
    InvalidNextGeneration { expected: u64, actual: u64 },
}

/// Ошибка commit в RecoveryStorage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryStorageError<E> {
    Generation(RecoveryGenerationError),
    InvalidPackage(RecoveryError),
    Backend(E),
}

/// Проверяет compare-and-swap условие и строгое увеличение generation.
///
/// Backend обязан вызывать эту функцию внутри своей атомарной секции записи,
/// используя фактическое текущее поколение, прочитанное под той же блокировкой/CAS.
pub fn validate_generation_transition(
    expected_current: Option<u64>,
    actual_current: Option<u64>,
    candidate_generation: u64,
) -> Result<(), RecoveryGenerationError> {
    if expected_current != actual_current {
        return Err(RecoveryGenerationError::GenerationConflict {
            expected_current,
            actual_current,
        });
    }

    let expected_next = match actual_current {
        Some(current) => current
            .checked_add(1)
            .ok_or(RecoveryGenerationError::GenerationOverflow)?,
        None => 1,
    };

    if candidate_generation != expected_next {
        return Err(RecoveryGenerationError::InvalidNextGeneration {
            expected: expected_next,
            actual: candidate_generation,
        });
    }

    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryManifest {
    /// Монотонный номер поколения recovery bundle, задаваемый storage-слоем.
    pub generation: u64,
    /// SHA-256 полного snapshot blob.
    pub snapshot_digest: [u8; DIGEST_LEN],
    /// Ожидаемая длина journal segment в байтах.
    pub journal_bytes: u64,
    /// Точка журнала, соответствующая snapshot.
    pub anchor: JournalCheckpoint,
    /// Долговечно подтверждённый конец journal segment.
    pub expected_end: JournalCheckpoint,
}

impl RecoveryManifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + 2 + 8 + 32 + 8 + 40 + 40 + DIGEST_LEN);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.generation.to_le_bytes());
        bytes.extend_from_slice(&self.snapshot_digest);
        bytes.extend_from_slice(&self.journal_bytes.to_le_bytes());
        put_checkpoint(&mut bytes, &self.anchor);
        put_checkpoint(&mut bytes, &self.expected_end);
        let digest = Sha256::digest(&bytes);
        bytes.extend_from_slice(&digest);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RecoveryError> {
        const PAYLOAD_LEN: usize = 4 + 2 + 8 + 32 + 8 + 40 + 40;
        if bytes.len() < PAYLOAD_LEN + DIGEST_LEN {
            return Err(RecoveryError::Truncated);
        }
        if bytes.len() != PAYLOAD_LEN + DIGEST_LEN {
            return Err(RecoveryError::InvalidLength);
        }
        let (payload, supplied_digest) = bytes.split_at(PAYLOAD_LEN);
        if Sha256::digest(payload).as_slice() != supplied_digest {
            return Err(RecoveryError::InvalidDigest);
        }

        let mut reader = Reader {
            bytes: payload,
            pos: 0,
        };
        if reader.take(4)? != MAGIC {
            return Err(RecoveryError::InvalidMagic);
        }
        let version = reader.u16()?;
        if version != VERSION {
            return Err(RecoveryError::UnsupportedVersion(version));
        }
        let generation = reader.u64()?;
        let snapshot_digest = reader
            .take(DIGEST_LEN)?
            .try_into()
            .map_err(|_| RecoveryError::Truncated)?;
        let journal_bytes = reader.u64()?;
        let anchor = reader.checkpoint()?;
        let expected_end = reader.checkpoint()?;
        if reader.pos != payload.len() {
            return Err(RecoveryError::LengthOverflow);
        }
        if expected_end.next_offset < anchor.next_offset {
            return Err(RecoveryError::InvalidCheckpointRange);
        }

        Ok(Self {
            generation,
            snapshot_digest,
            journal_bytes,
            anchor,
            expected_end,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryPackage {
    pub snapshot: Vec<u8>,
    pub journal: Vec<u8>,
    pub manifest: RecoveryManifest,
}

impl RecoveryPackage {
    /// Проверяет целостность пакета и полностью воспроизводит его без изменения пакета.
    pub fn validate(&self) -> Result<(), RecoveryError> {
        self.restore_inner().map(|_| ())
    }

    /// Проверяет manifest, целостность snapshot и полностью воспроизводит journal.
    pub fn restore(self) -> Result<(EventValidator, ReplayReport), RecoveryError> {
        self.restore_inner()
    }

    fn restore_inner(&self) -> Result<(EventValidator, ReplayReport), RecoveryError> {
        if self.journal.len() as u64 != self.manifest.journal_bytes {
            return Err(RecoveryError::JournalLengthMismatch);
        }
        if Sha256::digest(&self.snapshot).as_slice() != self.manifest.snapshot_digest {
            return Err(RecoveryError::InvalidSnapshotDigest);
        }
        if self.manifest.expected_end.next_offset < self.manifest.anchor.next_offset {
            return Err(RecoveryError::InvalidCheckpointRange);
        }

        EventValidator::restore_and_replay(
            &self.snapshot,
            &self.journal,
            &self.manifest.anchor,
            &self.manifest.expected_end,
        )
        .map_err(RecoveryError::Journal)
    }
}

/// Абстракция долговременного хранилища. Реализация обязана публиковать generation
/// атомарно: после аварии load_current возвращает либо предыдущий полный generation,
/// либо новый полный generation, но никогда смешанные snapshot/journal/manifest.
pub trait RecoveryStorage {
    type Error;

    fn load_current(&self) -> Result<Option<RecoveryPackage>, Self::Error>;

    /// Compare-and-swap commit полного поколения.
    ///
    /// Реализация обязана внутри одной атомарной секции:
    /// 1. прочитать фактическое текущее generation;
    /// 2. проверить его через validate_generation_transition;
    /// 3. проверить package.validate();
    /// 4. durable записать все части пакета;
    /// 5. атомарно опубликовать новый current generation.
    ///
    /// expected_current_generation=None разрешён только для создания первого поколения 1.
    /// При существующем поколении G допускается только G+1. Устаревший writer получает
    /// GenerationConflict; пропуск поколений и повторное использование номера запрещены.
    fn commit_generation(
        &mut self,
        expected_current_generation: Option<u64>,
        package: RecoveryPackage,
    ) -> Result<(), RecoveryStorageError<Self::Error>>;
}

fn put_checkpoint(out: &mut Vec<u8>, checkpoint: &JournalCheckpoint) {
    out.extend_from_slice(&checkpoint.next_offset.to_le_bytes());
    out.extend_from_slice(&checkpoint.last_hash);
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], RecoveryError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(RecoveryError::LengthOverflow)?;
        if end > self.bytes.len() {
            return Err(RecoveryError::Truncated);
        }
        let result = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(result)
    }

    fn u16(&mut self) -> Result<u16, RecoveryError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| RecoveryError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, RecoveryError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| RecoveryError::Truncated)?,
        ))
    }

    fn checkpoint(&mut self) -> Result<JournalCheckpoint, RecoveryError> {
        let next_offset = self.u64()?;
        let last_hash = self
            .take(DIGEST_LEN)?
            .try_into()
            .map_err(|_| RecoveryError::Truncated)?;
        Ok(JournalCheckpoint {
            next_offset,
            last_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_record, MarketEvent, Quote};

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

    fn package() -> RecoveryPackage {
        let snapshot = EventValidator::new().encode_snapshot().unwrap();
        let anchor = JournalCheckpoint::default();
        let journal = encode_record(&anchor, &quote("e1", 1)).unwrap();
        let expected_end = JournalCheckpoint {
            next_offset: 1,
            last_hash: journal[journal.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let manifest = RecoveryManifest {
            generation: 1,
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
    fn manifest_round_trip_is_canonical() {
        let manifest = package().manifest;
        let encoded = manifest.encode();
        assert_eq!(RecoveryManifest::decode(&encoded).unwrap(), manifest);
        assert_eq!(
            RecoveryManifest::decode(&encoded).unwrap().encode(),
            encoded
        );
    }

    #[test]
    fn package_restores_only_after_all_cross_checks() {
        let package = package();
        let (mut validator, report) = package.restore().unwrap();
        assert_eq!(report.records_replayed, 1);
        assert_eq!(validator.validate(&quote("e2", 2)), Ok(()));
    }

    #[test]
    fn rejects_manifest_corruption() {
        let mut encoded = package().manifest.encode();
        encoded[12] ^= 1;
        assert_eq!(
            RecoveryManifest::decode(&encoded),
            Err(RecoveryError::InvalidDigest)
        );
    }

    #[test]
    fn rejects_snapshot_manifest_mismatch() {
        let mut package = package();
        package.snapshot[0] ^= 1;
        assert!(matches!(
            package.restore(),
            Err(RecoveryError::InvalidSnapshotDigest)
        ));
    }

    #[test]
    fn rejects_journal_truncation_even_at_record_boundary() {
        let mut package = package();
        package.journal.clear();
        assert!(matches!(
            package.restore(),
            Err(RecoveryError::JournalLengthMismatch)
        ));
    }
    #[test]
    fn generation_transition_requires_compare_and_swap() {
        assert_eq!(validate_generation_transition(None, None, 1), Ok(()));
        assert_eq!(
            validate_generation_transition(Some(4), Some(5), 6),
            Err(RecoveryGenerationError::GenerationConflict {
                expected_current: Some(4),
                actual_current: Some(5),
            })
        );
    }

    #[test]
    fn generation_transition_requires_exact_successor() {
        assert_eq!(
            validate_generation_transition(Some(4), Some(4), 6),
            Err(RecoveryGenerationError::InvalidNextGeneration {
                expected: 5,
                actual: 6,
            })
        );
        assert_eq!(validate_generation_transition(Some(4), Some(4), 5), Ok(()));
    }

    #[test]
    fn generation_transition_detects_overflow() {
        assert_eq!(
            validate_generation_transition(Some(u64::MAX), Some(u64::MAX), 0),
            Err(RecoveryGenerationError::GenerationOverflow)
        );
    }

    #[test]
    fn package_can_be_validated_without_consuming_it() {
        let package = package();
        assert_eq!(package.validate(), Ok(()));
        assert_eq!(package.manifest.generation, 1);
    }
}
