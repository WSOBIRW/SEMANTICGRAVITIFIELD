use semantic_gravity_field::experiment::{
    apply_intervention, observe, ExperimentError, StateVariable, SyntheticIntervention,
    SyntheticObserver, SyntheticState,
};

fn state() -> SyntheticState {
    SyntheticState {
        state_id: "synthetic-1".into(),
        step: 4,
        variables: vec![
            StateVariable { key: "alpha".into(), value: 123 },
            StateVariable { key: "beta".into(), value: -47 },
        ],
    }
}

fn observer() -> SyntheticObserver {
    SyntheticObserver {
        observer_id: "observer-a".into(),
        version: 1,
        visible_keys: vec!["alpha".into(), "beta".into()],
        resolution: 1,
        noise_bound: 0,
        noise_seed: 42,
    }
}

#[test]
fn observation_does_not_mutate_hidden_state() {
    let original = state();
    let observed = observe(&original, &observer()).unwrap();
    assert_eq!(original.step, 4);
    assert_eq!(original.variables[0].value, 123);
    assert_eq!(observed.visible_variables[0].value, 123);
}

#[test]
fn observer_sees_only_explicitly_available_variables() {
    let mut limited = observer();
    limited.visible_keys = vec!["beta".into()];
    let view = observe(&state(), &limited).unwrap();
    assert_eq!(view.visible_variables.len(), 1);
    assert_eq!(view.visible_variables[0].key, "beta");
    assert_eq!(view.visible_variables[0].value, -47);
}

#[test]
fn quantization_is_explicit_and_deterministic() {
    let mut coarse = observer();
    coarse.resolution = 10;
    let view = observe(&state(), &coarse).unwrap();
    assert_eq!(view.visible_variables[0].value, 120);
    assert_eq!(view.visible_variables[1].value, -50);
    assert_eq!(view, observe(&state(), &coarse).unwrap());
}

#[test]
fn noise_is_repeatable_for_same_seed_and_inputs() {
    let mut noisy = observer();
    noisy.noise_bound = 5;
    noisy.noise_seed = 991;
    let first = observe(&state(), &noisy).unwrap();
    let second = observe(&state(), &noisy).unwrap();
    assert_eq!(first, second);
    for (actual, expected) in first.visible_variables.iter().zip([123_i64, -47]) {
        assert!((actual.value - expected).abs() <= 5);
    }
}

#[test]
fn different_noise_seeds_can_produce_different_views() {
    let mut first_observer = observer();
    first_observer.noise_bound = 100;
    first_observer.noise_seed = 1;
    let mut second_observer = first_observer.clone();
    second_observer.noise_seed = 2;
    assert_ne!(
        observe(&state(), &first_observer).unwrap(),
        observe(&state(), &second_observer).unwrap()
    );
}

#[test]
fn intervention_changes_only_the_returned_state() {
    let original = state();
    let changed = apply_intervention(
        &original,
        &SyntheticIntervention { variable_key: "alpha".into(), delta: 7 },
    ).unwrap();
    assert_eq!(original.variables[0].value, 123);
    assert_eq!(original.step, 4);
    assert_eq!(changed.variables[0].value, 130);
    assert_eq!(changed.step, 5);
}

#[test]
fn rejects_invalid_and_ambiguous_states() {
    let mut duplicate = state();
    duplicate.variables.push(duplicate.variables[0].clone());
    assert_eq!(duplicate.validate(), Err(ExperimentError::DuplicateVariableKey));
    let mut invalid_observer = observer();
    invalid_observer.resolution = 0;
    assert_eq!(observe(&state(), &invalid_observer), Err(ExperimentError::ZeroResolution));
}

#[test]
fn rejects_duplicate_or_unknown_visible_keys() {
    let mut duplicate = observer();
    duplicate.visible_keys = vec!["alpha".into(), "alpha".into()];
    assert_eq!(observe(&state(), &duplicate), Err(ExperimentError::DuplicateVisibleKey));
    let mut unknown = observer();
    unknown.visible_keys = vec!["not-in-state".into()];
    assert_eq!(observe(&state(), &unknown), Err(ExperimentError::UnknownVisibleKey));
}

#[test]
fn deterministic_noise_is_scoped_to_the_sample_step() {
    let mut noisy = observer();
    noisy.noise_bound = 50;
    let first = observe(&state(), &noisy).unwrap();
    let mut next_state = state();
    next_state.step += 1;
    let second = observe(&next_state, &noisy).unwrap();
    assert_ne!(first.visible_variables[0].value, second.visible_variables[0].value);
}

#[test]
fn rejects_missing_intervention_target_and_overflow() {
    let missing = SyntheticIntervention { variable_key: "absent".into(), delta: 1 };
    assert_eq!(apply_intervention(&state(), &missing), Err(ExperimentError::MissingInterventionTarget));
    let mut maximum = state();
    maximum.variables[0].value = i64::MAX;
    let overflow = SyntheticIntervention { variable_key: "alpha".into(), delta: 1 };
    assert_eq!(apply_intervention(&maximum, &overflow), Err(ExperimentError::ArithmeticOverflow));
}

#[test]
fn rejects_noise_bounds_that_exceed_the_symmetric_hash_domain() {
    let mut oversized = observer();
    oversized.noise_bound = u64::MAX;
    assert_eq!(oversized.validate(), Err(ExperimentError::NoiseBoundOutOfRange));
    assert_eq!(observe(&state(), &oversized), Err(ExperimentError::NoiseBoundOutOfRange));
}

#[test]
fn accepts_the_largest_supported_noise_bound_within_i64_domain() {
    let mut largest = observer();
    largest.noise_bound = i64::MAX as u64;
    let mut zero_state = state();
    zero_state.variables[0].value = 0;
    assert!(largest.validate().is_ok());
    // Проверяем только представимость диапазона, не статистическую равномерность.
    let value = observe(&zero_state, &largest).unwrap().visible_variables[0].value;
    let delta = i128::from(value);
    assert!(delta >= -i128::from(i64::MAX));
    assert!(delta <= i128::from(i64::MAX));
}

#[test]
fn observation_preserves_state_order_not_access_list_order() {
    let mut reversed = observer();
    reversed.visible_keys = vec!["beta".into(), "alpha".into()];

    let view = observe(&state(), &reversed).unwrap();

    assert_eq!(
        view.visible_variables
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
}


#[test]
fn projection_filter_is_not_an_authorization_boundary() {
    // SyntheticObserver is caller-controlled. The projection selects output
    // fields but does not authenticate or authorize the caller.
    let mut unrestricted_projection = observer();
    unrestricted_projection.visible_keys = vec!["alpha".into(), "beta".into()];

    let view = observe(&state(), &unrestricted_projection).unwrap();

    assert_eq!(view.visible_variables.len(), 2);
    assert_eq!(
        view.visible_variables
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
}
