//! Типизированное основание модели наблюдателя SGF.
//!
//! Модуль хранит идентичность наблюдателей, процедуры и происхождение наблюдений.
//! Он намеренно не выводит истинность наблюдения, независимость наблюдателей
//! или причинное влияние измерения на исследуемую систему.

use sha2::{Digest, Sha256};

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

/// Метаданные акта наблюдения и его происхождения.
/// Исходное содержимое хранится отдельно; evidence_refs указывает на него.
/// Структура изменяема в памяти. Неизменяемость сохранённых доказательств
/// должна обеспечиваться отдельным контрактом хранилища.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationRecord {
    pub observation_id: String,
    pub observer_id: String,
    pub observer_version: u32,
    pub procedure_id: String,
    pub procedure_version: u32,
    /// Digest полного определения наблюдателя, а не только его ID/version.
    pub observer_definition_digest: [u8; 32],
    /// Digest полного определения процедуры, а не только её ID/version.
    pub procedure_definition_digest: [u8; 32],
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
    ObserverIdentityMismatch,
    ObserverVersionMismatch,
    ProcedureIdentityMismatch,
    ProcedureVersionMismatch,
    ObserverDefinitionDigestMismatch,
    ProcedureDefinitionDigestMismatch,
}

impl ObserverDescriptor {
    /// Канонический digest полного определения наблюдателя версии 1.
    ///
    /// Digest связывает запись с точным содержимым переданного определения,
    /// но не является подписью и не доказывает происхождение descriptor.
    pub fn definition_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"SGF:OBSERVER_DESCRIPTOR:V1\0");
        update_string(&mut hasher, &self.observer_id);
        update_string(&mut hasher, &self.observer_type);
        hasher.update(self.version.to_le_bytes());
        update_string_list(&mut hasher, &self.capability_codes);
        update_string_list(&mut hasher, &self.limitation_codes);
        update_string_list(&mut hasher, &self.dependency_refs);
        hasher.finalize().into()
    }

    /// Неизвестные коды типов допустимы и не интерпретируются модулем.
    pub fn validate(&self) -> Result<(), ObserverValidationError> {
        require_non_empty(&self.observer_id, ObserverValidationError::EmptyObserverId)?;
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
    /// Канонический digest полного определения процедуры версии 1.
    ///
    /// Digest связывает запись с точным содержимым переданного определения,
    /// но сам по себе не подтверждает подлинность источника или неизменяемость
    /// внешнего хранилища. Для доверенной идентичности нужен доверенный реестр
    /// либо подпись/доверенный digest вне контроля вызывающей стороны.
    pub fn definition_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"SGF:OBSERVATION_PROCEDURE:V1\0");
        update_string(&mut hasher, &self.procedure_id);
        update_string(&mut hasher, &self.procedure_type);
        hasher.update(self.version.to_le_bytes());
        hasher.update([position_code(self.position)]);
        hasher.update([interaction_code(self.interaction)]);
        hasher.update([coordination_code(self.coordination)]);
        update_string(&mut hasher, &self.channel_code);
        update_string(&mut hasher, &self.configuration_ref);
        hasher.finalize().into()
    }

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
    /// Проверяет локальную структуру и точное соответствие версий.
    pub fn validate_against(
        &self,
        observer: &ObserverDescriptor,
        procedure: &ObservationProcedure,
    ) -> Result<(), ObserverValidationError> {
        self.validate()?;
        observer.validate()?;
        procedure.validate()?;

        if self.observer_id != observer.observer_id {
            return Err(ObserverValidationError::ObserverIdentityMismatch);
        }
        if self.observer_version != observer.version {
            return Err(ObserverValidationError::ObserverVersionMismatch);
        }
        if self.observer_definition_digest != observer.definition_digest() {
            return Err(ObserverValidationError::ObserverDefinitionDigestMismatch);
        }
        if self.procedure_id != procedure.procedure_id {
            return Err(ObserverValidationError::ProcedureIdentityMismatch);
        }
        if self.procedure_version != procedure.version {
            return Err(ObserverValidationError::ProcedureVersionMismatch);
        }
        if self.procedure_definition_digest != procedure.definition_digest() {
            return Err(ObserverValidationError::ProcedureDefinitionDigestMismatch);
        }

        Ok(())
    }

    pub fn validate(&self) -> Result<(), ObserverValidationError> {
        require_non_empty(
            &self.observation_id,
            ObserverValidationError::EmptyObservationId,
        )?;
        require_non_empty(&self.observer_id, ObserverValidationError::EmptyObserverId)?;
        require_non_empty(
            &self.procedure_id,
            ObserverValidationError::EmptyProcedureId,
        )?;
        require_non_empty(&self.target_ref, ObserverValidationError::EmptyTargetRef)?;

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


fn update_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn update_string_list(hasher: &mut Sha256, values: &[String]) {
    hasher.update((values.len() as u64).to_le_bytes());
    for value in values {
        update_string(hasher, value);
    }
}

fn position_code(value: ObserverPosition) -> u8 {
    match value {
        ObserverPosition::External => 0,
        ObserverPosition::Internal => 1,
        ObserverPosition::Unknown => 2,
    }
}

fn interaction_code(value: InteractionMode) -> u8 {
    match value {
        InteractionMode::Passive => 0,
        InteractionMode::Active => 1,
        InteractionMode::Unknown => 2,
    }
}

fn coordination_code(value: ObservationCoordination) -> u8 {
    match value {
        ObservationCoordination::Individual => 0,
        ObservationCoordination::Collective => 1,
        ObservationCoordination::Distributed => 2,
        ObservationCoordination::Unknown => 3,
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
