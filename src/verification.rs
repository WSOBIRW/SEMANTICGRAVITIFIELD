use crate::event::MarketEvent;

/// Границы временного разделения данных для проверки вне обучающего периода.
///
/// Обе границы задаются в Unix milliseconds UTC. Интервал проверки полуоткрытый:
/// [test_start_ms, test_end_ms). Обучающие события должны быть известны
/// до train_end_ms, включая время получения события.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChronologicalSplitConfig {
    pub train_end_ms: i64,
    pub test_start_ms: i64,
    pub test_end_ms: i64,
}

/// Индексы событий после временного разделения.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChronologicalSplit {
    pub training_indices: Vec<usize>,
    pub testing_indices: Vec<usize>,
    pub excluded_late_training_events: usize,
    pub excluded_gap_events: usize,
    pub excluded_outside_period_events: usize,
}

/// Ошибки построения временного разделения.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitError {
    InvalidBoundary,
    NegativeTimestamp,
    EmptyTrainingSet,
    EmptyTestingSet,
}

/// Делит события по времени без случайного перемешивания.
///
/// В обучающую часть попадают только события, у которых и время события,
/// и время получения строго меньше train_end_ms. Поздно полученные события
/// из обучающего интервала исключаются, чтобы не использовать сведения,
/// которые не были доступны на момент отсечения.
///
/// Проверочная часть включает события из [test_start_ms, test_end_ms),
/// полученные до test_end_ms. События между обучающей и проверочной
/// границами не переносятся ни в одну часть.
///
/// Функция не оценивает прибыль, статистическую значимость или устойчивость
/// гипотезы. Она обеспечивает только временную изоляцию входных событий.
pub fn split_chronologically(
    events: &[MarketEvent],
    config: ChronologicalSplitConfig,
) -> Result<ChronologicalSplit, SplitError> {
    if config.train_end_ms < 0
        || config.test_start_ms < config.train_end_ms
        || config.test_end_ms <= config.test_start_ms
    {
        return Err(SplitError::InvalidBoundary);
    }

    let mut result = ChronologicalSplit {
        training_indices: Vec::new(),
        testing_indices: Vec::new(),
        excluded_late_training_events: 0,
        excluded_gap_events: 0,
        excluded_outside_period_events: 0,
    };

    for (index, event) in events.iter().enumerate() {
        if event.event_time_ms < 0 || event.received_time_ms < 0 {
            return Err(SplitError::NegativeTimestamp);
        }

        if event.event_time_ms < config.train_end_ms {
            if event.received_time_ms < config.train_end_ms {
                result.training_indices.push(index);
            } else {
                result.excluded_late_training_events += 1;
            }
        } else if event.event_time_ms >= config.test_start_ms
            && event.event_time_ms < config.test_end_ms
            && event.received_time_ms < config.test_end_ms
        {
            result.testing_indices.push(index);
        } else if event.event_time_ms < config.test_start_ms {
            result.excluded_gap_events += 1;
        } else {
            result.excluded_outside_period_events += 1;
        }
    }

    if result.training_indices.is_empty() {
        return Err(SplitError::EmptyTrainingSet);
    }
    if result.testing_indices.is_empty() {
        return Err(SplitError::EmptyTestingSet);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Quote;

    fn event(id: &str, event_time_ms: i64, received_time_ms: i64) -> MarketEvent {
        MarketEvent::quote(
            id,
            "test-source",
            "BTC_USDT",
            event_time_ms,
            received_time_ms,
            None,
            Quote {
                bid_ticks: 10_000,
                ask_ticks: 10_010,
                price_scale: 100_000,
            },
        )
    }

    fn config() -> ChronologicalSplitConfig {
        ChronologicalSplitConfig {
            train_end_ms: 100,
            test_start_ms: 120,
            test_end_ms: 200,
        }
    }

    #[test]
    fn separates_training_and_testing_by_event_time() {
        let events = vec![
            event("a", 10, 10),
            event("b", 99, 99),
            event("c", 120, 120),
            event("d", 199, 199),
        ];

        let split = split_chronologically(&events, config()).unwrap();
        assert_eq!(split.training_indices, vec![0, 1]);
        assert_eq!(split.testing_indices, vec![2, 3]);
    }

    #[test]
    fn excludes_late_arriving_training_events() {
        let events = vec![
            event("a", 50, 50),
            event("late", 90, 110),
            event("test", 130, 130),
        ];

        let split = split_chronologically(&events, config()).unwrap();
        assert_eq!(split.training_indices, vec![0]);
        assert_eq!(split.testing_indices, vec![2]);
        assert_eq!(split.excluded_late_training_events, 1);
    }

    #[test]
    fn excludes_gap_and_outside_period_events() {
        let events = vec![
            event("train", 50, 50),
            event("gap", 110, 110),
            event("test", 130, 130),
            event("after", 220, 220),
        ];

        let split = split_chronologically(&events, config()).unwrap();
        assert_eq!(split.excluded_gap_events, 1);
        assert_eq!(split.excluded_outside_period_events, 1);
    }

    #[test]
    fn rejects_overlapping_or_reversed_boundaries() {
        let events = vec![event("train", 10, 10), event("test", 130, 130)];
        let invalid = ChronologicalSplitConfig {
            train_end_ms: 140,
            test_start_ms: 120,
            test_end_ms: 200,
        };
        assert_eq!(
            split_chronologically(&events, invalid),
            Err(SplitError::InvalidBoundary)
        );
    }

    #[test]
    fn rejects_negative_timestamps() {
        let events = vec![event("train", -1, 0), event("test", 130, 130)];
        assert_eq!(
            split_chronologically(&events, config()),
            Err(SplitError::NegativeTimestamp)
        );
    }

    #[test]
    fn rejects_empty_training_or_testing_period() {
        let no_training = vec![event("test", 130, 130)];
        assert_eq!(
            split_chronologically(&no_training, config()),
            Err(SplitError::EmptyTrainingSet)
        );

        let no_testing = vec![event("train", 10, 10)];
        assert_eq!(
            split_chronologically(&no_testing, config()),
            Err(SplitError::EmptyTestingSet)
        );
    }

    #[test]
    fn excludes_test_events_received_after_test_end() {
        let events = vec![
            event("train", 10, 10),
            event("late-test", 150, 210),
            event("test", 160, 160),
        ];

        let split = split_chronologically(&events, config()).unwrap();
        assert_eq!(split.testing_indices, vec![2]);
        assert_eq!(split.excluded_outside_period_events, 1);
    }
}
