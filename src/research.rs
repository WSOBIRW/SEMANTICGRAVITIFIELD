use crate::event::{EventPayload, MarketEvent};

/// Параметры первичного отбора кандидатов.
///
/// Пороговые значения являются параметрами эксперимента, а не
/// универсальными торговыми рекомендациями.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResearchConfig {
    /// Минимальное абсолютное изменение цены между первой и последней сделками, в bps.
    pub minimum_absolute_move_bps: u64,
    /// Максимальный наблюдавшийся спред котировки, в bps.
    pub maximum_spread_bps: u64,
    /// Минимальное число сделок в исследуемом окне.
    pub minimum_trade_count: usize,
}

/// Ошибка при анализе окна рыночных событий.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchError {
    InvalidConfig,
    UnsupportedSchema(u16),
    EmptyWindow,
    EmptyInstrumentId,
    EmptySourceId,
    MixedInstruments,
    MixedSources,
    OutOfOrderEvents {
        previous_time_ms: i64,
        current_time_ms: i64,
    },
    InvalidEventTime(i64),
    InvalidPriceScale,
    MixedPriceScales,
    InvalidQuote,
    InvalidTrade,
    TradeQuantityOverflow,
}

/// Направление изменения цены внутри исследуемого окна.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceDirection {
    Up,
    Down,
    Flat,
}

/// Измеряемые характеристики одного окна рынка.
///
/// Метрики описывают наблюдавшиеся данные и сами по себе не доказывают
/// наличие торгового преимущества.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketWindowMetrics {
    pub instrument_id: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub event_count: usize,
    pub quote_count: usize,
    pub trade_count: usize,
    pub total_trade_quantity_lots: u128,
    pub first_trade_price_ticks: Option<i64>,
    pub last_trade_price_ticks: Option<i64>,
    /// Изменение между первой и последней сделками, в базисных пунктах.
    pub movement_bps: Option<i128>,
    /// Максимальный наблюдавшийся спред котировки, в базисных пунктах.
    pub maximum_observed_spread_bps: Option<u64>,
}

/// Кандидат, прошедший простые фильтры исследовательского окна.
///
/// Это только объект для дальнейшего исследования. Он не является
/// торговым сигналом, рекомендацией открыть позицию или доказательством прибыли.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpportunityCandidate {
    pub instrument_id: String,
    pub direction: PriceDirection,
    pub movement_bps: i128,
    pub maximum_observed_spread_bps: u64,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub trade_count: usize,
}

/// Результат анализа окна.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchReport {
    pub metrics: MarketWindowMetrics,
    pub candidate: Option<OpportunityCandidate>,
}

/// Анализирует одно упорядоченное окно событий одного инструмента.
///
/// Входные события должны быть предварительно проверены EventValidator.
/// Кандидат формируется только при наличии сделок и котировок, достаточном
/// движении цены, допустимом максимальном спреде и достаточном числе сделок.
/// Никакая доходность или исполнимость сделки здесь не моделируется.
pub fn analyze_market_window(
    events: &[MarketEvent],
    config: ResearchConfig,
) -> Result<ResearchReport, ResearchError> {
    if config.minimum_absolute_move_bps == 0 || config.minimum_trade_count < 2 {
        return Err(ResearchError::InvalidConfig);
    }

    let first = events.first().ok_or(ResearchError::EmptyWindow)?;
    if first.instrument_id.trim().is_empty() {
        return Err(ResearchError::EmptyInstrumentId);
    }

    if first.source_id.trim().is_empty() {
        return Err(ResearchError::EmptySourceId);
    }

    let instrument_id = first.instrument_id.clone();
    let source_id = first.source_id.clone();
    let start_time_ms = first.event_time_ms;
    let mut end_time_ms = first.event_time_ms;
    let mut previous_time_ms = None;
    let mut price_scale = None;

    let mut quote_count = 0usize;
    let mut trade_count = 0usize;
    let mut total_trade_quantity_lots = 0u128;
    let mut first_trade_price_ticks = None;
    let mut last_trade_price_ticks = None;
    let mut maximum_observed_spread_bps = None;

    for event in events {
        if event.schema_version != 1 {
            return Err(ResearchError::UnsupportedSchema(event.schema_version));
        }
        if event.instrument_id.trim().is_empty() {
            return Err(ResearchError::EmptyInstrumentId);
        }
        if event.source_id.trim().is_empty() {
            return Err(ResearchError::EmptySourceId);
        }
        if event.source_id != source_id {
            return Err(ResearchError::MixedSources);
        }
        if event.instrument_id != instrument_id {
            return Err(ResearchError::MixedInstruments);
        }
        if event.event_time_ms < 0 {
            return Err(ResearchError::InvalidEventTime(event.event_time_ms));
        }
        if let Some(previous) = previous_time_ms {
            if event.event_time_ms < previous {
                return Err(ResearchError::OutOfOrderEvents {
                    previous_time_ms: previous,
                    current_time_ms: event.event_time_ms,
                });
            }
        }
        previous_time_ms = Some(event.event_time_ms);
        end_time_ms = event.event_time_ms;

        match &event.payload {
            EventPayload::Quote {
                bid_ticks,
                ask_ticks,
                price_scale: event_scale,
            } => {
                if *event_scale == 0 {
                    return Err(ResearchError::InvalidPriceScale);
                }
                if *bid_ticks <= 0 || *ask_ticks < *bid_ticks {
                    return Err(ResearchError::InvalidQuote);
                }
                check_price_scale(&mut price_scale, *event_scale)?;
                quote_count += 1;

                let spread_numerator = (*ask_ticks as i128 - *bid_ticks as i128) * 20_000;
                let spread_denominator = *ask_ticks as i128 + *bid_ticks as i128;
                let spread_bps =
                    ((spread_numerator + spread_denominator - 1) / spread_denominator) as u64;
                maximum_observed_spread_bps = Some(
                    maximum_observed_spread_bps
                        .map_or(spread_bps, |current: u64| current.max(spread_bps)),
                );
            }
            EventPayload::Trade {
                price_ticks,
                quantity_lots,
                price_scale: event_scale,
            } => {
                if *event_scale == 0 {
                    return Err(ResearchError::InvalidPriceScale);
                }
                if *price_ticks <= 0 || *quantity_lots == 0 {
                    return Err(ResearchError::InvalidTrade);
                }
                check_price_scale(&mut price_scale, *event_scale)?;
                trade_count += 1;
                total_trade_quantity_lots = total_trade_quantity_lots
                    .checked_add(*quantity_lots as u128)
                    .ok_or(ResearchError::TradeQuantityOverflow)?;
                first_trade_price_ticks.get_or_insert(*price_ticks);
                last_trade_price_ticks = Some(*price_ticks);
            }
        }
    }

    let movement_bps = match (first_trade_price_ticks, last_trade_price_ticks) {
        (Some(first_price), Some(last_price)) => {
            Some((last_price as i128 - first_price as i128) * 10_000 / first_price as i128)
        }
        _ => None,
    };

    let metrics = MarketWindowMetrics {
        instrument_id: instrument_id.clone(),
        start_time_ms,
        end_time_ms,
        event_count: events.len(),
        quote_count,
        trade_count,
        total_trade_quantity_lots,
        first_trade_price_ticks,
        last_trade_price_ticks,
        movement_bps,
        maximum_observed_spread_bps,
    };

    let candidate = match (movement_bps, maximum_observed_spread_bps) {
        (Some(movement), Some(spread))
            if trade_count >= config.minimum_trade_count
                && movement.unsigned_abs() >= config.minimum_absolute_move_bps as u128
                && spread <= config.maximum_spread_bps =>
        {
            Some(OpportunityCandidate {
                instrument_id,
                direction: if movement > 0 {
                    PriceDirection::Up
                } else if movement < 0 {
                    PriceDirection::Down
                } else {
                    PriceDirection::Flat
                },
                movement_bps: movement,
                maximum_observed_spread_bps: spread,
                start_time_ms,
                end_time_ms,
                trade_count,
            })
        }
        _ => None,
    };

    Ok(ResearchReport { metrics, candidate })
}

fn check_price_scale(current: &mut Option<u32>, incoming: u32) -> Result<(), ResearchError> {
    match current {
        Some(existing) if *existing != incoming => Err(ResearchError::MixedPriceScales),
        Some(_) => Ok(()),
        None => {
            *current = Some(incoming);
            Ok(())
        }
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
            "source-a",
            "BTC-USD",
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

    fn trade(id: &str, time: i64, price: i64, quantity: u64) -> MarketEvent {
        MarketEvent {
            schema_version: 1,
            event_id: id.to_owned(),
            source_id: "source-a".to_owned(),
            instrument_id: "BTC-USD".to_owned(),
            event_time_ms: time,
            received_time_ms: time,
            sequence: None,
            payload: EventPayload::Trade {
                price_ticks: price,
                quantity_lots: quantity,
                price_scale: SCALE,
            },
        }
    }

    fn config() -> ResearchConfig {
        ResearchConfig {
            minimum_absolute_move_bps: 10,
            maximum_spread_bps: 20,
            minimum_trade_count: 2,
        }
    }

    #[test]
    fn emits_upward_candidate_when_all_explicit_filters_pass() {
        let events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 2),
            trade("t2", 3, 10_100, 3),
            quote("q2", 4, 10_090, 10_110),
        ];

        let report = analyze_market_window(&events, config()).unwrap();
        let candidate = report.candidate.unwrap();

        assert_eq!(candidate.direction, PriceDirection::Up);
        assert_eq!(candidate.movement_bps, 100);
        assert_eq!(candidate.trade_count, 2);
        assert_eq!(report.metrics.total_trade_quantity_lots, 5);
        assert_eq!(report.metrics.quote_count, 2);
    }

    #[test]
    fn emits_downward_candidate_for_negative_movement() {
        let events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 9_900, 1),
        ];

        let report = analyze_market_window(&events, config()).unwrap();
        assert_eq!(report.candidate.unwrap().direction, PriceDirection::Down);
    }

    #[test]
    fn does_not_emit_candidate_below_movement_threshold() {
        let events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_005, 1),
        ];

        let report = analyze_market_window(&events, config()).unwrap();
        assert!(report.candidate.is_none());
    }

    #[test]
    fn does_not_emit_candidate_when_spread_is_too_wide() {
        let events = vec![
            quote("q1", 1, 10_000, 10_100),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_100, 1),
        ];

        let report = analyze_market_window(&events, config()).unwrap();
        assert!(report.candidate.is_none());
    }

    #[test]
    fn does_not_emit_candidate_without_quote_evidence() {
        let events = vec![trade("t1", 1, 10_000, 1), trade("t2", 2, 10_100, 1)];

        let report = analyze_market_window(&events, config()).unwrap();
        assert!(report.candidate.is_none());
        assert_eq!(report.metrics.maximum_observed_spread_bps, None);
    }

    #[test]
    fn reports_metrics_without_candidate_when_there_are_no_trades() {
        let events = vec![quote("q1", 1, 10_000, 10_010)];

        let report = analyze_market_window(&events, config()).unwrap();
        assert_eq!(report.metrics.trade_count, 0);
        assert!(report.metrics.movement_bps.is_none());
        assert!(report.candidate.is_none());
    }

    #[test]
    fn rejects_mixed_instruments() {
        let mut events = vec![quote("q1", 1, 10_000, 10_010)];
        let mut other = trade("t1", 2, 10_100, 1);
        other.instrument_id = "ETH-USD".to_owned();
        events.push(other);

        assert_eq!(
            analyze_market_window(&events, config()),
            Err(ResearchError::MixedInstruments)
        );
    }

    #[test]
    fn rejects_out_of_order_events() {
        let events = vec![quote("q1", 2, 10_000, 10_010), trade("t1", 1, 10_100, 1)];

        assert!(matches!(
            analyze_market_window(&events, config()),
            Err(ResearchError::OutOfOrderEvents { .. })
        ));
    }

    #[test]
    fn rejects_mixed_price_scales() {
        let mut events = vec![quote("q1", 1, 10_000, 10_010)];
        let mut second = trade("t1", 2, 10_100, 1);
        if let EventPayload::Trade { price_scale, .. } = &mut second.payload {
            *price_scale = SCALE + 1;
        }
        events.push(second);

        assert_eq!(
            analyze_market_window(&events, config()),
            Err(ResearchError::MixedPriceScales)
        );
    }

    #[test]
    fn rejects_invalid_quotes_and_trades() {
        let bad_quote = vec![quote("q1", 1, 10_010, 10_000)];
        assert_eq!(
            analyze_market_window(&bad_quote, config()),
            Err(ResearchError::InvalidQuote)
        );

        let mut bad_trade = trade("t1", 1, 10_000, 1);
        if let EventPayload::Trade { quantity_lots, .. } = &mut bad_trade.payload {
            *quantity_lots = 0;
        }
        assert_eq!(
            analyze_market_window(&[bad_trade], config()),
            Err(ResearchError::InvalidTrade)
        );
    }

    #[test]
    fn rounds_spread_up_before_applying_maximum_threshold() {
        let events = vec![
            quote("q1", 1, 10_000, 10_021),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_100, 1),
        ];

        let report = analyze_market_window(&events, config()).unwrap();
        assert_eq!(report.metrics.maximum_observed_spread_bps, Some(21));
        assert!(report.candidate.is_none());
    }

    #[test]
    fn rejects_invalid_research_configuration() {
        let events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_100, 1),
        ];
        let invalid = ResearchConfig {
            minimum_absolute_move_bps: 0,
            ..config()
        };

        assert_eq!(
            analyze_market_window(&events, invalid),
            Err(ResearchError::InvalidConfig)
        );
    }

    #[test]
    fn rejects_unsupported_event_schema_version() {
        let mut events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_100, 1),
        ];
        events[1].schema_version = 2;

        assert_eq!(
            analyze_market_window(&events, config()),
            Err(ResearchError::UnsupportedSchema(2))
        );
    }

    #[test]
    fn rejects_mixed_sources_in_one_market_window() {
        let mut events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_100, 1),
        ];
        events[2].source_id = "mexc-futures-rest".to_owned();

        assert_eq!(
            analyze_market_window(&events, config()),
            Err(ResearchError::MixedSources)
        );
    }

    #[test]
    fn rejects_empty_source_id() {
        let mut events = vec![
            quote("q1", 1, 10_000, 10_010),
            trade("t1", 2, 10_000, 1),
            trade("t2", 3, 10_100, 1),
        ];
        events[0].source_id.clear();

        assert_eq!(
            analyze_market_window(&events, config()),
            Err(ResearchError::EmptySourceId)
        );
    }

    #[test]
    fn rejects_empty_window() {
        assert_eq!(
            analyze_market_window(&[], config()),
            Err(ResearchError::EmptyWindow)
        );
    }
}
