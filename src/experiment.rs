//! Минимальное синтетическое ядро экспериментов с наблюдателями.
//!
//! Скрытое состояние, проекция наблюдения и вмешательство разделены.
//! Вызов observe не изменяет состояние. Изменение возможно только через
//! явный apply_intervention. Типы здесь описывают синтетическую модель,
//! а не онтологию реального смыслового поля.

use std::collections::BTreeSet;

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

/// Конфигурация синтетической проекции наблюдателя.
///
/// Важно: SyntheticObserver создаётся вызывающей стороной. Поле
/// `visible_keys` задаёт только проекцию данных для эксперимента и НЕ является
/// проверкой полномочий, границей конфиденциальности или механизмом изоляции.
/// Не передавайте через этот API секретные данные в расчёте на сокрытие ключей.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticObserver {
    pub observer_id: String,
    pub version: u32,
    /// Фильтр проекции, а не список разрешений безопасности.
    /// Пустой список означает отсутствие переменных в представлении.
    pub visible_keys: Vec<String>,
    /// Шаг квантования; 1 означает отсутствие квантования.
    pub resolution: u64,
    /// Максимальная абсолютная добавляемая ошибка измерения.
    /// Только синтетический шум; не средство приватности или сокрытия данных.
    pub noise_bound: u64,
    /// Публичный seed детерминированного шума симуляции.
    /// Не предназначен для генерации секретного или криптографического шума.
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
    NoiseBoundOutOfRange,
    DuplicateVisibleKey,
    UnknownVisibleKey,
    MissingInterventionTarget,
    ArithmeticOverflow,
}

impl SyntheticState {
    pub fn validate(&self) -> Result<(), ExperimentError> {
        if self.state_id.trim().is_empty() {
            return Err(ExperimentError::EmptyStateId);
        }

        let mut keys = BTreeSet::new();
        for variable in &self.variables {
            if variable.key.trim().is_empty() {
                return Err(ExperimentError::EmptyVariableKey);
            }
            if !keys.insert(variable.key.as_str()) {
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
        if self.noise_bound > i64::MAX as u64 {
            return Err(ExperimentError::NoiseBoundOutOfRange);
        }
        let mut visible_keys = BTreeSet::new();
        for key in &self.visible_keys {
            if !visible_keys.insert(key.as_str()) {
                return Err(ExperimentError::DuplicateVisibleKey);
            }
        }
        Ok(())
    }
}

/// Формирует представление по фильтру проекции состояния.
///
/// `visible_keys` не проверяет полномочия вызывающей стороны. Вызывающий код
/// может сформировать SyntheticObserver с любым набором ключей, включая все
/// ключи состояния. Это намеренно простая синтетическая проекция, а не
/// security boundary. Контроль доступа, если он нужен приложению, должен
/// выполняться отдельным доверенным слоем до передачи данных в эту функцию.
///
/// Функция не изменяет исходное состояние. Повторный вызов с одинаковыми
/// аргументами возвращает то же представление. Детерминированный шум является
/// только параметром симуляции: он не обеспечивает конфиденциальность,
/// дифференциальную приватность или криптографическую непредсказуемость.
pub fn observe(
    state: &SyntheticState,
    observer: &SyntheticObserver,
) -> Result<ObservationView, ExperimentError> {
    state.validate()?;
    observer.validate()?;
    let state_keys: BTreeSet<&str> = state
        .variables
        .iter()
        .map(|variable| variable.key.as_str())
        .collect();
    let visible_keys: BTreeSet<&str> = observer
        .visible_keys
        .iter()
        .map(String::as_str)
        .collect();

    if visible_keys.iter().any(|key| !state_keys.contains(key)) {
        return Err(ExperimentError::UnknownVisibleKey);
    }

    // Общая часть хеша вычисляется один раз на наблюдение, а не повторно
    // для каждой переменной. Это устраняет умножение длины ID на число ключей.
    let shared_noise_prefix = noise_prefix(
        observer.noise_seed,
        &observer.observer_id,
        &state.state_id,
        state.step,
    );

    let mut visible_variables = Vec::new();

    for variable in &state.variables {
        if !visible_keys.contains(variable.key.as_str()) {
            continue;
        }

        let resolution = i128::from(observer.resolution);
        let quantized = i128::from(variable.value).div_euclid(resolution) * resolution;
        let noise = deterministic_noise(
            shared_noise_prefix,
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

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn noise_prefix(seed: u64, observer_id: &str, state_id: &str, step: u64) -> u64 {
    let hash = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
    let hash = hash_bytes(hash, observer_id.as_bytes());
    let hash = hash_bytes(hash, &[0xff]);
    let hash = hash_bytes(hash, state_id.as_bytes());
    let hash = hash_bytes(hash, &step.to_le_bytes());
    hash_bytes(hash, &[0xfe])
}

fn deterministic_noise(prefix: u64, key: &str, bound: u64) -> i128 {
    if bound == 0 {
        return 0;
    }

    let hash = hash_bytes(prefix, key.as_bytes());

    // Для bound <= i64::MAX ширина 2 * bound + 1 помещается в u64.
    // Преобразование в i128 до сложения исключает переполнение.
    let bound = i128::from(bound);
    let width = bound * 2 + 1;
    i128::from(hash) % width - bound
}
