use crate::event::MarketEvent;
use crate::validation::{EventValidator, ValidationError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindingCode {
    InvalidInput,
    DuplicateEvent,
    SequenceViolation,
    InvalidMarketValue,
    TimeIntegrityViolation,
    UnsupportedSchema,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreventionReport {
    pub accepted: bool,
    pub findings: Vec<Finding>,
}

impl PreventionReport {
    pub fn clean() -> Self {
        Self {
            accepted: true,
            findings: Vec::new(),
        }
    }

    pub fn rejected(finding: Finding) -> Self {
        Self {
            accepted: false,
            findings: vec![finding],
        }
    }
}

/// Первый защитный контур: проверка входных событий без торговых полномочий.
#[derive(Default)]
pub struct ErrorPreventionEngine {
    validator: EventValidator,
}

impl ErrorPreventionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inspect(&mut self, event: &MarketEvent) -> PreventionReport {
        match self.validator.validate(event) {
            Ok(()) => PreventionReport::clean(),
            Err(error) => PreventionReport::rejected(map_error(error)),
        }
    }
}

fn map_error(error: ValidationError) -> Finding {
    let (code, severity) = match error {
        ValidationError::DuplicateEventId => (FindingCode::DuplicateEvent, Severity::Error),
        ValidationError::NonIncreasingSequence { .. } => {
            (FindingCode::SequenceViolation, Severity::Error)
        }
        ValidationError::EventTimeAfterReceiveTime
        | ValidationError::NegativeEventTime
        | ValidationError::NegativeReceiveTime => {
            (FindingCode::TimeIntegrityViolation, Severity::Critical)
        }
        ValidationError::InvalidQuote { .. }
        | ValidationError::InvalidPriceScale
        | ValidationError::InvalidTradePrice(_)
        | ValidationError::InvalidTradeQuantity => {
            (FindingCode::InvalidMarketValue, Severity::Critical)
        }
        ValidationError::UnsupportedSchema(_) => {
            (FindingCode::UnsupportedSchema, Severity::Critical)
        }
        _ => (FindingCode::InvalidInput, Severity::Error),
    };

    Finding {
        code,
        severity,
        message: format!("{error:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Quote;

    fn quote(id: &str) -> MarketEvent {
        MarketEvent::quote(
            id,
            "source",
            "GBPUSD",
            10,
            12,
            Some(1),
            Quote {
                bid_ticks: 100,
                ask_ticks: 101,
                price_scale: 10,
            },
        )
    }

    #[test]
    fn clean_event_is_accepted() {
        let event = quote("q1");
        let report = ErrorPreventionEngine::new().inspect(&event);
        assert!(report.accepted);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn invalid_event_is_rejected_with_critical_finding() {
        let event = MarketEvent::quote(
            "q1",
            "source",
            "GBPUSD",
            10,
            12,
            Some(1),
            Quote {
                bid_ticks: 102,
                ask_ticks: 101,
                price_scale: 10,
            },
        );
        let report = ErrorPreventionEngine::new().inspect(&event);
        assert!(!report.accepted);
        assert_eq!(report.findings[0].code, FindingCode::InvalidMarketValue);
        assert_eq!(report.findings[0].severity, Severity::Critical);
    }

    #[test]
    fn duplicate_is_reported() {
        let event = quote("q1");
        let mut engine = ErrorPreventionEngine::new();
        assert!(engine.inspect(&event).accepted);
        let report = engine.inspect(&event);
        assert_eq!(report.findings[0].code, FindingCode::DuplicateEvent);
    }
}
