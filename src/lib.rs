//! Минимальное детерминированное ядро SGF.
//!
//! Ядро проверяет входные события и сообщает о проблемах.
//! Оно не формирует торговые сигналы и не исполняет ордера.

pub mod event;
pub mod journal;
pub mod prevention;
pub mod research;
pub mod recovery;
pub mod snapshot;
pub mod validation;

pub use event::{EventPayload, MarketEvent, Quote};
pub use journal::{
    encode_record, encode_reset_record, JournalCheckpoint, JournalError, ReplayReport,
};
pub use prevention::{ErrorPreventionEngine, Finding, FindingCode, PreventionReport, Severity};
pub use research::{
    analyze_market_window, MarketWindowMetrics, OpportunityCandidate, PriceDirection,
    ResearchConfig, ResearchError, ResearchReport,
};
pub use recovery::{
    validate_generation_transition, RecoveryError, RecoveryGenerationError, RecoveryManifest,
    RecoveryPackage, RecoveryStorage, RecoveryStorageError,
};
pub use snapshot::SnapshotError;
pub use validation::{EventValidator, ValidationError, ValidationWarning};
