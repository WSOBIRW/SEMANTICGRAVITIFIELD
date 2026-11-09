use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;

use crate::event::{EventPayload, MarketEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    UnsupportedSchema(u16),
    EmptyEventId,
    EmptySourceId,
    EmptyInstrumentId,
    NegativeEventTime,
    NegativeReceiveTime,
    DuplicateEventId,
    NonIncreasingSequence { previous: u64, current: u64 },
    InvalidQuote { bid_ticks: i64, ask_ticks: i64 },
    InvalidPriceScale,
    InvalidTradePrice(i64),
    InvalidTradeQuantity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationWarning {
    EventClockSkewSuspected {
        event_time_ms: i64,
        received_time_ms: i64,
    },
    SequenceGap {
        previous: u64,
        current: u64,
        missing: u64,
    },
}

#[derive(Default)]
pub struct EventValidator {
    // Контракт MVP: event_id глобально уникален в пределах экземпляра валидатора.
    seen_event_ids: HashSet<String>,
    event_id_order: VecDeque<String>,
    dedup_capacity: Option<NonZeroUsize>,
    last_sequences: HashMap<(String, String), u64>,
}

impl EventValidator {
    /// Создаёт валидатор с неограниченной дедупликацией.
    /// Для длительно работающего процесса следует задать явную ёмкость.
    pub fn new() -> Self {
        Self::default()
    }

    /// Создаёт валидатор с ограниченным окном последних N event_id.
    /// Идентификаторы вытесняются по FIFO; повтор вне окна может быть принят.
    pub fn with_dedup_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            dedup_capacity: Some(capacity),
            ..Self::default()
        }
    }

    fn remember_event_id(&mut self, event_id: &str) {
        self.seen_event_ids.insert(event_id.to_owned());
        if let Some(capacity) = self.dedup_capacity {
            self.event_id_order.push_back(event_id.to_owned());
            while self.event_id_order.len() > capacity.get() {
                if let Some(expired) = self.event_id_order.pop_front() {
                    self.seen_event_ids.remove(&expired);
                }
            }
        }
    }

    /// Совместимый строгий интерфейс: возвращает ошибку либо принимает событие.
    /// Предупреждения доступны через inspect_with_warnings.
    pub fn validate(&mut self, event: &MarketEvent) -> Result<(), ValidationError> {
        self.inspect_with_warnings(event).map(|_| ())
    }

    /// Проверяет событие. Наблюдаемый разрыв sequence и подозрение на рассинхронизацию
    /// часов возвращаются как предупреждения, а не как доказанная порча события.
    /// Невалидное событие не изменяет состояние дедупликации или последовательности.
    pub fn inspect_with_warnings(
        &mut self,
        event: &MarketEvent,
    ) -> Result<Vec<ValidationWarning>, ValidationError> {
        validate_static(event)?;

        if self.seen_event_ids.contains(&event.event_id) {
            return Err(ValidationError::DuplicateEventId);
        }

        let stream_key = (event.source_id.clone(), event.instrument_id.clone());
        let mut warnings = Vec::new();

        if event.event_time_ms > event.received_time_ms {
            warnings.push(ValidationWarning::EventClockSkewSuspected {
                event_time_ms: event.event_time_ms,
                received_time_ms: event.received_time_ms,
            });
        }

        if let Some(current) = event.sequence {
            if let Some(previous) = self.last_sequences.get(&stream_key).copied() {
                if current <= previous {
                    return Err(ValidationError::NonIncreasingSequence { previous, current });
                }
                if current - previous > 1 {
                    warnings.push(ValidationWarning::SequenceGap {
                        previous,
                        current,
                        missing: current - previous - 1,
                    });
                }
            }
        }

        self.remember_event_id(&event.event_id);
        if let Some(current) = event.sequence {
            self.last_sequences.insert(stream_key, current);
        }
        Ok(warnings)
    }
}

fn validate_static(event: &MarketEvent) -> Result<(), ValidationError> {
    if event.schema_version != 1 {
        return Err(ValidationError::UnsupportedSchema(event.schema_version));
    }
    if event.event_id.trim().is_empty() {
        return Err(ValidationError::EmptyEventId);
    }
    if event.source_id.trim().is_empty() {
        return Err(ValidationError::EmptySourceId);
    }
    if event.instrument_id.trim().is_empty() {
        return Err(ValidationError::EmptyInstrumentId);
    }
    if event.event_time_ms < 0 {
        return Err(ValidationError::NegativeEventTime);
    }
    if event.received_time_ms < 0 {
        return Err(ValidationError::NegativeReceiveTime);
    }

    match &event.payload {
        EventPayload::Quote {
            bid_ticks,
            ask_ticks,
            price_scale,
        } => {
            if *price_scale == 0 {
                return Err(ValidationError::InvalidPriceScale);
            }
            if *bid_ticks <= 0 || *ask_ticks <= 0 || bid_ticks > ask_ticks {
                return Err(ValidationError::InvalidQuote {
                    bid_ticks: *bid_ticks,
                    ask_ticks: *ask_ticks,
                });
            }
        }
        EventPayload::Trade {
            price_ticks,
            quantity_lots,
            price_scale,
        } => {
            if *price_scale == 0 {
                return Err(ValidationError::InvalidPriceScale);
            }
            if *price_ticks <= 0 {
                return Err(ValidationError::InvalidTradePrice(*price_ticks));
            }
            if *quantity_lots == 0 {
                return Err(ValidationError::InvalidTradeQuantity);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Quote;
    use std::num::NonZeroUsize;

    fn quote(
        id: &str,
        source: &str,
        instrument: &str,
        seq: u64,
        event_time: i64,
        received: i64,
    ) -> MarketEvent {
        MarketEvent::quote(
            id,
            source,
            instrument,
            event_time,
            received,
            Some(seq),
            Quote {
                bid_ticks: 10_000,
                ask_ticks: 10_002,
                price_scale: 100_000,
            },
        )
    }

    #[test]
    fn accepts_valid_quote() {
        assert!(EventValidator::new()
            .validate(&quote("e1", "feed-a", "EURUSD", 1, 100, 110))
            .is_ok());
    }

    #[test]
    fn rejects_duplicate_event_id_globally() {
        let mut validator = EventValidator::new();
        assert!(validator
            .validate(&quote("e1", "feed-a", "EURUSD", 1, 100, 110))
            .is_ok());
        assert_eq!(
            validator.validate(&quote("e1", "feed-b", "BTCUSD", 1, 100, 110)),
            Err(ValidationError::DuplicateEventId)
        );
    }

    #[test]
    fn rejects_non_increasing_sequence() {
        let mut validator = EventValidator::new();
        assert!(validator
            .validate(&quote("e1", "feed-a", "EURUSD", 8, 100, 110))
            .is_ok());
        assert_eq!(
            validator.validate(&quote("e2", "feed-a", "EURUSD", 8, 100, 110)),
            Err(ValidationError::NonIncreasingSequence {
                previous: 8,
                current: 8
            })
        );
    }

    #[test]
    fn sequence_gap_is_reported_and_stream_advances() {
        let mut validator = EventValidator::new();
        validator
            .validate(&quote("e1", "feed-a", "EURUSD", 10, 100, 110))
            .unwrap();
        let warnings = validator
            .inspect_with_warnings(&quote("e2", "feed-a", "EURUSD", 14, 100, 110))
            .unwrap();
        assert_eq!(
            warnings,
            vec![ValidationWarning::SequenceGap {
                previous: 10,
                current: 14,
                missing: 3
            }]
        );
        assert!(validator
            .validate(&quote("e3", "feed-a", "EURUSD", 15, 100, 110))
            .is_ok());
    }

    #[test]
    fn sequence_state_is_independent_per_source_and_instrument() {
        let mut validator = EventValidator::new();
        assert!(validator
            .validate(&quote("e1", "feed-a", "EURUSD", 100, 100, 110))
            .is_ok());
        assert!(validator
            .validate(&quote("e2", "feed-b", "EURUSD", 1, 100, 110))
            .is_ok());
        assert!(validator
            .validate(&quote("e3", "feed-a", "BTCUSD", 1, 100, 110))
            .is_ok());
    }

    #[test]
    fn future_event_time_is_a_clock_skew_warning_not_proof_of_corruption() {
        let mut validator = EventValidator::new();
        let warnings = validator
            .inspect_with_warnings(&quote("e1", "feed-a", "EURUSD", 1, 120, 110))
            .unwrap();
        assert_eq!(
            warnings,
            vec![ValidationWarning::EventClockSkewSuspected {
                event_time_ms: 120,
                received_time_ms: 110
            }]
        );
        assert!(validator
            .validate(&quote("e2", "feed-a", "EURUSD", 2, 121, 120))
            .is_ok());
    }

    #[test]
    fn rejects_crossed_quote() {
        let mut event = quote("e1", "feed-a", "EURUSD", 1, 100, 110);
        event.payload = EventPayload::Quote {
            bid_ticks: 10_003,
            ask_ticks: 10_002,
            price_scale: 100_000,
        };
        assert!(matches!(
            EventValidator::new().validate(&event),
            Err(ValidationError::InvalidQuote { .. })
        ));
    }


    #[test]
    fn bounded_deduplication_rejects_repeats_inside_window() {
        let mut validator = EventValidator::with_dedup_capacity(NonZeroUsize::new(2).unwrap());
        let make = |id: &str| {
            MarketEvent::quote(
                id,
                "feed-a",
                "EURUSD",
                100,
                110,
                None,
                Quote {
                    bid_ticks: 10_000,
                    ask_ticks: 10_002,
                    price_scale: 100_000,
                },
            )
        };
        assert!(validator.validate(&make("e1")).is_ok());
        assert_eq!(validator.validate(&make("e1")), Err(ValidationError::DuplicateEventId));
    }

    #[test]
    fn bounded_deduplication_evicts_oldest_identifier() {
        let mut validator = EventValidator::with_dedup_capacity(NonZeroUsize::new(2).unwrap());
        let make = |id: &str| {
            MarketEvent::quote(
                id,
                "feed-a",
                "EURUSD",
                100,
                110,
                None,
                Quote {
                    bid_ticks: 10_000,
                    ask_ticks: 10_002,
                    price_scale: 100_000,
                },
            )
        };
        assert!(validator.validate(&make("e1")).is_ok());
        assert!(validator.validate(&make("e2")).is_ok());
        assert!(validator.validate(&make("e3")).is_ok());
        assert!(validator.validate(&make("e1")).is_ok());
    }

    #[test]
    fn failed_validation_does_not_consume_event_id_or_sequence() {
        let mut validator = EventValidator::new();
        let mut invalid = quote("e1", "feed-a", "EURUSD", 1, 100, 110);
        invalid.payload = EventPayload::Quote {
            bid_ticks: 0,
            ask_ticks: 1,
            price_scale: 100_000,
        };
        assert!(validator.validate(&invalid).is_err());
        assert!(validator
            .validate(&quote("e1", "feed-a", "EURUSD", 1, 100, 110))
            .is_ok());
    }
}
