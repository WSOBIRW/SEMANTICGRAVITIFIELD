use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::io::BufReader;
use std::process;

use serde_json::{json, Value};
use semantic_gravity_field::{
    analyze_market_window, evaluate_round_trip, EconomicConfig, MarketEvent,
    MexcCaptureReader, PriceDirection, ResearchConfig,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Ошибка исследования: {error}");
        process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() != 2 || args[0] == "--help" || args[0] == "-h" {
        return Err(
            "Использование: sgf-research <capture.jsonl> <experiment.json>\n\
             Файл эксперимента должен явно задавать окно, пороги, комиссии, \
             проскальзывание и funding."
                .to_owned(),
        );
    }

    let config_bytes = std::fs::read(&args[1])
        .map_err(|error| format!("не удалось прочитать конфигурацию {}: {error}", args[1]))?;
    let config_json: Value = serde_json::from_slice(&config_bytes)
        .map_err(|error| format!("некорректный JSON конфигурации: {error}"))?;
    let window_ms = required_u64(&config_json, "window_ms")?;
    if window_ms == 0 {
        return Err("window_ms должен быть больше нуля".to_owned());
    }
    let research = ResearchConfig {
        minimum_absolute_move_bps: required_u64(&config_json, "minimum_absolute_move_bps")?,
        maximum_spread_bps: required_u64(&config_json, "maximum_spread_bps")?,
        minimum_trade_count: usize::try_from(required_u64(&config_json, "minimum_trade_count")?)
            .map_err(|_| "minimum_trade_count выходит за допустимый диапазон".to_owned())?,
    };
    let economic_json = config_json
        .get("economic")
        .ok_or_else(|| "отсутствует объект economic".to_owned())?;
    let economic = EconomicConfig {
        entry_fee_bps: required_u64(economic_json, "entry_fee_bps")?,
        exit_fee_bps: required_u64(economic_json, "exit_fee_bps")?,
        entry_slippage_bps: required_u64(economic_json, "entry_slippage_bps")?,
        exit_slippage_bps: required_u64(economic_json, "exit_slippage_bps")?,
        funding_cost_bps: required_i64(economic_json, "funding_cost_bps")?,
    };

    let input = File::open(&args[0])
        .map_err(|error| format!("не удалось открыть capture {}: {error}", args[0]))?;
    let observations = MexcCaptureReader::new(BufReader::new(input))
        .read_sorted()
        .map_err(|error| format!("capture не прошёл проверку: {error:?}"))?;
    if observations.is_empty() {
        return Err("capture не содержит исследуемых рыночных событий".to_owned());
    }

    let mut sources: BTreeMap<String, Vec<MarketEvent>> = BTreeMap::new();
    for observation in observations {
        sources
            .entry(observation.event.source_id.clone())
            .or_default()
            .push(observation.event);
    }

    let mut windows_scanned = 0usize;
    let mut candidate_reports = Vec::new();
    let mut positive_count = 0usize;
    let mut negative_count = 0usize;
    let mut break_even_count = 0usize;
    let window_ms_i64 =
        i64::try_from(window_ms).map_err(|_| "window_ms слишком велик".to_owned())?;

    for (source_id, events) in sources {
        let origin = events
            .first()
            .ok_or_else(|| "пустая группа источника".to_owned())?
            .event_time_ms;
        let mut windows: BTreeMap<u64, Vec<MarketEvent>> = BTreeMap::new();
        for event in events {
            let elapsed = event
                .event_time_ms
                .checked_sub(origin)
                .ok_or_else(|| "переполнение временного интервала".to_owned())?;
            if elapsed < 0 {
                return Err("события источника не отсортированы по времени".to_owned());
            }
            let index = u64::try_from(elapsed)
                .map_err(|_| "временной интервал выходит за допустимый диапазон".to_owned())?
                / window_ms;
            windows.entry(index).or_default().push(event);
        }

        for (index, window_events) in windows {
            windows_scanned += 1;
            let report = analyze_market_window(&window_events, research)
                .map_err(|error| format!("ошибка анализа окна: {error:?}"))?;
            let Some(candidate) = report.candidate else {
                continue;
            };
            let assessment = evaluate_round_trip(&window_events, &candidate, economic);
            let (economic_result, economic_error) = match assessment {
                Ok(value) => {
                    match value.verdict {
                        semantic_gravity_field::EconomicVerdict::Positive => positive_count += 1,
                        semantic_gravity_field::EconomicVerdict::Negative => negative_count += 1,
                        semantic_gravity_field::EconomicVerdict::BreakEven => break_even_count += 1,
                    }
                    (
                        Some(json!({
                            "gross_return_bps": value.gross_return_bps,
                            "total_cost_bps": value.total_cost_bps,
                            "net_return_bps": value.net_return_bps,
                            "verdict": format!("{:?}", value.verdict),
                            "entry_time_ms": value.entry_time_ms,
                            "exit_time_ms": value.exit_time_ms,
                            "entry_price_ticks": value.entry_price_ticks,
                            "exit_price_ticks": value.exit_price_ticks
                        })),
                        None,
                    )
                }
                Err(error) => (None, Some(format!("{error:?}"))),
            };
            let window_start_ms = origin
                .checked_add(
                    i64::try_from(index)
                        .map_err(|_| "номер окна выходит за допустимый диапазон".to_owned())?
                        .checked_mul(window_ms_i64)
                        .ok_or_else(|| "переполнение начала окна".to_owned())?,
                )
                .ok_or_else(|| "переполнение начала окна".to_owned())?;
            let direction = match candidate.direction {
                PriceDirection::Up => "UP",
                PriceDirection::Down => "DOWN",
                PriceDirection::Flat => "FLAT",
            };
            let _ = &report.metrics;
            candidate_reports.push(json!({
                "status": "PRELIMINARY_CANDIDATE",
                "source_id": source_id,
                "instrument_id": candidate.instrument_id,
                "direction": direction,
                "window_start_ms": window_start_ms,
                "window_end_ms": candidate.end_time_ms,
                "movement_bps": candidate.movement_bps,
                "maximum_spread_bps": candidate.maximum_observed_spread_bps,
                "trade_count": candidate.trade_count,
                "economic_assessment": economic_result,
                "economic_error": economic_error
            }));
        }
    }

    let output = json!({
        "status": "PRELIMINARY_RESEARCH_ONLY",
        "input": args[0],
        "configuration": config_json,
        "windows_scanned": windows_scanned,
        "candidates_found": candidate_reports.len(),
        "economic_positive": positive_count,
        "economic_negative": negative_count,
        "economic_break_even": break_even_count,
        "candidates": candidate_reports,
        "limitations": [
            "Оценка использует только исторический top-of-book.",
            "Результат не моделирует очередь, глубину для размера позиции и задержку исполнения.",
            "Funding и торговые издержки берутся только из явно заданной конфигурации.",
            "Положительный результат одного окна не является независимым подтверждением преимущества."
        ]
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&output)
            .map_err(|error| format!("не удалось сформировать отчёт: {error}"))?
    );
    Ok(())
}

fn required_u64(value: &Value, key: &str) -> Result<u64, String> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("поле {key} должно быть целым неотрицательным числом"))
}

fn required_i64(value: &Value, key: &str) -> Result<i64, String> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("поле {key} должно быть целым числом"))
}
