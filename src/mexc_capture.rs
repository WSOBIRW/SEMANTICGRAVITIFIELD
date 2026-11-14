use std::collections::{BTreeMap, VecDeque};
use std::io::BufRead;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::event::{EventPayload, MarketEvent, Quote};
use crate::mexc::{
    parse_scaled_i64, parse_scaled_u64, MexcContractSpec, MexcObservation, MexcObservationMetadata,
    MexcPositionAction, MexcSelfTradeFlag, MexcTradeSide,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MexcCaptureError {
    Io,
    InvalidJson,
    MissingField,
    UnsupportedCaptureSchema(u64),
    UnsupportedVenue,
    UnsupportedMarket,
    CaptureMarkedNonContiguous,
    MissingContractSpec,
    MissingDepthSnapshot,
    SymbolMismatch,
    InvalidTimestamp,
    InvalidContractValue,
    InvalidDepthSnapshot,
    InvalidDepthDelta,
    DepthSequenceGap { expected: u64, actual: u64 },
    EmptyBook,
    CrossedBook,
    DuplicateLevel,
    InvalidTrade,
    InvalidTradeMetadata,
    InvalidLimits,
    LineTooLarge,
    InvalidUtf8,
    TooManyLevels,
    TooManyTrades,
    CaptureAlreadyActive,
    CaptureNotActive,
    MissingCaptureEnd,
    DepthSnapshotVersionRewind { current: u64, actual: u64 },
}

/// Настраиваемые бюджеты чтения недоверенного capture JSONL.
///
/// Значения по умолчанию допускают крупные кадры и глубокие книги.
/// Ограничения можно увеличить для конкретного источника.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MexcCaptureLimits {
    pub max_line_bytes: usize,
    pub max_levels_per_side: usize,
    pub max_trades_per_frame: usize,
}

impl Default for MexcCaptureLimits {
    fn default() -> Self {
        Self {
            max_line_bytes: 16 * 1024 * 1024,
            max_levels_per_side: 100_000,
            max_trades_per_frame: 100_000,
        }
    }
}

#[derive(Default)]
struct DepthBook {
    bids: BTreeMap<i64, u64>,
    asks: BTreeMap<i64, u64>,
    version: Option<u64>,
}

/// Читает JSONL-файл, создаваемый диагностическим сборщиком MEXC Futures
/// в TRADINGCORE, и выдаёт нормализованные SGF observations.
///
/// Читатель не открывает сеть и не изменяет исходный файл. Сырой кадр остаётся
/// в capture JSONL; каждое нормализованное событие получает SHA-256 этого кадра.
/// События выдаются в порядке получения. Для исторического анализа доступен
/// read_sorted(), который сортирует конечный capture по exchange event time.
pub struct MexcCaptureReader<R: BufRead> {
    reader: R,
    line: Vec<u8>,
    limits: MexcCaptureLimits,
    spec: Option<MexcContractSpec>,
    symbol: Option<String>,
    capture_id: Option<i64>,
    capture_active: bool,
    completed_capture: bool,
    book: DepthBook,
    pending: VecDeque<MexcObservation>,
    failed: bool,
}

impl<R: BufRead> MexcCaptureReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            line: Vec::new(),
            limits: MexcCaptureLimits::default(),
            spec: None,
            symbol: None,
            capture_id: None,
            capture_active: false,
            completed_capture: false,
            book: DepthBook::default(),
            pending: VecDeque::new(),
            failed: false,
        }
    }

    pub fn with_limits(reader: R, limits: MexcCaptureLimits) -> Result<Self, MexcCaptureError> {
        if limits.max_line_bytes == 0
            || limits.max_levels_per_side == 0
            || limits.max_trades_per_frame == 0
        {
            return Err(MexcCaptureError::InvalidLimits);
        }
        Ok(Self {
            reader,
            line: Vec::new(),
            limits,
            spec: None,
            symbol: None,
            capture_id: None,
            capture_active: false,
            completed_capture: false,
            book: DepthBook::default(),
            pending: VecDeque::new(),
            failed: false,
        })
    }

    /// Собирает конечный capture в память и сортирует его по exchange event time.
    /// Для длинных файлов используйте Iterator напрямую, чтобы обрабатывать события потоково.
    pub fn read_sorted(mut self) -> Result<Vec<MexcObservation>, MexcCaptureError> {
        let mut observations = Vec::new();
        for item in self.by_ref() {
            observations.push(item?);
        }
        if self.capture_active {
            return Err(MexcCaptureError::MissingCaptureEnd);
        }
        if !self.completed_capture {
            return Err(MexcCaptureError::MissingCaptureEnd);
        }
        observations.sort_by_key(|item| (item.event.event_time_ms, item.event.received_time_ms));
        Ok(observations)
    }

    fn process_line(&mut self, line: &str) -> Result<(), MexcCaptureError> {
        let record: Value =
            serde_json::from_str(line).map_err(|_| MexcCaptureError::InvalidJson)?;
        let record_type = string_field(&record, "record_type")?;

        match record_type {
            "capture_start" => {
                self.check_schema(&record)?;
                self.process_capture_start(&record)
            }
            "depth_snapshot" => {
                self.check_schema(&record)?;
                self.process_depth_snapshot(&record)
            }
            "depth_delta" => {
                self.check_schema(&record)?;
                self.process_depth_delta(&record)
            }
            "deal_frame" => {
                self.check_schema(&record)?;
                self.process_deal_frame(&record)
            }
            "capture_end" => self.process_capture_end(&record),
            // Метаданные, heartbeat и reference snapshots не превращаются
            // в Quote/Trade, но остаются в исходном JSONL.
            _ => Ok(()),
        }
    }

    fn check_schema(&self, record: &Value) -> Result<(), MexcCaptureError> {
        // Capture schema is declared by capture_start. Older producer records
        // do not repeat schema_version on every JSONL line.
        if let Some(value) = record.get("schema_version") {
            let schema = value.as_u64().ok_or(MexcCaptureError::MissingField)?;
            if schema != 1 {
                return Err(MexcCaptureError::UnsupportedCaptureSchema(schema));
            }
        }
        Ok(())
    }

    fn process_capture_end(&mut self, record: &Value) -> Result<(), MexcCaptureError> {
        if !self.capture_active {
            return Err(MexcCaptureError::CaptureNotActive);
        }
        if record
            .get("depth_stream_contiguous")
            .and_then(Value::as_bool)
            != Some(true)
            || optional_u64(record.get("depth_sequence_gaps")).unwrap_or(0) > 0
        {
            return Err(MexcCaptureError::CaptureMarkedNonContiguous);
        }
        if record.get("private_api_used").and_then(Value::as_bool) == Some(true)
            || record.get("orders_submitted").and_then(Value::as_bool) == Some(true)
        {
            return Err(MexcCaptureError::InvalidContractValue);
        }
        self.capture_active = false;
        self.completed_capture = true;
        Ok(())
    }

    fn process_capture_start(&mut self, record: &Value) -> Result<(), MexcCaptureError> {
        if self.capture_active {
            return Err(MexcCaptureError::CaptureAlreadyActive);
        }
        if string_field(record, "venue")? != "MEXC" {
            return Err(MexcCaptureError::UnsupportedVenue);
        }
        if string_field(record, "market")? != "FUTURES_PERPETUAL" {
            return Err(MexcCaptureError::UnsupportedMarket);
        }
        if record.get("private_api_used").and_then(Value::as_bool) == Some(true)
            || record.get("orders_submitted").and_then(Value::as_bool) == Some(true)
        {
            return Err(MexcCaptureError::InvalidContractValue);
        }

        let symbol = string_field(record, "symbol")?.to_owned();
        let price_tick = string_field(record, "price_tick")?;
        let quantity_step = string_field(record, "quantity_step")?;
        let contract_size = string_field(record, "contract_size")?;

        let spec =
            MexcContractSpec::from_steps(symbol.clone(), price_tick, quantity_step, contract_size)
                .map_err(|_| MexcCaptureError::InvalidContractValue)?;

        let capture_id = i64_field(record, "initialized_at_ms")?;
        if capture_id < 0 {
            return Err(MexcCaptureError::InvalidTimestamp);
        }
        self.capture_id = Some(capture_id);
        self.capture_active = true;
        self.completed_capture = false;
        self.symbol = Some(symbol);
        self.spec = Some(spec);
        self.book = DepthBook::default();
        self.pending.clear();
        Ok(())
    }

    fn process_depth_snapshot(&mut self, record: &Value) -> Result<(), MexcCaptureError> {
        if !self.capture_active {
            return Err(MexcCaptureError::CaptureNotActive);
        }
        let spec = self
            .spec
            .as_ref()
            .ok_or(MexcCaptureError::MissingContractSpec)?;
        let symbol = string_field(record, "symbol")?;
        self.check_symbol(symbol)?;

        let captured_at_ms = i64_field(record, "captured_at_ms")?;
        let server_time_ms = optional_i64(record.get("server_time_ms"));
        let raw_payload = string_field(record, "raw_payload")?;
        let raw: Value = serde_json::from_str(raw_payload)
            .map_err(|_| MexcCaptureError::InvalidDepthSnapshot)?;
        let data = raw
            .get("data")
            .and_then(Value::as_object)
            .ok_or(MexcCaptureError::InvalidDepthSnapshot)?;
        let raw_version = u64_field(data.get("version"))?;
        let record_version = u64_field(record.get("version"))?;
        if raw_version != record_version {
            return Err(MexcCaptureError::InvalidDepthSnapshot);
        }
        if let Some(current) = self.book.version {
            if record_version < current {
                return Err(MexcCaptureError::DepthSnapshotVersionRewind {
                    current,
                    actual: record_version,
                });
            }
        }

        let bids = parse_snapshot_levels(
            data.get("bids").ok_or(MexcCaptureError::MissingField)?,
            spec,
            self.limits.max_levels_per_side,
        )?;
        let asks = parse_snapshot_levels(
            data.get("asks").ok_or(MexcCaptureError::MissingField)?,
            spec,
            self.limits.max_levels_per_side,
        )?;
        self.book = DepthBook {
            bids,
            asks,
            version: Some(record_version),
        };
        let (bid, ask) = best_prices(&self.book)?;
        let event_time_ms = server_time_ms.unwrap_or(captured_at_ms);
        if event_time_ms < 0 || captured_at_ms < 0 {
            return Err(MexcCaptureError::InvalidTimestamp);
        }

        self.pending.push_back(make_quote_observation(
            spec,
            DepthQuoteFrame {
                capture_id: self
                    .capture_id
                    .ok_or(MexcCaptureError::MissingContractSpec)?,
                symbol,
                version: record_version,
                bid_ticks: bid,
                ask_ticks: ask,
                event_time_ms,
                received_time_ms: captured_at_ms,
                raw_payload,
            },
        ));
        Ok(())
    }

    fn process_depth_delta(&mut self, record: &Value) -> Result<(), MexcCaptureError> {
        if !self.capture_active {
            return Err(MexcCaptureError::CaptureNotActive);
        }
        let spec = self
            .spec
            .as_ref()
            .ok_or(MexcCaptureError::MissingContractSpec)?;
        let symbol = string_field(record, "symbol")?;
        self.check_symbol(symbol)?;

        let begin = u64_field(record.get("begin"))?;
        let end = u64_field(record.get("end"))?;
        let version = u64_field(record.get("version"))?;
        if begin > end || end != version {
            return Err(MexcCaptureError::InvalidDepthDelta);
        }
        let expected = self
            .book
            .version
            .and_then(|value| value.checked_add(1))
            .ok_or(MexcCaptureError::MissingDepthSnapshot)?;
        if begin != expected
            || record
                .get("sequence_contiguous_from_previous")
                .and_then(Value::as_bool)
                == Some(false)
        {
            return Err(MexcCaptureError::DepthSequenceGap {
                expected,
                actual: begin,
            });
        }

        let captured_at_ms = i64_field(record, "captured_at_ms")?;
        let matching_engine_time_ms = optional_i64(record.get("matching_engine_time_ms"));
        let server_time_ms = optional_i64(record.get("server_time_ms"));
        let raw_payload = string_field(record, "raw_payload")?;

        apply_delta_levels(
            &mut self.book.bids,
            record.get("bids").ok_or(MexcCaptureError::MissingField)?,
            spec,
            self.limits.max_levels_per_side,
        )?;
        apply_delta_levels(
            &mut self.book.asks,
            record.get("asks").ok_or(MexcCaptureError::MissingField)?,
            spec,
            self.limits.max_levels_per_side,
        )?;
        self.book.version = Some(version);

        let (bid, ask) = best_prices(&self.book)?;
        let event_time_ms = matching_engine_time_ms
            .or(server_time_ms)
            .unwrap_or(captured_at_ms);
        if event_time_ms < 0 || captured_at_ms < 0 {
            return Err(MexcCaptureError::InvalidTimestamp);
        }

        self.pending.push_back(make_quote_observation(
            spec,
            DepthQuoteFrame {
                capture_id: self
                    .capture_id
                    .ok_or(MexcCaptureError::MissingContractSpec)?,
                symbol,
                version,
                bid_ticks: bid,
                ask_ticks: ask,
                event_time_ms,
                received_time_ms: captured_at_ms,
                raw_payload,
            },
        ));
        Ok(())
    }

    fn process_deal_frame(&mut self, record: &Value) -> Result<(), MexcCaptureError> {
        if !self.capture_active {
            return Err(MexcCaptureError::CaptureNotActive);
        }
        let spec = self
            .spec
            .as_ref()
            .ok_or(MexcCaptureError::MissingContractSpec)?;
        let symbol = string_field(record, "symbol")?;
        self.check_symbol(symbol)?;
        let captured_at_ms = i64_field(record, "captured_at_ms")?;
        if captured_at_ms < 0 {
            return Err(MexcCaptureError::InvalidTimestamp);
        }
        let raw_payload = string_field(record, "raw_payload")?;
        let raw_hash = payload_hash(raw_payload);
        let trades = record
            .get("trades")
            .and_then(Value::as_array)
            .ok_or(MexcCaptureError::MissingField)?;
        if trades.len() > self.limits.max_trades_per_frame {
            return Err(MexcCaptureError::TooManyTrades);
        }
        if optional_u64(record.get("trade_count")).is_some_and(|count| count != trades.len() as u64)
        {
            return Err(MexcCaptureError::InvalidTrade);
        }

        for (event_index, trade) in trades.iter().enumerate() {
            let price_ticks = i64_field(trade, "price_ticks")?;
            let quantity_lots = u64_field(trade.get("quantity_lots"))?;
            let transaction_id = optional_string_field(trade, "transaction_id")
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned);
            let trade_time_ms = i64_field(trade, "trade_time_ms")?;
            let matching_engine_time_ms = optional_i64(trade.get("matching_engine_time_ms"));

            if price_ticks <= 0
                || quantity_lots == 0
                || price_ticks % spec.price_unit_ticks != 0
                || quantity_lots % spec.volume_unit_lots != 0
                || trade_time_ms < 0
                || matching_engine_time_ms.is_some_and(|time| time < 0)
            {
                return Err(MexcCaptureError::InvalidTrade);
            }

            let trade_side = parse_trade_side(string_field(trade, "side")?)?;
            let position_action = parse_position_action(string_field(trade, "position_effect")?)?;
            let self_trade = parse_self_trade(string_field(trade, "self_trade")?)?;

            let local_event_index = if transaction_id.is_none() {
                Some(u32::try_from(event_index).map_err(|_| MexcCaptureError::InvalidTrade)?)
            } else {
                None
            };
            let event_id = match transaction_id.as_deref() {
                Some(id) => format!("mexc:{}:deal:{}", spec.symbol, id),
                None => format!(
                    "mexc:{}:deal:local:{}:{}:{}",
                    spec.symbol,
                    trade_time_ms,
                    local_event_index.ok_or(MexcCaptureError::InvalidTrade)?,
                    hex_payload_hash(&raw_hash)
                ),
            };
            let event = MarketEvent {
                schema_version: 1,
                event_id,
                source_id: format!(
                    "mexc-futures:{}",
                    self.capture_id
                        .ok_or(MexcCaptureError::MissingContractSpec)?
                ),
                instrument_id: spec.symbol.clone(),
                event_time_ms: matching_engine_time_ms.unwrap_or(trade_time_ms),
                received_time_ms: captured_at_ms,
                sequence: None,
                payload: EventPayload::Trade {
                    price_ticks,
                    quantity_lots,
                    price_scale: spec.price_scale_factor,
                },
            };

            self.pending.push_back(MexcObservation {
                event,
                metadata: MexcObservationMetadata::Trade {
                    trade_side,
                    position_action,
                    self_trade,
                    transaction_id,
                    local_event_index,
                    trade_time_ms,
                    matching_engine_time_ms,
                },
                raw_payload_sha256: raw_hash,
                volume_scale_factor: spec.volume_scale_factor,
            });
        }
        Ok(())
    }

    fn check_symbol(&self, symbol: &str) -> Result<(), MexcCaptureError> {
        if self.symbol.as_deref() != Some(symbol) {
            return Err(MexcCaptureError::SymbolMismatch);
        }
        Ok(())
    }
}

impl<R: BufRead> Iterator for MexcCaptureReader<R> {
    type Item = Result<MexcObservation, MexcCaptureError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        loop {
            if let Some(item) = self.pending.pop_front() {
                return Some(Ok(item));
            }
            match read_line_bounded(&mut self.reader, &mut self.line, self.limits.max_line_bytes) {
                Ok(0) => {
                    if self.capture_active {
                        self.failed = true;
                        return Some(Err(MexcCaptureError::MissingCaptureEnd));
                    }
                    return None;
                },
                Ok(_) => {}
                Err(error) => {
                    self.failed = true;
                    return Some(Err(error));
                }
            }
            let bytes = std::mem::take(&mut self.line);
            let line = match String::from_utf8(bytes) {
                Ok(line) => line,
                Err(error) => {
                    self.line = error.into_bytes();
                    self.failed = true;
                    return Some(Err(MexcCaptureError::InvalidUtf8));
                }
            };
            if line.trim().is_empty() {
                self.line = line.into_bytes();
                continue;
            }
            let result = self.process_line(&line);
            self.line = line.into_bytes();
            if let Err(error) = result {
                self.failed = true;
                return Some(Err(error));
            }
        }
    }
}

fn read_line_bounded<R: BufRead>(
    reader: &mut R,
    output: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<usize, MexcCaptureError> {
    output.clear();
    loop {
        let (consumed, found_newline) = {
            let available = reader.fill_buf().map_err(|_| MexcCaptureError::Io)?;
            if available.is_empty() {
                return Ok(output.len());
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            if output.len().saturating_add(count) > max_bytes {
                return Err(MexcCaptureError::LineTooLarge);
            }
            output.extend_from_slice(&available[..count]);
            (count, newline.is_some())
        };
        reader.consume(consumed);
        if found_newline {
            return Ok(output.len());
        }
    }
}

struct DepthQuoteFrame<'a> {
    capture_id: i64,
    symbol: &'a str,
    version: u64,
    bid_ticks: i64,
    ask_ticks: i64,
    event_time_ms: i64,
    received_time_ms: i64,
    raw_payload: &'a str,
}

fn make_quote_observation(spec: &MexcContractSpec, frame: DepthQuoteFrame<'_>) -> MexcObservation {
    let event = MarketEvent::quote(
        format!(
            "mexc:{}:depth:{}:{}",
            frame.symbol, frame.capture_id, frame.version
        ),
        format!("mexc-futures:{}", frame.capture_id),
        spec.symbol.clone(),
        frame.event_time_ms,
        frame.received_time_ms,
        None,
        Quote {
            bid_ticks: frame.bid_ticks,
            ask_ticks: frame.ask_ticks,
            price_scale: spec.price_scale_factor,
        },
    );
    MexcObservation {
        event,
        metadata: MexcObservationMetadata::Quote {
            last_price_ticks: None,
        },
        raw_payload_sha256: payload_hash(frame.raw_payload),
        volume_scale_factor: spec.volume_scale_factor,
    }
}

fn parse_snapshot_levels(
    value: &Value,
    spec: &MexcContractSpec,
    max_levels: usize,
) -> Result<BTreeMap<i64, u64>, MexcCaptureError> {
    let levels = value
        .as_array()
        .ok_or(MexcCaptureError::InvalidDepthSnapshot)?;
    if levels.len() > max_levels {
        return Err(MexcCaptureError::TooManyLevels);
    }
    let mut result = BTreeMap::new();
    for level in levels {
        let fields = level
            .as_array()
            .ok_or(MexcCaptureError::InvalidDepthSnapshot)?;
        let quantity_index = match fields.len() {
            2 | 3 => 1,
            _ => return Err(MexcCaptureError::InvalidDepthSnapshot),
        };
        let price = decimal_string(&fields[0])?;
        let quantity = decimal_string(&fields[quantity_index])?;
        let price_ticks = parse_scaled_i64(&price, spec.price_scale_factor)
            .map_err(|_| MexcCaptureError::InvalidDepthSnapshot)?;
        let quantity_lots = parse_scaled_u64(&quantity, spec.volume_scale_factor)
            .map_err(|_| MexcCaptureError::InvalidDepthSnapshot)?;
        if price_ticks <= 0
            || quantity_lots == 0
            || price_ticks % spec.price_unit_ticks != 0
            || quantity_lots % spec.volume_unit_lots != 0
        {
            return Err(MexcCaptureError::InvalidDepthSnapshot);
        }
        if result.insert(price_ticks, quantity_lots).is_some() {
            return Err(MexcCaptureError::DuplicateLevel);
        }
    }
    Ok(result)
}

fn apply_delta_levels(
    book_side: &mut BTreeMap<i64, u64>,
    value: &Value,
    spec: &MexcContractSpec,
    max_levels: usize,
) -> Result<(), MexcCaptureError> {
    let levels = value
        .as_array()
        .ok_or(MexcCaptureError::InvalidDepthDelta)?;
    for level in levels {
        let price = i64_field(level, "price_ticks")?;
        let quantity = u64_field(level.get("quantity_lots"))?;
        if price <= 0 || price % spec.price_unit_ticks != 0 || quantity % spec.volume_unit_lots != 0
        {
            return Err(MexcCaptureError::InvalidDepthDelta);
        }
        if quantity == 0 {
            book_side.remove(&price);
        } else {
            book_side.insert(price, quantity);
        }
        if book_side.len() > max_levels {
            return Err(MexcCaptureError::TooManyLevels);
        }
    }
    Ok(())
}

fn best_prices(book: &DepthBook) -> Result<(i64, i64), MexcCaptureError> {
    let bid = book.bids.last_key_value().map(|(price, _)| *price);
    let ask = book.asks.first_key_value().map(|(price, _)| *price);
    match (bid, ask) {
        (Some(bid), Some(ask)) if bid <= ask => Ok((bid, ask)),
        (Some(_), Some(_)) => Err(MexcCaptureError::CrossedBook),
        _ => Err(MexcCaptureError::EmptyBook),
    }
}

fn parse_trade_side(value: &str) -> Result<MexcTradeSide, MexcCaptureError> {
    match value {
        "Buy" => Ok(MexcTradeSide::Buy),
        "Sell" => Ok(MexcTradeSide::Sell),
        _ => parse_other(value).map(MexcTradeSide::Other),
    }
}

fn parse_position_action(value: &str) -> Result<MexcPositionAction, MexcCaptureError> {
    match value {
        "Open" => Ok(MexcPositionAction::Open),
        "Reduce" => Ok(MexcPositionAction::Reduce),
        "Unchanged" => Ok(MexcPositionAction::Unchanged),
        _ => parse_other(value).map(MexcPositionAction::Other),
    }
}

fn parse_self_trade(value: &str) -> Result<MexcSelfTradeFlag, MexcCaptureError> {
    match value {
        "Yes" => Ok(MexcSelfTradeFlag::Yes),
        "No" => Ok(MexcSelfTradeFlag::No),
        _ => parse_other(value).map(MexcSelfTradeFlag::Other),
    }
}

fn parse_other(value: &str) -> Result<u64, MexcCaptureError> {
    value
        .strip_prefix("Other(")
        .and_then(|value| value.strip_suffix(')'))
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(MexcCaptureError::InvalidTradeMetadata)
}

fn optional_string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn string_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, MexcCaptureError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(MexcCaptureError::MissingField)
}

fn i64_field(value: &Value, key: &str) -> Result<i64, MexcCaptureError> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|v| i64::try_from(v).ok())
        })
        .ok_or(MexcCaptureError::MissingField)
}

fn u64_field(value: Option<&Value>) -> Result<u64, MexcCaptureError> {
    value
        .and_then(Value::as_u64)
        .ok_or(MexcCaptureError::MissingField)
}

fn optional_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
    })
}

fn optional_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64)
}

fn decimal_string(value: &Value) -> Result<String, MexcCaptureError> {
    match value {
        Value::Number(number) => Ok(number.to_string()),
        Value::String(string) => Ok(string.clone()),
        _ => Err(MexcCaptureError::InvalidDepthSnapshot),
    }
}

fn payload_hash(payload: &str) -> [u8; 32] {
    Sha256::digest(payload.as_bytes()).into()
}

fn hex_payload_hash(digest: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn capture_lines(sequence_begin: u64) -> String {
        let start = json!({
            "record_type": "capture_start",
            "schema_version": 1,
            "venue": "MEXC",
            "market": "FUTURES_PERPETUAL",
            "symbol": "BTC_USDT",
            "initialized_at_ms": 900,
            "price_tick": "0.01",
            "quantity_step": "0.01",
            "contract_size": "0.001",
            "private_api_used": false,
            "orders_submitted": false
        });
        let snapshot_raw = r#"{"success":true,"data":{"version":10,"asks":[[100.10,1.00,7]],"bids":[[100.00,1.00,9]]},"ts":1000}"#;
        let snapshot = json!({
            "record_type": "depth_snapshot",
            "captured_at_ms": 1001,
            "symbol": "BTC_USDT",
            "version": 10,
            "server_time_ms": 1000,
            "raw_payload": snapshot_raw
        });
        let deal_raw = r#"{"channel":"push.deal","symbol":"BTC_USDT"}"#;
        let deals = json!({
            "record_type": "deal_frame",
            "captured_at_ms": 1006,
            "symbol": "BTC_USDT",
            "raw_payload": deal_raw,
            "trade_count": 1,
            "trades": [{
                "price_ticks": 10005,
                "quantity_lots": 50,
                "side": "Buy",
                "position_effect": "Open",
                "self_trade": "No",
                "transaction_id": "tx-1",
                "trade_time_ms": 1005,
                "matching_engine_time_ms": 1004
            }]
        });
        let delta_raw = r#"{"channel":"push.depth","symbol":"BTC_USDT"}"#;
        let delta = json!({
            "record_type": "depth_delta",
            "captured_at_ms": 1011,
            "symbol": "BTC_USDT",
            "begin": sequence_begin,
            "end": sequence_begin,
            "version": sequence_begin,
            "matching_engine_time_ms": 1009,
            "server_time_ms": 1010,
            "sequence_contiguous_from_previous": sequence_begin == 11,
            "bids": [
                {"price_ticks": 10000, "quantity_lots": 0},
                {"price_ticks": 10005, "quantity_lots": 50}
            ],
            "asks": [{"price_ticks": 10010, "quantity_lots": 80}],
            "raw_payload": delta_raw
        });

        let end = json!({
            "record_type": "capture_end",
            "depth_sequence_gaps": 0,
            "depth_stream_contiguous": true,
            "private_api_used": false,
            "orders_submitted": false
        });
        [
            start.to_string(),
            snapshot.to_string(),
            deals.to_string(),
            delta.to_string(),
            end.to_string(),
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn rejects_delta_book_growth_over_configured_level_budget() {
        let spec = MexcContractSpec::from_steps("BTC_USDT", "0.01", "0.01", "0.001").unwrap();
        let mut side = BTreeMap::new();
        let updates = json!([
            {"price_ticks": 10_000, "quantity_lots": 100},
            {"price_ticks": 10_001, "quantity_lots": 100}
        ]);
        assert_eq!(
            apply_delta_levels(&mut side, &updates, &spec, 1),
            Err(MexcCaptureError::TooManyLevels)
        );
    }

    #[test]
    fn rejects_trade_frames_over_configured_trade_budget() {
        let spec = MexcContractSpec::from_steps("BTC_USDT", "0.01", "0.01", "0.001").unwrap();
        let mut reader = MexcCaptureReader::new(Cursor::new(""));
        reader.symbol = Some(spec.symbol.clone());
        reader.capture_id = Some(900);
        reader.spec = Some(spec);
        reader.limits.max_trades_per_frame = 1;
        let trade = json!({
            "price_ticks": 10_000,
            "quantity_lots": 100,
            "side": "Buy",
            "position_effect": "Open",
            "self_trade": "No",
            "transaction_id": "tx-1",
            "trade_time_ms": 1_000,
            "matching_engine_time_ms": 1_000
        });
        let record = json!({
            "symbol": "BTC_USDT",
            "captured_at_ms": 1_001,
            "raw_payload": "{}",
            "trades": [trade.clone(), trade]
        });
        assert_eq!(
            reader.process_deal_frame(&record),
            Err(MexcCaptureError::TooManyTrades)
        );
    }

    #[test]
    fn rejects_capture_lines_over_configured_byte_budget() {
        let limits = MexcCaptureLimits {
            max_line_bytes: 8,
            ..MexcCaptureLimits::default()
        };
        let mut reader =
            MexcCaptureReader::with_limits(Cursor::new(capture_lines(11)), limits).unwrap();
        assert_eq!(reader.next(), Some(Err(MexcCaptureError::LineTooLarge)));
    }

    #[test]
    fn rejects_depth_snapshots_over_configured_level_budget() {
        let spec = MexcContractSpec::from_steps("BTC_USDT", "0.01", "0.01", "0.001").unwrap();
        let levels = json!([["100.00", "1.00"], ["99.99", "1.00"]]);
        assert_eq!(
            parse_snapshot_levels(&levels, &spec, 1),
            Err(MexcCaptureError::TooManyLevels)
        );
    }

    #[test]
    fn rejects_zero_capture_limits() {
        let limits = MexcCaptureLimits {
            max_line_bytes: 0,
            ..MexcCaptureLimits::default()
        };
        assert!(matches!(
            MexcCaptureReader::with_limits(Cursor::new(""), limits),
            Err(MexcCaptureError::InvalidLimits)
        ));
    }

    #[test]
    fn preserves_exact_decimal_tokens_from_json_numbers() {
        let spec = MexcContractSpec::from_steps("BTC_USDT", "0.000000001", "0.000000001", "0.001")
            .unwrap();
        let raw: Value = serde_json::from_str(
            r#"{"bids":[[10000000.000000001,1,1]],"asks":[[10000000.000000002,1,1]]}"#,
        )
        .unwrap();
        let bids = parse_snapshot_levels(&raw["bids"], &spec, 100).unwrap();
        let asks = parse_snapshot_levels(&raw["asks"], &spec, 100).unwrap();
        assert!(bids.contains_key(&10_000_000_000_000_001));
        assert!(asks.contains_key(&10_000_000_000_000_002));
    }

    #[test]
    fn accepts_locked_top_of_book() {
        let mut book = DepthBook::default();
        book.bids.insert(10_000, 100);
        book.asks.insert(10_000, 100);
        assert_eq!(best_prices(&book), Ok((10_000, 10_000)));
    }

    #[test]
    fn reads_capture_and_emits_sorted_trade_and_top_of_book_events() {
        let reader = MexcCaptureReader::new(Cursor::new(capture_lines(11)));
        let events = reader.read_sorted().unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.event_time_ms, 1_000);
        assert_eq!(events[1].event.event_time_ms, 1_004);
        assert_eq!(events[2].event.event_time_ms, 1_009);

        assert_eq!(
            events[0].event.payload,
            EventPayload::Quote {
                bid_ticks: 10_000,
                ask_ticks: 10_010,
                price_scale: 100,
            }
        );
        assert_eq!(
            events[1].event.payload,
            EventPayload::Trade {
                price_ticks: 10_005,
                quantity_lots: 50,
                price_scale: 100,
            }
        );
        assert_eq!(
            events[2].event.payload,
            EventPayload::Quote {
                bid_ticks: 10_005,
                ask_ticks: 10_010,
                price_scale: 100,
            }
        );
        assert_eq!(
            events[1].metadata,
            MexcObservationMetadata::Trade {
                trade_side: MexcTradeSide::Buy,
                position_action: MexcPositionAction::Open,
                self_trade: MexcSelfTradeFlag::No,
                transaction_id: Some("tx-1".to_owned()),
                local_event_index: None,
                trade_time_ms: 1_005,
                matching_engine_time_ms: Some(1_004),
            }
        );
    }

    #[test]
    fn rejects_non_mexc_or_non_perpetual_capture() {
        let lines = capture_lines(11).replace(r#""venue":"MEXC""#, r#""venue":"OTHER""#);
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        assert_eq!(
            reader.read_sorted(),
            Err(MexcCaptureError::UnsupportedVenue)
        );
    }

    #[test]
    fn reads_capture_when_trade_id_is_absent() {
        let lines = capture_lines(11).replace(r#","transaction_id":"tx-1""#, "");
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        let events = reader.read_sorted().unwrap();
        assert!(events[1].event.event_id.contains(":deal:local:1005:0:"));
        assert!(matches!(
            events[1].metadata,
            MexcObservationMetadata::Trade {
                transaction_id: None,
                local_event_index: Some(0),
                ..
            }
        ));
    }

    #[test]
    fn rejects_capture_end_marked_non_contiguous() {
        let lines = capture_lines(11)
            .replace(r#""depth_sequence_gaps":0,"#, r#""depth_sequence_gaps":1,"#)
            .replace(
                r#""depth_stream_contiguous":true"#,
                r#""depth_stream_contiguous":false"#,
            );
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        assert_eq!(
            reader.read_sorted(),
            Err(MexcCaptureError::CaptureMarkedNonContiguous)
        );
    }


    #[test]
    fn streaming_iterator_reports_missing_capture_end() {
        let mut records: Vec<String> = capture_lines(11).lines().map(str::to_owned).collect();
        records.pop();
        let lines = records.join("\n");
        let mut reader = MexcCaptureReader::new(Cursor::new(lines));
        while let Some(item) = reader.next() {
            assert!(item.is_ok());
        }
    }

    #[test]
    fn rejects_capture_without_terminal_record() {
        let mut records: Vec<String> = capture_lines(11).lines().map(str::to_owned).collect();
        records.pop();
        let lines = records.join("\n");
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        assert_eq!(
            reader.read_sorted(),
            Err(MexcCaptureError::MissingCaptureEnd)
        );
    }

    #[test]
    fn reads_three_field_depth_rows_using_contract_volume_not_order_count() {
        let spec = MexcContractSpec::from_steps("BTC_USDT", "0.01", "0.01", "0.001").unwrap();
        let levels = json!([["100.00", 1.00, 9]]);
        let parsed = parse_snapshot_levels(&levels, &spec, 10).unwrap();
        assert_eq!(parsed.get(&10_000), Some(&100));
    }

    #[test]
    fn rejects_unsupported_capture_schema_version() {
        let lines = capture_lines(11).replacen(r#""schema_version":1"#, r#""schema_version":2"#, 1);
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        assert_eq!(
            reader.read_sorted(),
            Err(MexcCaptureError::UnsupportedCaptureSchema(2))
        );
    }

    #[test]
    fn rejects_non_perpetual_market() {
        let lines =
            capture_lines(11).replace(r#""market":"FUTURES_PERPETUAL""#, r#""market":"SPOT""#);
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        assert_eq!(
            reader.read_sorted(),
            Err(MexcCaptureError::UnsupportedMarket)
        );
    }

    #[test]
    fn iterator_emits_events_incrementally_in_capture_order() {
        let mut reader = MexcCaptureReader::new(Cursor::new(capture_lines(11)));
        let first = reader.next().unwrap().unwrap();
        assert_eq!(first.event.event_time_ms, 1_000);
        assert_eq!(first.event.source_id, "mexc-futures:900");

        let second = reader.next().unwrap().unwrap();
        assert_eq!(second.event.event_time_ms, 1_004);
        assert!(matches!(second.event.payload, EventPayload::Trade { .. }));
    }

    #[test]
    fn fails_closed_on_depth_sequence_gap() {
        let reader = MexcCaptureReader::new(Cursor::new(capture_lines(12)));
        let result = reader.read_sorted();
        assert_eq!(
            result,
            Err(MexcCaptureError::DepthSequenceGap {
                expected: 11,
                actual: 12,
            })
        );
    }

    #[test]
    fn preserves_unknown_exchange_enum_codes_from_capture() {
        let mut lines = capture_lines(11);
        lines = lines.replace(r#""side":"Buy""#, r#""side":"Other(9)""#);
        let reader = MexcCaptureReader::new(Cursor::new(lines));
        let events = reader.read_sorted().unwrap();
        assert!(matches!(
            events[1].metadata,
            MexcObservationMetadata::Trade {
                trade_side: MexcTradeSide::Other(9),
                ..
            }
        ));
    }
}
