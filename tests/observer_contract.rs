use semantic_gravity_field::{
    InteractionMode, ObservationCoordination, ObservationProcedure, ObservationRecord,
    ObserverDescriptor, ObserverPosition, ObserverValidationError,
};

fn observer() -> ObserverDescriptor {
    ObserverDescriptor {
        observer_id: "observer-a".into(),
        observer_type: "sensor.market-feed.v2".into(),
        version: 2,
        capability_codes: vec!["quote.capture".into()],
        limitation_codes: vec!["no.hidden-liquidity".into()],
        dependency_refs: vec!["provider-a".into()],
    }
}

fn procedure() -> ObservationProcedure {
    ObservationProcedure {
        procedure_id: "procedure-a".into(),
        procedure_type: "stream.capture".into(),
        version: 1,
        position: ObserverPosition::External,
        interaction: InteractionMode::Passive,
        coordination: ObservationCoordination::Individual,
        channel_code: "websocket".into(),
        configuration_ref: "sha256:configuration-a".into(),
    }
}

fn record() -> ObservationRecord {
    ObservationRecord {
        observation_id: "observation-a".into(),
        observer_id: "observer-a".into(),
        observer_version: 2,
        procedure_id: "procedure-a".into(),
        procedure_version: 1,
        target_ref: "market:BTC-USDT".into(),
        started_at_ms: 1_000,
        completed_at_ms: 2_000,
        evidence_refs: vec!["evidence:raw-frame-1".into()],
    }
}

#[test]
fn accepts_explicit_observer_procedure_and_observation() {
    assert_eq!(observer().validate(), Ok(()));
    assert_eq!(procedure().validate(), Ok(()));
    assert_eq!(record().validate(), Ok(()));
}

#[test]
fn accepts_unknown_extensible_observer_and_procedure_codes() {
    let mut descriptor = observer();
    descriptor.observer_type = "future-domain.quantum-like-observer".into();
    let mut method = procedure();
    method.procedure_type = "future-domain.adaptive-probe".into();
    assert_eq!(descriptor.validate(), Ok(()));
    assert_eq!(method.validate(), Ok(()));
}

#[test]
fn rejects_zero_versions() {
    let mut descriptor = observer();
    descriptor.version = 0;
    assert_eq!(descriptor.validate(), Err(ObserverValidationError::ZeroObserverVersion));

    let mut method = procedure();
    method.version = 0;
    assert_eq!(method.validate(), Err(ObserverValidationError::ZeroProcedureVersion));

    let mut observation = record();
    observation.observer_version = 0;
    assert_eq!(
        observation.validate(),
        Err(ObserverValidationError::ZeroObservationObserverVersion)
    );
}

#[test]
fn rejects_empty_identity_and_configuration_fields() {
    let mut descriptor = observer();
    descriptor.observer_id = "  ".into();
    assert_eq!(descriptor.validate(), Err(ObserverValidationError::EmptyObserverId));

    let mut method = procedure();
    method.configuration_ref.clear();
    assert_eq!(method.validate(), Err(ObserverValidationError::EmptyConfigurationRef));

    let mut observation = record();
    observation.target_ref.clear();
    assert_eq!(observation.validate(), Err(ObserverValidationError::EmptyTargetRef));
}

#[test]
fn rejects_invalid_time_interval() {
    let mut observation = record();
    observation.started_at_ms = 2_001;
    assert_eq!(
        observation.validate(),
        Err(ObserverValidationError::CompletionBeforeStart)
    );
}

#[test]
fn rejects_negative_time() {
    let mut observation = record();
    observation.started_at_ms = -1;
    assert_eq!(
        observation.validate(),
        Err(ObserverValidationError::NegativeStartTime)
    );
}

#[test]
fn requires_source_evidence_references() {
    let mut observation = record();
    observation.evidence_refs.clear();
    assert_eq!(
        observation.validate(),
        Err(ObserverValidationError::MissingEvidenceReference)
    );

    observation.evidence_refs = vec!["".into()];
    assert_eq!(
        observation.validate(),
        Err(ObserverValidationError::EmptyEvidenceReference)
    );
}

#[test]
fn preserves_declared_dependencies_without_claiming_independence() {
    let descriptor = observer();
    assert_eq!(descriptor.dependency_refs, vec!["provider-a"]);
    assert_eq!(descriptor.validate(), Ok(()));

    let mut no_dependencies = observer();
    no_dependencies.dependency_refs.clear();
    assert_eq!(no_dependencies.validate(), Ok(()));
}

#[test]
fn unknown_interaction_and_observer_position_are_explicit_values() {
    let mut method = procedure();
    method.position = ObserverPosition::Unknown;
    method.interaction = InteractionMode::Unknown;
    method.coordination = ObservationCoordination::Unknown;
    assert_eq!(method.validate(), Ok(()));
}
