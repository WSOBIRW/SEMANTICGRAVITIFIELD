//! Минимальное детерминированное ядро SGF.
//!
//! Ядро проверяет входные события и сообщает о проблемах.
//! Оно не формирует торговые сигналы и не исполняет ордера.

pub mod event;
pub mod prevention;
pub mod validation;

pub use event::{EventPayload, MarketEvent, Quote};
pub use prevention::{ErrorPreventionEngine, Finding, FindingCode, PreventionReport, Severity};
pub use validation::{EventValidator, ValidationError, ValidationWarning};
