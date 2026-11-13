use crate::event::{EventPayload, MarketEvent};
use crate::research::{OpportunityCandidate, PriceDirection};

/// Издержки одного полного входа и выхода.
///
/// Значения комиссий и проскальзывания задаются на одну сторону сделки.
/// Для MEXC Futures сюда передаются значения, соответствующие конкретному
/// контракту, типу исполнения и тарифному уровню. Значения не зашиты в ядро.
///
/// funding_cost_bps — суммарное финансирование за время удержания позиции:
/// положительное значение означает расход, отрицательное — полученный доход.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EconomicConfig {
    pub fee_bps_per_side: u64,
    pub slippage_bps_per_side: u64,
    pub funding_cost_bps: i64,
}

/// Ошибка оценки экономического результата.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EconomicError {
    NoCandidate,
    MissingEntryQuote,
    MissingExitQuote,
    NonPositiveHoldingInterval,
    OutOfOrderQuotes,
    UnsupportedSchema(u16),
    InvalidQuote,
    MixedPriceScales,
    MixedSources,
    InvalidCandidate,
    ArithmeticOverflow,
}

/// Итоговая классификация результата одного условного round-trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EconomicVerdict {
    Positive,
    Negative,
    BreakEven,
}

/// Оценка одного условного входа и выхода по наблюдаемым котировкам.
///
/// Это оценка по историческим top-of-book котировкам, а не симуляция очереди,
/// глубины стакана, задержки или гарантированного исполнения.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EconomicAssessment {
    pub instrument_id: String,
    pub source_id: String,
    pub direction: PriceDirection,
    pub entry_time_ms: i64,
    pub exit_time_ms: i64,
    pub entry_price_ticks: i64,
    pub exit_price_ticks: i64,
    /// Валовой результат до издержек, в bps от цены входа.
    pub gross_return_bps: i128,
    /// Комиссия за вход и выход, в bps.
    pub round_trip_fee_bps: i128,
    /// Заданное проскальзывание входа и выхода, в bps.
    pub round_trip_slippage_bps: i128,
    /// Суммарное финансирование за период удержания, в bps.
    pub funding_cost_bps: i128,
    /// Сумма всех учтённых издержек. Может быть отрицательной при доходе от funding.
    pub total_cost_bps: i128,
    /// Валовой результат минус все учтённые издержки, в bps.
    pub net_return_bps: i128,
    pub verdict: EconomicVerdict,
}

/// Оценивает условную покупку/продажу и последующее закрытие позиции.
///
/// Для длинной позиции вход выполняется по ask первой котировки, выход —
/// по bid последней котировки. Для короткой позиции вход выполняется по bid,
/// выход — по ask. Это учитывает наблюдаемый bid/ask spread без его повторного
/// вычитания в качестве отдельной статьи.
///
/// Комиссии, проскальзывание и funding задаются явно. Не заданные пользователем
/// параметры не подменяются выдуманными значениями.
pub fn evaluate_round_trip(
    events: &[MarketEvent],
    candidate: &OpportunityCandidate,
    config: EconomicConfig,
) -> Result<EconomicAssessment, EconomicError> {
    let fee = i128::from(config.fee_bps_per_side)
        .checked_mul(2)
        .ok_or(EconomicError::ArithmeticOverflow)?;
    let slippage = i128::from(config.slippage_bps_per_side)
        .checked_mul(2)
        .ok_or(EconomicError::ArithmeticOverflow)?;
    let funding = i128::from(config.funding_cost_bps);

    let mut first_quote: Option<&MarketEvent> = None;
    let mut last_quote: Option<&MarketEvent> = None;
    let mut price_scale: Option<u32> = None;
    let mut previous_quote_time_ms: Option<i64> = None;
    let mut quote_count = 0usize;
    let mut source_id: Option<&str> = None;

    if candidate.end_time_ms <= candidate.start_time_ms {
        return Err(EconomicError::NonPositiveHoldingInterval);
    }
    if (candidate.direction == PriceDirection::Up && candidate.movement_bps <= 0)
        || (candidate.direction == PriceDirection::Down && candidate.movement_bps >= 0)
        || candidate.direction == PriceDirection::Flat
    {
        return Err(EconomicError::InvalidCandidate);
    }

    for event in events {
        if event.instrument_id != candidate.instrument_id
            || event.event_time_ms < candidate.start_time_ms
            || event.event_time_ms > candidate.end_time_ms
        {
            continue;
        }
        if event.schema_version != 1 {
            return Err(EconomicError::UnsupportedSchema(event.schema_version));
        }
        if let EventPayload::Quote {
            bid_ticks,
            ask_ticks,
            price_scale: incoming_scale,
        } = &event.payload
        {
            if *incoming_scale == 0 || *bid_ticks <= 0 || *ask_ticks < *bid_ticks {
                return Err(EconomicError::InvalidQuote);
            }
            match price_scale {
                Some(existing) if *existing != *incoming_scale => {
                    return Err(EconomicError::MixedPriceScales);
                }
                None => price_scale = Some(*incoming_scale),
                _ => {}
            }
            if let Some(previous) = previous_quote_time_ms {
                if event.event_time_ms < previous {
                    return Err(EconomicError::OutOfOrderQuotes);
                }
            }
            previous_quote_time_ms = Some(event.event_time_ms);
            match source_id {
                Some(existing) if existing != event.source_id => {
                    return Err(EconomicError::MixedSources);
                }
                None => source_id = Some(&event.source_id),
                _ => {}
            }
            quote_count += 1;
            if first_quote.is_none() {
                first_quote = Some(event);
            }
            last_quote = Some(event);
        }
    }

    let entry = first_quote.ok_or(EconomicError::MissingEntryQuote)?;
    let exit = last_quote.ok_or(EconomicError::MissingExitQuote)?;
    if quote_count < 2 {
        return Err(EconomicError::MissingExitQuote);
    }
    if std::ptr::eq(entry, exit) || exit.event_time_ms <= entry.event_time_ms {
        return Err(EconomicError::NonPositiveHoldingInterval);
    }

    let (entry_price, exit_price) = match candidate.direction {
        PriceDirection::Up => (quote_ask(&entry.payload)?, quote_bid(&exit.payload)?),
        PriceDirection::Down => (quote_bid(&entry.payload)?, quote_ask(&exit.payload)?),
        PriceDirection::Flat => return Err(EconomicError::NoCandidate),
    };

    let price_delta = match candidate.direction {
        PriceDirection::Up => i128::from(exit_price) - i128::from(entry_price),
        PriceDirection::Down => i128::from(entry_price) - i128::from(exit_price),
        PriceDirection::Flat => return Err(EconomicError::NoCandidate),
    };
    let numerator = price_delta
        .checked_mul(10_000)
        .ok_or(EconomicError::ArithmeticOverflow)?;
    let gross = floor_div(numerator, i128::from(entry_price));

    let total_cost = fee
        .checked_add(slippage)
        .and_then(|value| value.checked_add(funding))
        .ok_or(EconomicError::ArithmeticOverflow)?;
    let net = gross
        .checked_sub(total_cost)
        .ok_or(EconomicError::ArithmeticOverflow)?;

    let verdict = if net > 0 {
        EconomicVerdict::Positive
    } else if net < 0 {
        EconomicVerdict::Negative
    } else {
        EconomicVerdict::BreakEven
    };

    Ok(EconomicAssessment {
        instrument_id: candidate.instrument_id.clone(),
        source_id: entry.source_id.clone(),
        direction: candidate.direction,
        entry_time_ms: entry.event_time_ms,
        exit_time_ms: exit.event_time_ms,
        entry_price_ticks: entry_price,
        exit_price_ticks: exit_price,
        gross_return_bps: gross,
        round_trip_fee_bps: fee,
        round_trip_slippage_bps: slippage,
        funding_cost_bps: funding,
        total_cost_bps: total_cost,
        net_return_bps: net,
        verdict,
    })
}

fn quote_bid(payload: &EventPayload) -> Result<i64, EconomicError> {
    match payload {
        EventPayload::Quote { bid_ticks, .. } => Ok(*bid_ticks),
        EventPayload::Trade { .. } => Err(EconomicError::MissingEntryQuote),
    }
}

fn quote_ask(payload: &EventPayload) -> Result<i64, EconomicError> {
    match payload {
        EventPayload::Quote { ask_ticks, .. } => Ok(*ask_ticks),
        EventPayload::Trade { .. } => Err(EconomicError::MissingExitQuote),
    }
}

/// Деление с округлением вниз, в том числе для отрицательного результата.
fn floor_div(numerator: i128, denominator: i128) -> i128 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder != 0 && numerator < 0 {
        quotient - 1
    } else {
        quotient
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Quote;

    const SCALE: u32 = 100_000;

    fn quote(id: &str, time: i64, bid: i64, ask: i64) -> MarketEvent {
        MarketEvent::quote(
            id,
            "mexc-futures-ws",
            "BTC_USDT",
            time,
            time,
            None,
            Quote {
                bid_ticks: bid,
                ask_ticks: ask,
                price_scale: SCALE,
            },
        )
    }

    fn candidate(direction: PriceDirection) -> OpportunityCandidate {
        OpportunityCandidate {
            instrument_id: "BTC_USDT".to_owned(),
            direction,
            movement_bps: 100,
            maximum_observed_spread_bps: 2,
            start_time_ms: 1_000,
            end_time_ms: 2_000,
            trade_count: 2,
        }
    }

    fn config() -> EconomicConfig {
        EconomicConfig {
            fee_bps_per_side: 2,
            slippage_bps_per_side: 1,
            funding_cost_bps: 0,
        }
    }

    #[test]
    fn long_round_trip_uses_ask_to_enter_and_bid_to_exit() {
        let events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 2_000, 10_200, 10_210),
        ];
        let result =
            evaluate_round_trip(&events, &candidate(PriceDirection::Up), config()).unwrap();

        assert_eq!(result.entry_price_ticks, 10_010);
        assert_eq!(result.exit_price_ticks, 10_200);
        assert_eq!(result.gross_return_bps, 189);
        assert_eq!(result.round_trip_fee_bps, 4);
        assert_eq!(result.round_trip_slippage_bps, 2);
        assert_eq!(result.net_return_bps, 183);
        assert_eq!(result.verdict, EconomicVerdict::Positive);
    }

    #[test]
    fn short_round_trip_uses_bid_to_enter_and_ask_to_exit() {
        let events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 2_000, 9_800, 9_810),
        ];
        let result =
            evaluate_round_trip(&events, &candidate(PriceDirection::Down), config()).unwrap();

        assert_eq!(result.entry_price_ticks, 10_000);
        assert_eq!(result.exit_price_ticks, 9_810);
        assert_eq!(result.gross_return_bps, 190);
        assert_eq!(result.net_return_bps, 184);
        assert_eq!(result.verdict, EconomicVerdict::Positive);
    }

    #[test]
    fn negative_funding_cost_represents_funding_income() {
        let events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 2_000, 10_000, 10_010),
        ];
        let config = EconomicConfig {
            fee_bps_per_side: 0,
            slippage_bps_per_side: 0,
            funding_cost_bps: -3,
        };
        let result = evaluate_round_trip(&events, &candidate(PriceDirection::Up), config).unwrap();

        assert_eq!(result.funding_cost_bps, -3);
        assert_eq!(result.net_return_bps, -3);
        assert_eq!(result.verdict, EconomicVerdict::Negative);
    }

    #[test]
    fn losing_move_is_not_hidden_by_truncation_toward_zero() {
        let events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 2_000, 9_999, 10_009),
        ];
        let result =
            evaluate_round_trip(&events, &candidate(PriceDirection::Up), config()).unwrap();

        assert!(result.gross_return_bps < 0);
        assert!(result.net_return_bps < result.gross_return_bps);
        assert_eq!(result.verdict, EconomicVerdict::Negative);
    }

    #[test]
    fn rejects_missing_exit_quote() {
        let events = vec![quote("q1", 1_000, 10_000, 10_010)];
        assert_eq!(
            evaluate_round_trip(&events, &candidate(PriceDirection::Up), config()),
            Err(EconomicError::MissingExitQuote)
        );
    }

    #[test]
    fn rejects_non_positive_holding_interval() {
        let events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 1_000, 10_100, 10_110),
        ];
        assert_eq!(
            evaluate_round_trip(&events, &candidate(PriceDirection::Up), config()),
            Err(EconomicError::NonPositiveHoldingInterval)
        );
    }

    #[test]
    fn rejects_unsupported_schema() {
        let mut events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 2_000, 10_100, 10_110),
        ];
        events[1].schema_version = 2;
        assert_eq!(
            evaluate_round_trip(&events, &candidate(PriceDirection::Up), config()),
            Err(EconomicError::UnsupportedSchema(2))
        );
    }

    #[test]
    fn rejects_mixed_price_scales() {
        let mut events = vec![
            quote("q1", 1_000, 10_000, 10_010),
            quote("q2", 2_000, 10_100, 10_110),
        ];
        if let EventPayload::Quote { price_scale, .. } = &mut events[1].payload {
            *price_scale = SCALE + 1;
        }
        assert_eq!(
            evaluate_round_trip(&events, &candidate(PriceDirection::Up), config()),
            Err(EconomicError::MixedPriceScales)
        );
    }
}
