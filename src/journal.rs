//! Журнал событий с последовательной нумерацией и SHA-256 цепочкой.
//! Файловый ввод-вывод и атомарная фиксация остаются ответственностью storage-слоя.

use sha2::{Digest, Sha256};

use crate::{EventPayload, EventValidator, MarketEvent, ValidationError};

const MAGIC: &[u8; 4] = b"SGFJ";
const VERSION: u16 = 1;
const DIGEST_LEN: usize = 32;
const ZERO_HASH: [u8; DIGEST_LEN] = [0; DIGEST_LEN];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalCheckpoint {
    /// Offset следующей записи. Первая запись журнала имеет offset 0.
    pub next_offset: u64,
    /// SHA-256 последней подтверждённой записи; нулевой хэш означает пустой журнал.
    pub last_hash: [u8; DIGEST_LEN],
}

impl Default for JournalCheckpoint {
    fn default() -> Self {
        Self {
            next_offset: 0,
            last_hash: ZERO_HASH,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalError {
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidDigest,
    BrokenHashChain {
        offset: u64,
    },
    UnexpectedOffset {
        expected: u64,
        actual: u64,
    },
    InvalidUtf8,
    InvalidEventEncoding,
    LengthOverflow,
    Snapshot(crate::SnapshotError),
    CheckpointMismatch {
        expected: JournalCheckpoint,
        actual: JournalCheckpoint,
    },
    Validation {
        offset: u64,
        error: ValidationError,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    pub checkpoint: JournalCheckpoint,
    pub records_replayed: u64,
}

/// Кодирует одну запись. Вызывающий слой обязан использовать checkpoint,
/// полученный после последней успешно зафиксированной записи.
pub fn encode_record(
    checkpoint: &JournalCheckpoint,
    event: &MarketEvent,
) -> Result<Vec<u8>, JournalError> {
    let mut body = Vec::new();
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&VERSION.to_le_bytes());
    body.extend_from_slice(&checkpoint.next_offset.to_le_bytes());
    body.extend_from_slice(&checkpoint.last_hash);
    body.push(0);
    encode_event(&mut body, event)?;

    let digest = Sha256::digest(&body);
    body.extend_from_slice(&digest);

    let len = u32::try_from(body.len()).map_err(|_| JournalError::LengthOverflow)?;
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Кодирует явную операцию сброса sequence-watermark потока.
/// Такая запись обязательна для точного replay reconnect-сценариев.
pub fn encode_reset_record(
    checkpoint: &JournalCheckpoint,
    source_id: &str,
    instrument_id: &str,
) -> Result<Vec<u8>, JournalError> {
    let mut body = Vec::new();
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&VERSION.to_le_bytes());
    body.extend_from_slice(&checkpoint.next_offset.to_le_bytes());
    body.extend_from_slice(&checkpoint.last_hash);
    body.push(1);
    put_string(&mut body, source_id)?;
    put_string(&mut body, instrument_id)?;

    let digest = Sha256::digest(&body);
    body.extend_from_slice(&digest);

    let len = u32::try_from(body.len()).map_err(|_| JournalError::LengthOverflow)?;
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

impl EventValidator {
    /// Воспроизводит весь переданный сегмент журнала поверх текущего состояния.
    /// При ошибке вызывающая сторона должна отбросить этот экземпляр валидатора:
    /// ранее воспроизведенные записи уже применены к нему.
    pub fn replay_journal(
        &mut self,
        bytes: &[u8],
        anchor: &JournalCheckpoint,
        expected_end: &JournalCheckpoint,
    ) -> Result<ReplayReport, JournalError> {
        let mut reader = Reader { bytes, pos: 0 };
        let mut checkpoint = anchor.clone();
        let mut replayed = 0u64;

        while reader.pos < bytes.len() {
            let frame_len = reader.u32()? as usize;
            if frame_len < 4 + 2 + 8 + DIGEST_LEN + DIGEST_LEN {
                return Err(JournalError::Truncated);
            }
            let frame = reader.take(frame_len)?;
            let payload_len = frame.len() - DIGEST_LEN;
            let (payload, supplied_digest) = frame.split_at(payload_len);
            let expected_digest = Sha256::digest(payload);
            if expected_digest.as_slice() != supplied_digest {
                return Err(JournalError::InvalidDigest);
            }

            let mut record = Reader {
                bytes: payload,
                pos: 0,
            };
            if record.take(4)? != MAGIC {
                return Err(JournalError::InvalidMagic);
            }
            let version = record.u16()?;
            if version != VERSION {
                return Err(JournalError::UnsupportedVersion(version));
            }
            let offset = record.u64()?;
            if offset != checkpoint.next_offset {
                return Err(JournalError::UnexpectedOffset {
                    expected: checkpoint.next_offset,
                    actual: offset,
                });
            }
            let previous_hash = record.take(DIGEST_LEN)?;
            if previous_hash != checkpoint.last_hash {
                return Err(JournalError::BrokenHashChain { offset });
            }

            match record.u8()? {
                0 => {
                    let event = decode_event(&mut record)?;
                    if record.pos != payload.len() {
                        return Err(JournalError::InvalidEventEncoding);
                    }
                    self.inspect_with_warnings(&event)
                        .map_err(|error| JournalError::Validation { offset, error })?;
                }
                1 => {
                    let source_id = record.string()?;
                    let instrument_id = record.string()?;
                    if record.pos != payload.len()
                        || source_id.trim().is_empty()
                        || instrument_id.trim().is_empty()
                    {
                        return Err(JournalError::InvalidEventEncoding);
                    }
                    self.reset_stream(&source_id, &instrument_id);
                }
                _ => return Err(JournalError::InvalidEventEncoding),
            }

            checkpoint.next_offset = checkpoint
                .next_offset
                .checked_add(1)
                .ok_or(JournalError::LengthOverflow)?;
            checkpoint.last_hash.copy_from_slice(supplied_digest);
            replayed = replayed
                .checked_add(1)
                .ok_or(JournalError::LengthOverflow)?;
        }

        if &checkpoint != expected_end {
            return Err(JournalError::CheckpointMismatch {
                expected: expected_end.clone(),
                actual: checkpoint,
            });
        }

        Ok(ReplayReport {
            checkpoint,
            records_replayed: replayed,
        })
    }

    /// Восстанавливает snapshot и воспроизводит последующий сегмент журнала.
    /// Ошибка не возвращает частично восстановленный валидатор.
    pub fn restore_and_replay(
        snapshot: &[u8],
        journal: &[u8],
        anchor: &JournalCheckpoint,
        expected_end: &JournalCheckpoint,
    ) -> Result<(Self, ReplayReport), JournalError> {
        let mut validator = Self::restore_snapshot(snapshot).map_err(JournalError::Snapshot)?;
        let report = validator.replay_journal(journal, anchor, expected_end)?;
        Ok((validator, report))
    }
}

fn encode_event(out: &mut Vec<u8>, event: &MarketEvent) -> Result<(), JournalError> {
    out.extend_from_slice(&event.schema_version.to_le_bytes());
    put_string(out, &event.event_id)?;
    put_string(out, &event.source_id)?;
    put_string(out, &event.instrument_id)?;
    out.extend_from_slice(&event.event_time_ms.to_le_bytes());
    out.extend_from_slice(&event.received_time_ms.to_le_bytes());

    match event.sequence {
        Some(sequence) => {
            out.push(1);
            out.extend_from_slice(&sequence.to_le_bytes());
        }
        None => out.push(0),
    }

    match &event.payload {
        EventPayload::Quote {
            bid_ticks,
            ask_ticks,
            price_scale,
        } => {
            out.push(0);
            out.extend_from_slice(&bid_ticks.to_le_bytes());
            out.extend_from_slice(&ask_ticks.to_le_bytes());
            out.extend_from_slice(&price_scale.to_le_bytes());
        }
        EventPayload::Trade {
            price_ticks,
            quantity_lots,
            price_scale,
        } => {
            out.push(1);
            out.extend_from_slice(&price_ticks.to_le_bytes());
            out.extend_from_slice(&quantity_lots.to_le_bytes());
            out.extend_from_slice(&price_scale.to_le_bytes());
        }
    }
    Ok(())
}

fn decode_event(reader: &mut Reader<'_>) -> Result<MarketEvent, JournalError> {
    let schema_version = reader.u16()?;
    let event_id = reader.string()?;
    let source_id = reader.string()?;
    let instrument_id = reader.string()?;
    let event_time_ms = reader.i64()?;
    let received_time_ms = reader.i64()?;
    let sequence = match reader.u8()? {
        0 => None,
        1 => Some(reader.u64()?),
        _ => return Err(JournalError::InvalidEventEncoding),
    };

    let payload = match reader.u8()? {
        0 => EventPayload::Quote {
            bid_ticks: reader.i64()?,
            ask_ticks: reader.i64()?,
            price_scale: reader.u32()?,
        },
        1 => EventPayload::Trade {
            price_ticks: reader.i64()?,
            quantity_lots: reader.u64()?,
            price_scale: reader.u32()?,
        },
        _ => return Err(JournalError::InvalidEventEncoding),
    };

    Ok(MarketEvent {
        schema_version,
        event_id,
        source_id,
        instrument_id,
        event_time_ms,
        received_time_ms,
        sequence,
        payload,
    })
}

fn put_string(out: &mut Vec<u8>, value: &str) -> Result<(), JournalError> {
    let len = u32::try_from(value.len()).map_err(|_| JournalError::LengthOverflow)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], JournalError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or(JournalError::LengthOverflow)?;
        if end > self.bytes.len() {
            return Err(JournalError::Truncated);
        }
        let result = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(result)
    }

    fn u8(&mut self) -> Result<u8, JournalError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, JournalError> {
        Ok(u16::from_le_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| JournalError::Truncated)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, JournalError> {
        Ok(u32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| JournalError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, JournalError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| JournalError::Truncated)?,
        ))
    }

    fn i64(&mut self) -> Result<i64, JournalError> {
        Ok(i64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| JournalError::Truncated)?,
        ))
    }

    fn string(&mut self) -> Result<String, JournalError> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| JournalError::InvalidUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Quote;

    fn quote(id: &str, seq: u64) -> MarketEvent {
        MarketEvent::quote(
            id,
            "feed-a",
            "BTCUSD",
            100 + seq as i64,
            110 + seq as i64,
            Some(seq),
            Quote {
                bid_ticks: 100,
                ask_ticks: 101,
                price_scale: 10,
            },
        )
    }

    #[test]
    fn records_round_trip_and_chain() {
        let mut checkpoint = JournalCheckpoint::default();
        let first = encode_record(&checkpoint, &quote("e1", 1)).unwrap();
        let mut validator = EventValidator::new();
        let first_end = JournalCheckpoint {
            next_offset: 1,
            last_hash: first[first.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let report = validator
            .replay_journal(&first, &checkpoint, &first_end)
            .unwrap();
        assert_eq!(report.records_replayed, 1);
        checkpoint = report.checkpoint;

        let second = encode_record(&checkpoint, &quote("e2", 2)).unwrap();
        let second_end = JournalCheckpoint {
            next_offset: 2,
            last_hash: second[second.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let report = validator
            .replay_journal(&second, &checkpoint, &second_end)
            .unwrap();
        assert_eq!(report.records_replayed, 1);
        assert_eq!(report.checkpoint.next_offset, 2);
    }

    #[test]
    fn rejects_replayed_segment_from_wrong_anchor() {
        let bytes = encode_record(&JournalCheckpoint::default(), &quote("e1", 1)).unwrap();
        let mut validator = EventValidator::new();
        let wrong = JournalCheckpoint {
            next_offset: 1,
            last_hash: [7; DIGEST_LEN],
        };
        assert!(matches!(
            validator.replay_journal(&bytes, &wrong, &JournalCheckpoint::default()),
            Err(JournalError::UnexpectedOffset { .. })
        ));
    }

    #[test]
    fn rejects_tampered_record() {
        let mut bytes = encode_record(&JournalCheckpoint::default(), &quote("e1", 1)).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let mut validator = EventValidator::new();
        assert_eq!(
            validator.replay_journal(
                &bytes,
                &JournalCheckpoint::default(),
                &JournalCheckpoint {
                    next_offset: 1,
                    last_hash: bytes[bytes.len() - DIGEST_LEN..].try_into().unwrap(),
                }
            ),
            Err(JournalError::InvalidDigest)
        );
    }

    #[test]
    fn rejects_truncated_tail_without_silent_acceptance() {
        let mut bytes = encode_record(&JournalCheckpoint::default(), &quote("e1", 1)).unwrap();
        let expected_end = JournalCheckpoint {
            next_offset: 1,
            last_hash: bytes[bytes.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        bytes.pop();
        let mut validator = EventValidator::new();
        assert!(matches!(
            validator.replay_journal(&bytes, &JournalCheckpoint::default(), &expected_end),
            Err(JournalError::Truncated)
        ));

        assert!(matches!(
            validator.replay_journal(&[], &JournalCheckpoint::default(), &expected_end),
            Err(JournalError::CheckpointMismatch { .. })
        ));
    }

    #[test]
    fn replay_is_deterministic_from_snapshot() {
        let snapshot = EventValidator::new().encode_snapshot().unwrap();
        let first = encode_record(&JournalCheckpoint::default(), &quote("e1", 1)).unwrap();
        let checkpoint = JournalCheckpoint {
            next_offset: 1,
            last_hash: first[first.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let second = encode_record(&checkpoint, &quote("e2", 2)).unwrap();
        let expected_end = JournalCheckpoint {
            next_offset: 2,
            last_hash: second[second.len() - DIGEST_LEN..].try_into().unwrap(),
        };

        let mut segment = first;
        segment.extend_from_slice(&second);
        let (replayed, report) = EventValidator::restore_and_replay(
            &snapshot,
            &segment,
            &JournalCheckpoint::default(),
            &expected_end,
        )
        .unwrap();
        assert_eq!(report.records_replayed, 2);

        let mut direct = EventValidator::new();
        direct.validate(&quote("e1", 1)).unwrap();
        direct.validate(&quote("e2", 2)).unwrap();
        assert_eq!(
            replayed.encode_snapshot().unwrap(),
            direct.encode_snapshot().unwrap()
        );
    }

    #[test]
    fn replay_preserves_explicit_stream_reset() {
        let first = encode_record(&JournalCheckpoint::default(), &quote("e1", 500)).unwrap();
        let checkpoint_after_first = JournalCheckpoint {
            next_offset: 1,
            last_hash: first[first.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let reset = encode_reset_record(&checkpoint_after_first, "feed-a", "BTCUSD").unwrap();
        let checkpoint_after_reset = JournalCheckpoint {
            next_offset: 2,
            last_hash: reset[reset.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let restarted = encode_record(&checkpoint_after_reset, &quote("e2", 1)).unwrap();
        let expected_end = JournalCheckpoint {
            next_offset: 3,
            last_hash: restarted[restarted.len() - DIGEST_LEN..]
                .try_into()
                .unwrap(),
        };

        let mut journal = first;
        journal.extend_from_slice(&reset);
        journal.extend_from_slice(&restarted);

        let snapshot = EventValidator::new().encode_snapshot().unwrap();
        let (replayed, report) = EventValidator::restore_and_replay(
            &snapshot,
            &journal,
            &JournalCheckpoint::default(),
            &expected_end,
        )
        .unwrap();
        assert_eq!(report.records_replayed, 3);

        let mut direct = EventValidator::new();
        direct.validate(&quote("e1", 500)).unwrap();
        assert!(direct.reset_stream("feed-a", "BTCUSD"));
        direct.validate(&quote("e2", 1)).unwrap();
        assert_eq!(
            replayed.encode_snapshot().unwrap(),
            direct.encode_snapshot().unwrap()
        );
    }

    #[test]
    fn invalid_event_does_not_return_partial_restored_state() {
        let snapshot = EventValidator::new().encode_snapshot().unwrap();
        let valid = encode_record(&JournalCheckpoint::default(), &quote("e1", 1)).unwrap();
        let checkpoint = JournalCheckpoint {
            next_offset: 1,
            last_hash: valid[valid.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let invalid = encode_record(&checkpoint, &quote("e1", 2)).unwrap();
        let expected_end = JournalCheckpoint {
            next_offset: 2,
            last_hash: invalid[invalid.len() - DIGEST_LEN..].try_into().unwrap(),
        };
        let mut journal = valid;
        journal.extend_from_slice(&invalid);

        assert!(matches!(
            EventValidator::restore_and_replay(
                &snapshot,
                &journal,
                &JournalCheckpoint::default(),
                &expected_end
            ),
            Err(JournalError::Validation {
                error: ValidationError::DuplicateEventId,
                ..
            })
        ));
    }
}
