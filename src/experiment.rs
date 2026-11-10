//! Минимальное синтетическое ядро экспериментов с наблюдателями.
//!
//! Скрытое состояние, доступ наблюдателя и вмешательство разделены.
//! Вызов observe не изменяет состояние. Изменение возможно только через
//! явный apply_intervention. Типы здесь описывают синтетическую модель,
//! а не онтологию реального смыслового поля.

/// Одно измеряемое свойство синтетической системы.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateVariable {
    pub key: String,
    pub value: i64,
}

/// Полное скрытое состояние синтетической системы.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticState {
    pub state_id: String,
    pub step: u64,
    pub variables: Vec<StateVariable>,
}

/// Ограничения доступа и измерения конкретного наблюдателя.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticObserver {
    pub observer_id: String,
    pub version: u32,
    /// Пустой список означает отсутствие доступа ко всем переменным.
    pub visible_keys: Vec<String>,
    /// Шаг квантования; 1 означает отсутствие квантования.
    pub resolution: u64,
    /// Максимальная абсолютная добавляемая ошибка измерения.
    pub noise_bound: u64,
    /// Seed локального детерминированного шума.
    pub noise_seed: u64,
}

/// Одно значение в представлении наблюдателя.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedVariable {
    pub key: String,
    pub value: i64,
}

/// Производное представление. Не является исходным состоянием.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationView {
    pub state_id: String,
    pub observer_id: String,
    pub observer_version: u32,
    pub source_step: u64,
    pub visible_variables: Vec<ObservedVariable>,
}

/// Явное вмешательство в одну переменную состояния.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticIntervention {
    pub variable_key: String,
    pub delta: i64,
}

/// Ошибки синтетической модели.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExperimentError {
    EmptyStateId,
    DuplicateVariableKey,
    EmptyVariableKey,
    EmptyObserverId,
    ZeroObserverVersion,
    ZeroResolution,
    MissingInterventionTarget,
    ArithmeticOverflow,
}

impl SyntheticState {
    pub fn validate(&self) -> Result<(), ExperimentError> {
        if self.state_id.trim().is_empty() {
            return Err(ExperimentError::EmptyStateId);
        }

        for (index, variable) in self.variables.iter().enumerate() {
            if variable.key.trim().is_empty() {
                return Err(ExperimentError::EmptyVariableKey);
            }
            if self.variables[..index]
                .iter()
                .any(|previous| previous.key == variable.key)
            {
                return Err(ExperimentError::DuplicateVariableKey);
            }
        }

        Ok(())
    }
}

impl SyntheticObserver {
    pub fn validate(&self) -> Result<(), ExperimentError> {
        if self.observer_id.trim().is_empty() {
            return Err(ExperimentError::EmptyObserverId);
        }
        if self.version == 0 {
            return Err(ExperimentError::ZeroObserverVersion);
        }
        if self.resolution == 0 {
            return Err(ExperimentError::ZeroResolution);
        }
        Ok(())
    }
}

/// Формирует представление по доступной проекции состояния.
///
/// Функция не изменяет исходное состояние. Повторный вызов с одинаковыми
/// аргументами возвращает тот же результат, включая шум.
pub fn observe(
    state: &SyntheticState,
    observer: &SyntheticObserver,
) -> Result<ObservationView, ExperimentError> {
    state.validate()?;
    observer.validate()?;

    let mut visible_variables = Vec::new();

    for variable in &state.variables {
        if !observer.visible_keys.iter().any(|key| key == &variable.key) {
            continue;
        }

        let resolution = i128::from(observer.resolution);
        let quantized = i128::from(variable.value).div_euclid(resolution) * resolution;
        let noise = deterministic_noise(
            observer.noise_seed,
            &observer.observer_id,
            &variable.key,
            observer.noise_bound,
        );
        let measured = quantized + noise;

        let value = i64::try_from(measured).map_err(|_| ExperimentError::ArithmeticOverflow)?;
        visible_variables.push(ObservedVariable {
            key: variable.key.clone(),
            value,
        });
    }

    Ok(ObservationView {
        state_id: state.state_id.clone(),
        observer_id: observer.observer_id.clone(),
        observer_version: observer.version,
        source_step: state.step,
        visible_variables,
    })
}

/// Применяет вмешательство и возвращает новое состояние.
///
/// Исходное состояние остаётся неизменным. Увеличение шага и изменение
/// значения выполняются с проверкой переполнения.
pub fn apply_intervention(
    state: &SyntheticState,
    intervention: &SyntheticIntervention,
) -> Result<SyntheticState, ExperimentError> {
    state.validate()?;

    let mut next = state.clone();
    let variable = next
        .variables
        .iter_mut()
        .find(|variable| variable.key == intervention.variable_key)
        .ok_or(ExperimentError::MissingInterventionTarget)?;

    variable.value = variable
        .value
        .checked_add(intervention.delta)
        .ok_or(ExperimentError::ArithmeticOverflow)?;
    next.step = next
        .step
        .checked_add(1)
        .ok_or(ExperimentError::ArithmeticOverflow)?;

    Ok(next)
}

fn deterministic_noise(seed: u64, observer_id: &str, key: &str, bound: u64) -> i128 {
    if bound == 0 {
        return 0;
    }

    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
    for byte in observer_id.bytes().chain([0xff]).chain(key.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    let bound = i128::from(bound);
    let width = bound * 2 + 1;
    i128::from(hash) % width - bound
}
