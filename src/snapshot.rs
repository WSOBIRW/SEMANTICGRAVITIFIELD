//! Версионированный снимок состояния валидатора с канонической кодировкой.
//! Модуль не выполняет файловый ввод-вывод: выбор backend и атомарная запись
//! остаются ответственностью интеграционного слоя.

use std::collections::{HashSet, VecDeque};
use std::num::NonZeroUsize;

use sha2::{Digest, Sha256};

use crate::validation::EventValidator;

const MAGIC: &[u8; 4] = b"SGFS";
const FORMAT_VERSION: u16 = 1;
const DIGEST_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidMode,
    InvalidCapacity,
    InvalidUtf8,
    InvalidOrdering,
    DuplicateEventId,
    DuplicateStream,
    InvalidDigest,
    TrailingData,
    LengthOverflow,
}

impl EventValidator {
    /// Экспортирует полное состояние в детерминированный бинарный формат.
    /// Повторный экспорт неизменённого состояния даёт побайтно одинаковый результат.
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, SnapshotError> {
        let mut payload = Vec::new();
        payload.extend_from_slice(MAGIC);
        payload.extend_from_slice(&FORMAT_VERSION.to_le_bytes());

        match self.dedup_capacity {
            Some(capacity) => {
                payload.push(1);
                payload.extend_from_slice(&(capacity.get() as u64).to_le_bytes());
                if self.event_id_order.len() > capacity.get()
                    || self.event_id_order.len() != self.seen_event_ids.len()
                {
                    return Err(SnapshotError::InvalidCapacity);
                }

                let mut unique = HashSet::new();
                for id in &self.event_id_order {
                    if !unique.insert(id) || !self.seen_event_ids.contains(id) {
                        return Err(SnapshotError::DuplicateEventId);
                    }
                }

                put_count(&mut payload, self.event_id_order.len())?;
                for id in &self.event_id_order {
                    put_string(&mut payload, id)?;
                }
            }
            None => {
                if !self.event_id_order.is_empty() {
                    return Err(SnapshotError::InvalidMode);
                }

                payload.push(0);
                payload.extend_from_slice(&0u64.to_le_bytes());
                let mut ids: Vec<&str> = self.seen_event_ids.iter().map(String::as_str).collect();
                ids.sort_unstable();
                put_count(&mut payload, ids.len())?;
                for id in ids {
                    put_string(&mut payload, id)?;
                }
            }
        }

        let mut streams: Vec<(&(String, String), &u64)> = self.last_sequences.iter().collect();
        streams.sort_unstable_by(|a, b| a.0.cmp(b.0));
        put_count(&mut payload, streams.len())?;
        for ((source, instrument), sequence) in streams {
            put_string(&mut payload, source)?;
            put_string(&mut payload, instrument)?;
            payload.extend_from_slice(&sequence.to_le_bytes());
        }

        let digest = Sha256::digest(&payload);
        payload.extend_from_slice(&digest);
        Ok(payload)
    }

    /// Восстанавливает состояние только после проверки формата, SHA-256 и инвариантов.
    pub fn restore_snapshot(bytes: &[u8]) -> Result<Self, SnapshotError> {
        if bytes.len() < DIGEST_LEN {
            return Err(SnapshotError::Truncated);
        }

        let split = bytes.len() - DIGEST_LEN;
        let (payload, supplied_digest) = bytes.split_at(split);
        let expected_digest = Sha256::digest(payload);
        if expected_digest.as_slice() != supplied_digest {
            return Err(SnapshotError::InvalidDigest);
        }

        let mut reader = Reader {
            bytes: payload,
            pos: 0,
        };
        if reader.take(4)? != MAGIC {
            return Err(SnapshotError::InvalidMagic);
        }

        let version = reader.u16()?;
        if version != FORMAT_VERSION {
            return Err(SnapshotError::UnsupportedVersion(version));
        }

        let mode = reader.u8()?;
        let capacity_raw = reader.u64()?;
        let capacity = match mode {
            0 if capacity_raw == 0 => None,
            1 if capacity_raw > 0 && capacity_raw <= usize::MAX as u64 => Some(
                NonZeroUsize::new(capacity_raw as usize).ok_or(SnapshotError::InvalidCapacity)?,
            ),
            0 | 1 => return Err(SnapshotError::InvalidCapacity),
            _ => return Err(SnapshotError::InvalidMode),
        };

        let id_count = reader.count()?;
        if capacity.is_some_and(|cap| id_count > cap.get()) {
            return Err(SnapshotError::InvalidCapacity);
        }
        // id_count поступает из недоверенных байтов. Не резервируем память по нему заранее.
        let mut seen_event_ids = HashSet::new();
        let mut event_id_order = VecDeque::new();
        let mut previous: Option<String> = None;

        for _ in 0..id_count {
            let id = reader.string()?;
            if id.trim().is_empty() {
                return Err(SnapshotError::InvalidOrdering);
            }
            if !seen_event_ids.insert(id.clone()) {
                return Err(SnapshotError::DuplicateEventId);
            }

            if capacity.is_some() {
                event_id_order.push_back(id);
            } else {
                if previous.as_ref().is_some_and(|p| p >= &id) {
                    return Err(SnapshotError::InvalidOrdering);
                }
                previous = Some(id);
            }
        }

        if let Some(cap) = capacity {
            if id_count > cap.get() {
                return Err(SnapshotError::InvalidCapacity);
            }
        }

        let stream_count = reader.count()?;
        let mut last_sequences = std::collections::HashMap::with_capacity(stream_count);
        let mut previous_key: Option<(String, String)> = None;

        for _ in 0..stream_count {
            let source = reader.string()?;
            let instrument = reader.string()?;
            if source.trim().is_empty() || instrument.trim().is_empty() {
                return Err(SnapshotError::InvalidOrdering);
            }

            let sequence = reader.u64()?;
            let key = (source, instrument);
            if previous_key.as_ref().is_some_and(|p| p >= &key) {
                return Err(SnapshotError::InvalidOrdering);
            }
            previous_key = Some(key.clone());

            if last_sequences.insert(key, sequence).is_some() {
                return Err(SnapshotError::DuplicateStream);
            }
        }

        if reader.pos != payload.len() {
            return Err(SnapshotError::TrailingData);
        }

        Ok(Self {
            seen_event_ids,
            event_id_order,
            dedup_capacity: capacity,
            last_sequences,
        })
    }
}

fn put_count(out: &mut Vec<u8>, count: usize) -> Result<(), SnapshotError> {
    let value = u32::try_from(count).map_err(|_| SnapshotError::LengthOverflow)?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), SnapshotError> {
    let len = u32::try_from(value.len()).map_err(|_| SnapshotError::LengthOverflow)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(SnapshotError::LengthOverflow)?;
        if end > self.bytes.len() {
            return Err(SnapshotError::Truncated);
        }

        let result = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SnapshotError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| SnapshotError::Truncated)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| SnapshotError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| SnapshotError::Truncated)?,
        ))
    }

    fn count(&mut self) -> Result<usize, SnapshotError> {
        let count = self.u32()? as usize;
        if count > self.bytes.len().saturating_sub(self.pos) / 4 {
            return Err(SnapshotError::Truncated);
        }
        Ok(count)
    }

    fn string(&mut self) -> Result<String, SnapshotError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SnapshotError::InvalidUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Quote;

    fn event(id: &str, source: &str, instrument: &str, seq: u64) -> crate::MarketEvent {
        crate::MarketEvent::quote(
            id,
            source,
            instrument,
            100,
            110,
            Some(seq),
            Quote {
                bid_ticks: 100,
                ask_ticks: 101,
                price_scale: 10,
            },
        )
    }

    #[test]
    fn snapshot_round_trip_preserves_fifo_and_watermarks() {
        let mut original = EventValidator::with_dedup_capacity(NonZeroUsize::new(2).unwrap());
        original.validate(&event("a", "s1", "i1", 10)).unwrap();
        original.validate(&event("b", "s1", "i1", 12)).unwrap();
        original.validate(&event("c", "s2", "i2", 7)).unwrap();

        let bytes = original.encode_snapshot().unwrap();
        let mut restored = EventValidator::restore_snapshot(&bytes).unwrap();
        assert_eq!(bytes, restored.encode_snapshot().unwrap());
        assert_eq!(
            restored.validate(&event("b", "s3", "i3", 1)),
            Err(crate::ValidationError::DuplicateEventId)
        );
        assert_eq!(restored.validate(&event("a", "s3", "i3", 1)), Ok(()));
        assert_eq!(
            restored.validate(&event("old-sequence", "s1", "i1", 11)),
            Err(crate::ValidationError::NonIncreasingSequence {
                previous: 12,
                current: 11
            })
        );
        assert_eq!(restored.validate(&event("next", "s1", "i1", 13)), Ok(()));
    }

    #[test]
    fn unbounded_snapshot_is_canonical_independent_of_hash_iteration_order() {
        let mut validator = EventValidator::new();
        validator.validate(&event("z", "z", "z", 1)).unwrap();
        validator.validate(&event("a", "a", "a", 1)).unwrap();

        let bytes = validator.encode_snapshot().unwrap();
        let restored = EventValidator::restore_snapshot(&bytes).unwrap();
        assert_eq!(bytes, restored.encode_snapshot().unwrap());
    }

    #[test]
    fn detects_corruption_before_restore() {
        let mut validator = EventValidator::new();
        validator.validate(&event("a", "s", "i", 1)).unwrap();

        let mut bytes = validator.encode_snapshot().unwrap();
        bytes[7] ^= 0x40;
        assert!(matches!(
            EventValidator::restore_snapshot(&bytes),
            Err(SnapshotError::InvalidDigest)
        ));
    }

    #[test]
    fn rejects_truncated_snapshot() {
        assert!(matches!(
            EventValidator::restore_snapshot(b"SGF"),
            Err(SnapshotError::Truncated)
        ));
    }
    #[test]
    fn rejects_bounded_snapshot_count_above_capacity_before_reading_ids() {
        let mut validator =
            EventValidator::with_dedup_capacity(NonZeroUsize::new(1).unwrap());
        validator.validate(&event("a", "s", "i", 1)).unwrap();
        let mut bytes = validator.encode_snapshot().unwrap();

        // Заголовок: magic(4) + version(2) + mode(1) + capacity(8), затем id_count.
        let count_offset = 4 + 2 + 1 + 8;
        bytes[count_offset..count_offset + 4].copy_from_slice(&2u32.to_le_bytes());

        let payload_len = bytes.len() - DIGEST_LEN;
        let digest = Sha256::digest(&bytes[..payload_len]);
        bytes[payload_len..].copy_from_slice(&digest);

        assert!(matches!(
            EventValidator::restore_snapshot(&bytes),
            Err(SnapshotError::InvalidCapacity)
        ));
    }

}
