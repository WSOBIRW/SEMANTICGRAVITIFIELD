use crate::event::MarketEvent;
use crate::validation::{EventValidator, ValidationError, ValidationWarning};

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
    SequenceGap,
    ClockSkewSuspected,
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

#[derive(Default)]
pub struct ErrorPreventionEngine {
    validator: EventValidator,
}

impl ErrorPreventionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inspect(&mut self, event: &MarketEvent) -> PreventionReport {
        match self.validator.inspect_with_warnings(event) {
            Ok(warnings) => PreventionReport {
                accepted: true,
                findings: warnings.into_iter().map(map_warning).collect(),
            },
            Err(error) => PreventionReport::rejected(map_error(error)),
        }
    }
}

fn map_warning(warning: ValidationWarning) -> Finding {
    let (code, message) = match warning {
        ValidationWarning::EventClockSkewSuspected {
            event_time_ms,
            received_time_ms,
        } => (
            FindingCode::ClockSkewSuspected,
            format!(
                "Подозрение на рассинхронизацию часов: event_time_ms={event_time_ms}, received_time_ms={received_time_ms}"
            ),
        ),
        ValidationWarning::SequenceGap {
            previous,
            current,
            missing,
        } => (
            FindingCode::SequenceGap,
            format!(
                "Обнаружен разрыв sequence: previous={previous}, current={current}, missing={missing}"
            ),
        ),
    };
    Finding {
        code,
        severity: Severity::Warning,
        message,
    }
}

fn map_error(error: ValidationError) -> Finding {
    let (code, severity) = match error {
        ValidationError::DuplicateEventId => (FindingCode::DuplicateEvent, Severity::Error),
        ValidationError::NonIncreasingSequence { .. } => {
            (FindingCode::SequenceViolation, Severity::Error)
        }
        ValidationError::NegativeEventTime | ValidationError::NegativeReceiveTime => {
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

    fn quote(id: &str, seq: u64) -> MarketEvent {
        MarketEvent::quote(
            id,
            "source",
            "GBPUSD",
            10,
            12,
            Some(seq),
            Quote {
                bid_ticks: 100,
                ask_ticks: 101,
                price_scale: 10,
            },
        )
    }

    #[test]
    fn clean_event_is_accepted() {
        let report = ErrorPreventionEngine::new().inspect(&quote("q1", 1));
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
        let event = quote("q1", 1);
        let mut engine = ErrorPreventionEngine::new();
        assert!(engine.inspect(&event).accepted);
        let report = engine.inspect(&event);
        assert_eq!(report.findings[0].code, FindingCode::DuplicateEvent);
    }

    #[test]
    fn sequence_gap_is_accepted_with_warning() {
        let mut engine = ErrorPreventionEngine::new();
        assert!(engine.inspect(&quote("q1", 1)).accepted);
        let report = engine.inspect(&quote("q2", 4));
        assert!(report.accepted);
        assert_eq!(report.findings[0].code, FindingCode::SequenceGap);
        assert_eq!(report.findings[0].severity, Severity::Warning);
    }

    #[test]
    fn clock_skew_is_accepted_with_warning() {
        let event = MarketEvent::quote(
            "q1",
            "source",
            "GBPUSD",
            20,
            12,
            Some(1),
            Quote {
                bid_ticks: 100,
                ask_ticks: 101,
                price_scale: 10,
            },
        );
        let report = ErrorPreventionEngine::new().inspect(&event);
        assert!(report.accepted);
        assert_eq!(report.findings[0].code, FindingCode::ClockSkewSuspected);
        assert_eq!(report.findings[0].severity, Severity::Warning);
    }
}
