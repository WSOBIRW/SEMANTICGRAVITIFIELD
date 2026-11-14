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

/// Ошибки причинной проверки исполнения после момента формирования сигнала.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardVerificationError {
    InvalidHoldingPeriod,
    MissingForwardEntry,
    MissingForwardExit,
    UnsupportedSchema(u16),
    InvalidQuote,
    OutOfOrderEvents,
    MixedSources,
    ArithmeticOverflow,
    EconomicEvaluation,
}

/// Строит одну причинную проверку: сигнал уже сформирован к decision_time,
/// вход выбирается только из более поздних доступных котировок, а выход —
/// из первой котировки не раньше заданного горизонта удержания.
///
/// Важно: функция не подбирает горизонт по результатам теста и не моделирует
/// очередь, глубину, задержку передачи ордера или гарантированное исполнение.
/// Она устраняет конкретную утечку будущего в выборе входа/выхода.
pub fn evaluate_forward_round_trip(
    events: &[MarketEvent],
    candidate: &crate::research::OpportunityCandidate,
    holding_period_ms: i64,
    config: crate::economic::EconomicConfig,
) -> Result<crate::economic::EconomicAssessment, ForwardVerificationError> {
    if holding_period_ms <= 0 || candidate.end_time_ms <= candidate.start_time_ms {
        return Err(ForwardVerificationError::InvalidHoldingPeriod);
    }

    let decision_time_ms = candidate.end_time_ms;
    let mut previous_event_time_ms = None;
    let mut source_id: Option<&str> = None;
    let mut entry: Option<&MarketEvent> = None;
    let mut exit: Option<&MarketEvent> = None;
    let mut exit_target_ms = None;

    for event in events {
        if event.instrument_id != candidate.instrument_id {
            continue;
        }
        if let Some(previous) = previous_event_time_ms {
            if event.event_time_ms < previous {
                return Err(ForwardVerificationError::OutOfOrderEvents);
            }
        }
        previous_event_time_ms = Some(event.event_time_ms);

        if event.schema_version != 1 {
            return Err(ForwardVerificationError::UnsupportedSchema(
                event.schema_version,
            ));
        }

        let crate::event::EventPayload::Quote {
            bid_ticks,
            ask_ticks,
            price_scale,
        } = &event.payload
        else {
            continue;
        };
        if *price_scale == 0 || *bid_ticks <= 0 || *ask_ticks < *bid_ticks {
            return Err(ForwardVerificationError::InvalidQuote);
        }

        match source_id {
            Some(existing) if existing != event.source_id => {
                return Err(ForwardVerificationError::MixedSources);
            }
            None => source_id = Some(&event.source_id),
            _ => {}
        }

        if entry.is_none() {
            // Котировка должна относиться к будущему относительно сигнала
            // и быть получена не раньше момента принятия решения.
            if event.event_time_ms >= decision_time_ms && event.received_time_ms >= decision_time_ms
            {
                let target = event
                    .received_time_ms
                    .checked_add(holding_period_ms)
                    .ok_or(ForwardVerificationError::ArithmeticOverflow)?;
                entry = Some(event);
                exit_target_ms = Some(target);
            }
            continue;
        }

        if exit.is_none()
            && event.event_time_ms >= exit_target_ms.unwrap_or(i64::MAX)
            && event.received_time_ms >= exit_target_ms.unwrap_or(i64::MAX)
        {
            exit = Some(event);
            break;
        }
    }

    let entry = entry.ok_or(ForwardVerificationError::MissingForwardEntry)?;
    let exit = exit.ok_or(ForwardVerificationError::MissingForwardExit)?;

    let mut forward_candidate = candidate.clone();
    forward_candidate.start_time_ms = entry.event_time_ms;
    forward_candidate.end_time_ms = exit.event_time_ms;

    crate::economic::evaluate_round_trip(&[entry.clone(), exit.clone()], &forward_candidate, config)
        .map_err(|_| ForwardVerificationError::EconomicEvaluation)
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

#[cfg(test)]
mod forward_execution_tests {
    use super::*;
    use crate::economic::{EconomicConfig, EconomicVerdict};
    use crate::event::Quote;
    use crate::research::{OpportunityCandidate, PriceDirection};

    fn quote(
        id: &str,
        event_time_ms: i64,
        received_time_ms: i64,
        bid: i64,
        ask: i64,
    ) -> MarketEvent {
        MarketEvent::quote(
            id,
            "test-source",
            "BTC_USDT",
            event_time_ms,
            received_time_ms,
            None,
            Quote {
                bid_ticks: bid,
                ask_ticks: ask,
                price_scale: 100_000,
            },
        )
    }

    fn candidate() -> OpportunityCandidate {
        OpportunityCandidate {
            instrument_id: "BTC_USDT".to_owned(),
            direction: PriceDirection::Up,
            movement_bps: 100,
            maximum_observed_spread_bps: 10,
            start_time_ms: 10,
            end_time_ms: 100,
            trade_count: 2,
        }
    }

    fn costs() -> EconomicConfig {
        EconomicConfig {
            entry_fee_bps: 0,
            exit_fee_bps: 0,
            entry_slippage_bps: 0,
            exit_slippage_bps: 0,
            funding_cost_bps: 0,
        }
    }

    #[test]
    fn selects_entry_and_first_exit_only_after_signal_and_holding_horizon() {
        let events = vec![
            quote("before", 90, 90, 9_900, 9_910),
            quote("entry", 110, 110, 10_000, 10_010),
            quote("too-early", 125, 125, 10_100, 10_110),
            quote("exit", 130, 130, 10_200, 10_210),
            quote("later-better", 190, 190, 10_500, 10_510),
        ];

        let result = evaluate_forward_round_trip(&events, &candidate(), 20, costs()).unwrap();

        assert_eq!(result.entry_time_ms, 110);
        assert_eq!(result.exit_time_ms, 130);
        assert_eq!(result.entry_price_ticks, 10_010);
        assert_eq!(result.exit_price_ticks, 10_200);
        assert_eq!(result.verdict, EconomicVerdict::Positive);
    }

    #[test]
    fn rejects_a_quote_from_before_the_signal_even_if_received_late() {
        let events = vec![
            quote("late-old", 95, 105, 10_000, 10_010),
            quote("entry", 110, 110, 10_000, 10_010),
            quote("exit", 130, 130, 10_100, 10_110),
        ];

        let result = evaluate_forward_round_trip(&events, &candidate(), 20, costs()).unwrap();
        assert_eq!(result.entry_time_ms, 110);
    }

    #[test]
    fn refuses_to_invent_an_exit_when_forward_horizon_is_not_observed() {
        let events = vec![
            quote("entry", 110, 110, 10_000, 10_010),
            quote("too-early", 129, 129, 10_100, 10_110),
        ];

        assert_eq!(
            evaluate_forward_round_trip(&events, &candidate(), 20, costs()),
            Err(ForwardVerificationError::MissingForwardExit)
        );
    }
}
