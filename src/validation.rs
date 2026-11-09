use std::collections::{HashMap, HashSet};

use crate::event::{EventPayload, MarketEvent};
#[cfg(test)]
use crate::event::Quote;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    UnsupportedSchema(u16),
    EmptyEventId,
    EmptySourceId,
    EmptyInstrumentId,
    NegativeEventTime,
    NegativeReceiveTime,
    EventTimeAfterReceiveTime,
    DuplicateEventId,
    NonIncreasingSequence { previous: u64, current: u64 },
    InvalidQuote { bid_ticks: i64, ask_ticks: i64 },
    InvalidPriceScale,
    InvalidTradePrice(i64),
    InvalidTradeQuantity,
}

#[derive(Default)]
pub struct EventValidator {
    seen_event_ids: HashSet<String>,
    last_sequences: HashMap<(String, String), u64>,
}

impl EventValidator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Проверяет событие и обновляет состояние только при успешной проверке.
    /// Порядок sequence контролируется отдельно для каждой пары source/instrument.
    pub fn validate(&mut self, event: &MarketEvent) -> Result<(), ValidationError> {
        validate_static(event)?;

        if self.seen_event_ids.contains(&event.event_id) {
            return Err(ValidationError::DuplicateEventId);
        }

        let stream_key = (event.source_id.clone(), event.instrument_id.clone());
        if let Some(current) = event.sequence {
            if let Some(previous) = self.last_sequences.get(&stream_key) {
                if current <= *previous {
                    return Err(ValidationError::NonIncreasingSequence {
                        previous: *previous,
                        current,
                    });
                }
            }
        }

        self.seen_event_ids.insert(event.event_id.clone());
        if let Some(current) = event.sequence {
            self.last_sequences.insert(stream_key, current);
        }
        Ok(())
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
    if event.event_time_ms > event.received_time_ms {
        return Err(ValidationError::EventTimeAfterReceiveTime);
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

    fn quote(id: &str, seq: u64) -> MarketEvent {
        MarketEvent::quote(
            id,
            "feed-a",
            "EURUSD",
            100,
            110,
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
        assert!(EventValidator::new().validate(&quote("e1", 1)).is_ok());
    }

    #[test]
    fn rejects_duplicate_event_id() {
        let mut validator = EventValidator::new();
        let event = quote("e1", 1);
        assert!(validator.validate(&event).is_ok());
        assert_eq!(
            validator.validate(&event),
            Err(ValidationError::DuplicateEventId)
        );
    }

    #[test]
    fn rejects_non_increasing_sequence() {
        let mut validator = EventValidator::new();
        assert!(validator.validate(&quote("e1", 8)).is_ok());
        assert_eq!(
            validator.validate(&quote("e2", 8)),
            Err(ValidationError::NonIncreasingSequence {
                previous: 8,
                current: 8
            })
        );
    }

    #[test]
    fn rejects_future_event_time() {
        let mut event = quote("e1", 1);
        event.event_time_ms = 111;
        assert_eq!(
            EventValidator::new().validate(&event),
            Err(ValidationError::EventTimeAfterReceiveTime)
        );
    }

    #[test]
    fn rejects_crossed_quote() {
        let mut event = quote("e1", 1);
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
    fn failed_validation_does_not_consume_event_id_or_sequence() {
        let mut validator = EventValidator::new();
        let mut invalid = quote("e1", 1);
        invalid.event_time_ms = 999;
        assert!(validator.validate(&invalid).is_err());
        assert!(validator.validate(&quote("e1", 1)).is_ok());
    }
}
