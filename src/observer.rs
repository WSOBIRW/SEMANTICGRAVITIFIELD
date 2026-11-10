//! Типизированное основание модели наблюдателя SGF.
//!
//! Модуль хранит идентичность наблюдателей, процедуры и происхождение наблюдений.
//! Он намеренно не выводит истинность наблюдения, независимость наблюдателей
//! или причинное влияние измерения на исследуемую систему.

/// Описание наблюдателя и его явно объявленных ограничений.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObserverDescriptor {
    pub observer_id: String,
    /// Расширяемый код типа, например "sensor.market-feed.v2".
    pub observer_type: String,
    pub version: u32,
    /// Возможности не являются доказательством фактической полноты.
    pub capability_codes: Vec<String>,
    pub limitation_codes: Vec<String>,
    /// Пустой список не означает доказанную независимость.
    pub dependency_refs: Vec<String>,
}

/// Отношение наблюдателя к исследуемой системе.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObserverPosition {
    External,
    Internal,
    Unknown,
}

/// Характер взаимодействия процедуры с исследуемой системой.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionMode {
    Passive,
    Active,
    Unknown,
}

/// Организация выполнения процедуры наблюдения.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationCoordination {
    Individual,
    Collective,
    Distributed,
    Unknown,
}

/// Версионированное описание процедуры наблюдения.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationProcedure {
    pub procedure_id: String,
    /// Расширяемый код процедуры.
    pub procedure_type: String,
    pub version: u32,
    pub position: ObserverPosition,
    pub interaction: InteractionMode,
    pub coordination: ObservationCoordination,
    pub channel_code: String,
    /// Ссылка на неизменяемую конфигурацию либо её описание.
    pub configuration_ref: String,
}

/// Неизменяемая запись акта наблюдения и его происхождения.
/// Исходное содержимое хранится отдельно, evidence_refs указывает на него.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationRecord {
    pub observation_id: String,
    pub observer_id: String,
    pub observer_version: u32,
    pub procedure_id: String,
    pub procedure_version: u32,
    pub target_ref: String,
    /// Время начала в Unix milliseconds UTC.
    pub started_at_ms: i64,
    /// Время завершения в Unix milliseconds UTC.
    pub completed_at_ms: i64,
    pub evidence_refs: Vec<String>,
}

/// Ошибки структурной проверки модели наблюдателя.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObserverValidationError {
    EmptyObserverId,
    EmptyObserverType,
    ZeroObserverVersion,
    EmptyCapabilityCode,
    EmptyLimitationCode,
    EmptyDependencyRef,
    EmptyProcedureId,
    EmptyProcedureType,
    ZeroProcedureVersion,
    EmptyChannelCode,
    EmptyConfigurationRef,
    EmptyObservationId,
    EmptyTargetRef,
    NegativeStartTime,
    NegativeCompletionTime,
    CompletionBeforeStart,
    ZeroObservationObserverVersion,
    ZeroObservationProcedureVersion,
    MissingEvidenceReference,
    EmptyEvidenceReference,
}

impl ObserverDescriptor {
    /// Неизвестные коды типов допустимы и не интерпретируются модулем.
    pub fn validate(&self) -> Result<(), ObserverValidationError> {
        require_non_empty(
            &self.observer_id,
            ObserverValidationError::EmptyObserverId,
        )?;
        require_non_empty(
            &self.observer_type,
            ObserverValidationError::EmptyObserverType,
        )?;

        if self.version == 0 {
            return Err(ObserverValidationError::ZeroObserverVersion);
        }

        validate_codes(
            &self.capability_codes,
            ObserverValidationError::EmptyCapabilityCode,
        )?;
        validate_codes(
            &self.limitation_codes,
            ObserverValidationError::EmptyLimitationCode,
        )?;
        validate_codes(
            &self.dependency_refs,
            ObserverValidationError::EmptyDependencyRef,
        )?;

        Ok(())
    }
}

impl ObservationProcedure {
    pub fn validate(&self) -> Result<(), ObserverValidationError> {
        require_non_empty(
            &self.procedure_id,
            ObserverValidationError::EmptyProcedureId,
        )?;
        require_non_empty(
            &self.procedure_type,
            ObserverValidationError::EmptyProcedureType,
        )?;

        if self.version == 0 {
            return Err(ObserverValidationError::ZeroProcedureVersion);
        }

        require_non_empty(
            &self.channel_code,
            ObserverValidationError::EmptyChannelCode,
        )?;
        require_non_empty(
            &self.configuration_ref,
            ObserverValidationError::EmptyConfigurationRef,
        )?;

        Ok(())
    }
}

impl ObservationRecord {
    pub fn validate(&self) -> Result<(), ObserverValidationError> {
        require_non_empty(
            &self.observation_id,
            ObserverValidationError::EmptyObservationId,
        )?;
        require_non_empty(
            &self.observer_id,
            ObserverValidationError::EmptyObserverId,
        )?;
        require_non_empty(
            &self.procedure_id,
            ObserverValidationError::EmptyProcedureId,
        )?;
        require_non_empty(
            &self.target_ref,
            ObserverValidationError::EmptyTargetRef,
        )?;

        if self.observer_version == 0 {
            return Err(ObserverValidationError::ZeroObservationObserverVersion);
        }
        if self.procedure_version == 0 {
            return Err(ObserverValidationError::ZeroObservationProcedureVersion);
        }
        if self.started_at_ms < 0 {
            return Err(ObserverValidationError::NegativeStartTime);
        }
        if self.completed_at_ms < 0 {
            return Err(ObserverValidationError::NegativeCompletionTime);
        }
        if self.completed_at_ms < self.started_at_ms {
            return Err(ObserverValidationError::CompletionBeforeStart);
        }
        if self.evidence_refs.is_empty() {
            return Err(ObserverValidationError::MissingEvidenceReference);
        }

        validate_codes(
            &self.evidence_refs,
            ObserverValidationError::EmptyEvidenceReference,
        )?;

        Ok(())
    }
}

fn require_non_empty(
    value: &str,
    error: ObserverValidationError,
) -> Result<(), ObserverValidationError> {
    if value.trim().is_empty() {
        Err(error)
    } else {
        Ok(())
    }
}

fn validate_codes(
    values: &[String],
    error: ObserverValidationError,
) -> Result<(), ObserverValidationError> {
    if values.iter().any(|value| value.trim().is_empty()) {
        Err(error)
    } else {
        Ok(())
    }
}
