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
    InvalidSnapshotDigest,
    JournalLengthMismatch,
    InvalidCheckpointRange,
    LengthOverflow,
    Journal(JournalError),
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
            return Err(RecoveryError::LengthOverflow);
        }
        let (payload, supplied_digest) = bytes.split_at(PAYLOAD_LEN);
        if Sha256::digest(payload).as_slice() != supplied_digest {
            return Err(RecoveryError::InvalidDigest);
        }

        let mut reader = Reader { bytes: payload, pos: 0 };
        if reader.take(4)? != MAGIC {
            return Err(RecoveryError::InvalidMagic);
        }
        let version = reader.u16()?;
        if version != VERSION {
            return Err(RecoveryError::UnsupportedVersion(version));
        }
        let generation = reader.u64()?;
        let snapshot_digest = reader.take(DIGEST_LEN)?.try_into().map_err(|_| RecoveryError::Truncated)?;
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
    /// Проверяет manifest, целостность snapshot и полностью воспроизводит journal.
    pub fn restore(self) -> Result<(EventValidator, ReplayReport), RecoveryError> {
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

    /// Атомарно публикует полный пакет как текущий generation.
    /// Возврат Ok означает, что manifest и все данные durable согласно контракту backend.
    fn commit_generation(&mut self, package: RecoveryPackage) -> Result<(), Self::Error>;
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
        let end = self.pos.checked_add(len).ok_or(RecoveryError::LengthOverflow)?;
        if end > self.bytes.len() {
            return Err(RecoveryError::Truncated);
        }
        let result = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(result)
    }

    fn u16(&mut self) -> Result<u16, RecoveryError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(|_| RecoveryError::Truncated)?))
    }

    fn u64(&mut self) -> Result<u64, RecoveryError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| RecoveryError::Truncated)?))
    }

    fn checkpoint(&mut self) -> Result<JournalCheckpoint, RecoveryError> {
        let next_offset = self.u64()?;
        let last_hash = self.take(DIGEST_LEN)?.try_into().map_err(|_| RecoveryError::Truncated)?;
        Ok(JournalCheckpoint { next_offset, last_hash })
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
            Quote { bid_ticks: 100, ask_ticks: 101, price_scale: 10 },
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
        RecoveryPackage { snapshot, journal, manifest }
    }

    #[test]
    fn manifest_round_trip_is_canonical() {
        let manifest = package().manifest;
        let encoded = manifest.encode();
        assert_eq!(RecoveryManifest::decode(&encoded).unwrap(), manifest);
        assert_eq!(RecoveryManifest::decode(&encoded).unwrap().encode(), encoded);
    }

    #[test]
    fn package_restores_only_after_all_cross_checks() {
        let package = package();
        let (validator, report) = package.restore().unwrap();
        assert_eq!(report.records_replayed, 1);
        assert_eq!(validator.validate(&quote("e2", 2)), Ok(()));
    }

    #[test]
    fn rejects_manifest_corruption() {
        let mut encoded = package().manifest.encode();
        encoded[12] ^= 1;
        assert_eq!(RecoveryManifest::decode(&encoded), Err(RecoveryError::InvalidDigest));
    }

    #[test]
    fn rejects_snapshot_manifest_mismatch() {
        let mut package = package();
        package.snapshot[0] ^= 1;
        assert_eq!(package.restore(), Err(RecoveryError::InvalidSnapshotDigest));
    }

    #[test]
    fn rejects_journal_truncation_even_at_record_boundary() {
        let mut package = package();
        package.journal.clear();
        assert_eq!(package.restore(), Err(RecoveryError::JournalLengthMismatch));
    }
}
