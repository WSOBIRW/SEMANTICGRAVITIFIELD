//! Минимальное детерминированное ядро SGF.
//!
//! Ядро проверяет входные события и сообщает о проблемах.
//! Оно не формирует торговые сигналы и не исполняет ордера.

pub mod event;
pub mod journal;
pub mod observer;
pub mod prevention;
pub mod recovery;
pub mod snapshot;
pub mod validation;

pub use event::{EventPayload, MarketEvent, Quote};
pub use journal::{
    encode_record, encode_reset_record, JournalCheckpoint, JournalError, ReplayReport,
};
pub use observer::{
    InteractionMode, ObservationCoordination, ObservationProcedure, ObservationRecord,
    ObserverDescriptor, ObserverPosition, ObserverValidationError,
};
pub use prevention::{ErrorPreventionEngine, Finding, FindingCode, PreventionReport, Severity};
pub use recovery::{
    validate_generation_transition, RecoveryError, RecoveryGenerationError, RecoveryManifest,
    RecoveryPackage, RecoveryStorage, RecoveryStorageError,
};
pub use snapshot::SnapshotError;
pub use validation::{EventValidator, ValidationError, ValidationWarning};
