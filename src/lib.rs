//! Минимальное детерминированное ядро SGF.
//!
//! Ядро проверяет рыночные события, формирует исследовательские кандидаты
//! и предварительные экономические оценки. Оно не выдаёт исполняемые
//! торговые сигналы и не размещает ордера.

pub mod economic;
pub mod event;
pub mod journal;
pub mod mexc;
pub mod mexc_capture;
pub mod prevention;
pub mod recovery;
pub mod research;
pub mod snapshot;
pub mod validation;

pub use economic::{
    evaluate_round_trip, EconomicAssessment, EconomicConfig, EconomicError, EconomicVerdict,
};
pub use event::{EventPayload, MarketEvent, Quote};
pub use journal::{
    encode_record, encode_reset_record, JournalCheckpoint, JournalError, ReplayReport,
};
pub use mexc::{
    normalize_deal, normalize_ticker, MexcContractSpec, MexcDealFrame, MexcError, MexcObservation,
    MexcObservationMetadata, MexcPositionAction, MexcSelfTradeFlag, MexcTickerFrame, MexcTradeSide,
};
pub use mexc_capture::{MexcCaptureError, MexcCaptureLimits, MexcCaptureReader};
pub use prevention::{ErrorPreventionEngine, Finding, FindingCode, PreventionReport, Severity};
pub use recovery::{
    validate_generation_transition, RecoveryError, RecoveryGenerationError, RecoveryManifest,
    RecoveryPackage, RecoveryStorage, RecoveryStorageError,
};
pub use research::{
    analyze_market_window, MarketWindowMetrics, OpportunityCandidate, PriceDirection,
    ResearchConfig, ResearchError, ResearchReport,
};
pub use snapshot::SnapshotError;
pub use validation::{EventValidator, ValidationError, ValidationWarning};
