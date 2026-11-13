use sha2::{Digest, Sha256};

use crate::event::{MarketEvent, Quote};

/// Метаданные одного MEXC Futures контракта.
///
/// priceScale и volScale из MEXC contract metadata представлены здесь
/// множителями фиксированной точки, а не количеством десятичных знаков.
/// priceUnitTicks и volumeUnitLots сохраняют минимальный шаг цены и объёма.
/// contract_size сохраняется как исходная десятичная строка: его смысл и
/// расчёт PnL зависят от спецификации конкретного контракта.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MexcContractSpec {
    pub symbol: String,
    pub price_scale_factor: u32,
    pub volume_scale_factor: u64,
    pub price_unit_ticks: i64,
    pub volume_unit_lots: u64,
    pub contract_size: String,
}

impl MexcContractSpec {
    /// Создаёт спецификацию из priceUnit/volUnit MEXC или capture manifest.
    /// Десятичная точность выводится из шага без округления.
    pub fn from_steps(
        symbol: impl Into<String>,
        price_unit: &str,
        volume_unit: &str,
        contract_size: impl Into<String>,
    ) -> Result<Self, MexcError> {
        let price_decimals = decimal_places(price_unit)?;
        let volume_decimals = decimal_places(volume_unit)?;
        Self::new(
            symbol,
            price_decimals,
            volume_decimals,
            price_unit,
            volume_unit,
            contract_size,
        )
    }

    /// Создаёт спецификацию из полей MEXC contract metadata.
    ///
    /// price_decimals соответствует priceScale, volume_decimals — volScale.
    pub fn new(
        symbol: impl Into<String>,
        price_decimals: u32,
        volume_decimals: u32,
        price_unit: &str,
        volume_unit: &str,
        contract_size: impl Into<String>,
    ) -> Result<Self, MexcError> {
        let symbol = symbol.into();
        let contract_size = contract_size.into();
        if symbol.trim().is_empty()
            || symbol.trim() != symbol
            || !symbol
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            || contract_size.trim().is_empty()
        {
            return Err(MexcError::InvalidContractSpec);
        }

        let price_scale_factor = decimal_scale_factor(price_decimals)?;
        let volume_scale_factor = decimal_scale_factor_u64(volume_decimals)?;
        let price_unit_ticks = parse_scaled_i64(price_unit, price_scale_factor)?;
        let volume_unit_lots = parse_scaled_u64(volume_unit, volume_scale_factor)?;

        if price_unit_ticks == 0 || volume_unit_lots == 0 {
            return Err(MexcError::InvalidContractSpec);
        }

        Ok(Self {
            symbol,
            price_scale_factor,
            volume_scale_factor,
            price_unit_ticks,
            volume_unit_lots,
            contract_size,
        })
    }
}

/// Декодированное поле MEXC ticker.
///
/// Транспортный слой разбирает JSON/WebSocket кадр в эту структуру,
/// сохраняя исходный кадр отдельно для аудита.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MexcTickerFrame {
    pub symbol: String,
    pub bid1: String,
    pub ask1: String,
    pub last_price: String,
    pub timestamp_ms: i64,
}

/// Декодированная запись MEXC deal.
///
/// trade_type: 1 — покупка, 2 — продажа.
/// position_action: 1 — открытие, 2 — сокращение, 3 — без изменения позиции.
/// self_trade: 1 — собственная сделка, 2 — не собственная.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MexcDealFrame {
    pub symbol: String,
    pub transaction_id: String,
    pub price: String,
    pub volume: String,
    pub trade_type: u64,
    pub position_action: u64,
    pub self_trade: u64,
    pub timestamp_ms: i64,
    pub matching_engine_time_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MexcTradeSide {
    Buy,
    Sell,
    Other(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MexcPositionAction {
    Open,
    Reduce,
    Unchanged,
    Other(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MexcSelfTradeFlag {
    Yes,
    No,
    Other(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MexcObservationMetadata {
    Quote {
        last_price_ticks: Option<i64>,
    },
    Trade {
        trade_side: MexcTradeSide,
        position_action: MexcPositionAction,
        self_trade: MexcSelfTradeFlag,
        transaction_id: String,
        trade_time_ms: i64,
        matching_engine_time_ms: Option<i64>,
    },
}

/// Нормализованное событие MEXC с сохранением дополнительных полей источника.
///
/// raw_payload_sha256 связывает событие с исходным кадром. Сборщик обязан
/// сохранять сам исходный кадр отдельно; хеш не заменяет первичные данные.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MexcObservation {
    pub event: MarketEvent,
    pub metadata: MexcObservationMetadata,
    pub raw_payload_sha256: [u8; 32],
    pub volume_scale_factor: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MexcError {
    InvalidContractSpec,
    SymbolMismatch,
    EmptyTransactionId,
    EmptyRawPayload,
    InvalidTimestamp,
    InvalidDecimal,
    DecimalPrecisionExceeded,
    NumericOverflow,
    InvalidQuote,
    InvalidTrade,
    PriceOffTick,
    VolumeOffStep,
}

/// Нормализует MEXC Futures ticker в Quote.
///
/// Цены преобразуются в целые фиксированные значения без f64.
/// received_time_ms задаётся локальным сборщиком при получении кадра.
pub fn normalize_ticker(
    spec: &MexcContractSpec,
    frame: &MexcTickerFrame,
    received_time_ms: i64,
    raw_payload: &str,
) -> Result<MexcObservation, MexcError> {
    validate_common(
        spec,
        &frame.symbol,
        frame.timestamp_ms,
        received_time_ms,
        raw_payload,
    )?;

    let bid_ticks = parse_scaled_i64(&frame.bid1, spec.price_scale_factor)?;
    let ask_ticks = parse_scaled_i64(&frame.ask1, spec.price_scale_factor)?;
    let last_price_ticks = parse_scaled_i64(&frame.last_price, spec.price_scale_factor)?;

    if bid_ticks <= 0 || ask_ticks < bid_ticks || last_price_ticks <= 0 {
        return Err(MexcError::InvalidQuote);
    }
    if bid_ticks % spec.price_unit_ticks != 0
        || ask_ticks % spec.price_unit_ticks != 0
        || last_price_ticks % spec.price_unit_ticks != 0
    {
        return Err(MexcError::PriceOffTick);
    }

    let digest = Sha256::digest(raw_payload.as_bytes());
    let raw_payload_sha256: [u8; 32] = digest.into();
    let event_id = format!(
        "mexc:{}:ticker:{}:{}",
        spec.symbol,
        frame.timestamp_ms,
        hex_digest(&raw_payload_sha256)
    );

    let event = MarketEvent::quote(
        event_id,
        "mexc-futures",
        spec.symbol.clone(),
        frame.timestamp_ms,
        received_time_ms,
        None,
        Quote {
            bid_ticks,
            ask_ticks,
            price_scale: spec.price_scale_factor,
        },
    );

    Ok(MexcObservation {
        event,
        metadata: MexcObservationMetadata::Quote {
            last_price_ticks: Some(last_price_ticks),
        },
        raw_payload_sha256,
        volume_scale_factor: spec.volume_scale_factor,
    })
}

/// Нормализует одну MEXC Futures сделку.
///
/// Сторона сделки, действие с позицией и флаг self-trade сохраняются
/// отдельно от базового MarketEvent, не теряясь при передаче в исследовательское ядро.
pub fn normalize_deal(
    spec: &MexcContractSpec,
    frame: &MexcDealFrame,
    received_time_ms: i64,
    raw_payload: &str,
) -> Result<MexcObservation, MexcError> {
    validate_common(
        spec,
        &frame.symbol,
        frame.timestamp_ms,
        received_time_ms,
        raw_payload,
    )?;

    if frame.transaction_id.trim().is_empty() {
        return Err(MexcError::EmptyTransactionId);
    }

    let price_ticks = parse_scaled_i64(&frame.price, spec.price_scale_factor)?;
    let quantity_lots = parse_scaled_u64(&frame.volume, spec.volume_scale_factor)?;

    if price_ticks <= 0 || quantity_lots == 0 {
        return Err(MexcError::InvalidTrade);
    }
    if price_ticks % spec.price_unit_ticks != 0 {
        return Err(MexcError::PriceOffTick);
    }
    if quantity_lots % spec.volume_unit_lots != 0 {
        return Err(MexcError::VolumeOffStep);
    }

    let trade_side = match frame.trade_type {
        1 => MexcTradeSide::Buy,
        2 => MexcTradeSide::Sell,
        other => MexcTradeSide::Other(other),
    };
    let position_action = match frame.position_action {
        1 => MexcPositionAction::Open,
        2 => MexcPositionAction::Reduce,
        3 => MexcPositionAction::Unchanged,
        other => MexcPositionAction::Other(other),
    };
    let self_trade = match frame.self_trade {
        1 => MexcSelfTradeFlag::Yes,
        2 => MexcSelfTradeFlag::No,
        other => MexcSelfTradeFlag::Other(other),
    };

    if frame.matching_engine_time_ms.is_some_and(|time| time < 0) {
        return Err(MexcError::InvalidTimestamp);
    }

    let digest = Sha256::digest(raw_payload.as_bytes());
    let raw_payload_sha256: [u8; 32] = digest.into();
    let event_id = format!("mexc:{}:deal:{}", spec.symbol, frame.transaction_id);

    let event = MarketEvent {
        schema_version: 1,
        event_id,
        source_id: "mexc-futures".to_owned(),
        instrument_id: spec.symbol.clone(),
        event_time_ms: frame.matching_engine_time_ms.unwrap_or(frame.timestamp_ms),
        received_time_ms,
        sequence: None,
        payload: crate::event::EventPayload::Trade {
            price_ticks,
            quantity_lots,
            price_scale: spec.price_scale_factor,
        },
    };

    Ok(MexcObservation {
        event,
        metadata: MexcObservationMetadata::Trade {
            trade_side,
            position_action,
            self_trade,
            transaction_id: frame.transaction_id.clone(),
            trade_time_ms: frame.timestamp_ms,
            matching_engine_time_ms: frame.matching_engine_time_ms,
        },
        raw_payload_sha256,
        volume_scale_factor: spec.volume_scale_factor,
    })
}

fn validate_common(
    spec: &MexcContractSpec,
    symbol: &str,
    event_time_ms: i64,
    received_time_ms: i64,
    raw_payload: &str,
) -> Result<(), MexcError> {
    if symbol != spec.symbol {
        return Err(MexcError::SymbolMismatch);
    }
    if event_time_ms < 0 || received_time_ms < 0 {
        return Err(MexcError::InvalidTimestamp);
    }
    if raw_payload.is_empty() {
        return Err(MexcError::EmptyRawPayload);
    }
    Ok(())
}

fn decimal_places(value: &str) -> Result<u32, MexcError> {
    if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
        return Err(MexcError::InvalidDecimal);
    }
    let mut parts = value.split('.');
    let whole = parts.next().ok_or(MexcError::InvalidDecimal)?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(MexcError::InvalidDecimal);
    }
    Ok(fraction.trim_end_matches('0').len() as u32)
}

fn decimal_scale_factor(decimals: u32) -> Result<u32, MexcError> {
    if decimals > 9 {
        return Err(MexcError::InvalidContractSpec);
    }
    10u32
        .checked_pow(decimals)
        .ok_or(MexcError::NumericOverflow)
}

fn decimal_scale_factor_u64(decimals: u32) -> Result<u64, MexcError> {
    if decimals > 18 {
        return Err(MexcError::InvalidContractSpec);
    }
    10u64
        .checked_pow(decimals)
        .ok_or(MexcError::NumericOverflow)
}

pub(crate) fn parse_scaled_i64(value: &str, scale: u32) -> Result<i64, MexcError> {
    let parsed = parse_scaled_u64(value, u64::from(scale))?;
    i64::try_from(parsed).map_err(|_| MexcError::NumericOverflow)
}

pub(crate) fn parse_scaled_u64(value: &str, scale: u64) -> Result<u64, MexcError> {
    if value.is_empty() || value.starts_with('-') || value.starts_with('+') {
        return Err(MexcError::InvalidDecimal);
    }

    let mut parts = value.split('.');
    let whole = parts.next().ok_or(MexcError::InvalidDecimal)?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(MexcError::InvalidDecimal);
    }

    let decimals = scale.to_string().len() - 1;
    let mut fractional = fraction.to_owned();
    if fractional.len() > decimals {
        if fractional[decimals..].bytes().any(|byte| byte != b'0') {
            return Err(MexcError::DecimalPrecisionExceeded);
        }
        fractional.truncate(decimals);
    }
    while fractional.len() < decimals {
        fractional.push('0');
    }

    let whole_value = whole
        .parse::<u64>()
        .map_err(|_| MexcError::NumericOverflow)?;
    let fractional_value = if fractional.is_empty() {
        0
    } else {
        fractional
            .parse::<u64>()
            .map_err(|_| MexcError::NumericOverflow)?
    };
    whole_value
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fractional_value))
        .ok_or(MexcError::NumericOverflow)
}

fn hex_digest(digest: &[u8; 32]) -> String {
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
    use crate::event::EventPayload;

    fn spec() -> MexcContractSpec {
        MexcContractSpec::new("BTC_USDT", 2, 2, "0.01", "0.01", "0.001").unwrap()
    }

    fn ticker() -> MexcTickerFrame {
        MexcTickerFrame {
            symbol: "BTC_USDT".to_owned(),
            bid1: "100.01".to_owned(),
            ask1: "100.02".to_owned(),
            last_price: "100.02".to_owned(),
            timestamp_ms: 1_000,
        }
    }

    fn deal() -> MexcDealFrame {
        MexcDealFrame {
            symbol: "BTC_USDT".to_owned(),
            transaction_id: "tx-100".to_owned(),
            price: "100.02".to_owned(),
            volume: "2.50".to_owned(),
            trade_type: 1,
            position_action: 1,
            self_trade: 2,
            timestamp_ms: 1_100,
            matching_engine_time_ms: Some(1_099),
        }
    }

    #[test]
    fn normalizes_ticker_to_integer_quote_without_float_conversion() {
        let result =
            normalize_ticker(&spec(), &ticker(), 1_005, r#"{"channel":"push.ticker"}"#).unwrap();

        assert_eq!(result.event.source_id, "mexc-futures");
        assert_eq!(result.event.instrument_id, "BTC_USDT");
        assert_eq!(result.event.event_time_ms, 1_000);
        assert_eq!(result.event.received_time_ms, 1_005);
        assert_eq!(
            result.event.payload,
            EventPayload::Quote {
                bid_ticks: 10_001,
                ask_ticks: 10_002,
                price_scale: 100,
            }
        );
        assert_eq!(
            result.metadata,
            MexcObservationMetadata::Quote {
                last_price_ticks: Some(10_002)
            }
        );
    }

    #[test]
    fn normalizes_deal_and_preserves_exchange_trade_metadata() {
        let result = normalize_deal(&spec(), &deal(), 1_105, r#"{"channel":"push.deal"}"#).unwrap();

        assert_eq!(result.event.event_id, "mexc:BTC_USDT:deal:tx-100");
        assert_eq!(
            result.event.payload,
            EventPayload::Trade {
                price_ticks: 10_002,
                quantity_lots: 250,
                price_scale: 100,
            }
        );
        assert_eq!(
            result.metadata,
            MexcObservationMetadata::Trade {
                trade_side: MexcTradeSide::Buy,
                position_action: MexcPositionAction::Open,
                self_trade: MexcSelfTradeFlag::No,
                transaction_id: "tx-100".to_owned(),
                trade_time_ms: 1_100,
                matching_engine_time_ms: Some(1_099),
            }
        );
        assert_eq!(result.volume_scale_factor, 100);
    }

    #[test]
    fn supports_volume_precision_above_price_scale_limit() {
        let spec =
            MexcContractSpec::new("BTC_USDT", 2, 10, "0.01", "0.0000000001", "0.001").unwrap();
        let mut frame = deal();
        frame.volume = "1.0000000001".to_owned();

        let result = normalize_deal(&spec, &frame, 1_105, "{}").unwrap();
        assert_eq!(result.volume_scale_factor, 10_000_000_000);
        assert_eq!(
            result.event.payload,
            EventPayload::Trade {
                price_ticks: 10_002,
                quantity_lots: 10_000_000_001,
                price_scale: 100,
            }
        );
    }

    #[test]
    fn rejects_symbol_mismatch() {
        let mut frame = ticker();
        frame.symbol = "ETH_USDT".to_owned();

        assert_eq!(
            normalize_ticker(&spec(), &frame, 1_005, "{}"),
            Err(MexcError::SymbolMismatch)
        );
    }

    #[test]
    fn rejects_prices_that_do_not_match_contract_tick() {
        let spec = MexcContractSpec::new("BTC_USDT", 2, 2, "0.05", "0.01", "0.001").unwrap();

        assert_eq!(
            normalize_ticker(&spec, &ticker(), 1_005, "{}"),
            Err(MexcError::PriceOffTick)
        );
    }

    #[test]
    fn rejects_volume_that_does_not_match_contract_step() {
        let spec = MexcContractSpec::new("BTC_USDT", 2, 2, "0.01", "0.25", "0.001").unwrap();
        let mut frame = deal();
        frame.volume = "0.30".to_owned();

        assert_eq!(
            normalize_deal(&spec, &frame, 1_105, "{}"),
            Err(MexcError::VolumeOffStep)
        );
    }

    #[test]
    fn preserves_unknown_exchange_enum_values() {
        let mut frame = deal();
        frame.trade_type = 3;
        frame.position_action = 4;
        frame.self_trade = 5;

        let result = normalize_deal(&spec(), &frame, 1_105, "{}").unwrap();
        assert_eq!(
            result.metadata,
            MexcObservationMetadata::Trade {
                trade_side: MexcTradeSide::Other(3),
                position_action: MexcPositionAction::Other(4),
                self_trade: MexcSelfTradeFlag::Other(5),
                transaction_id: "tx-100".to_owned(),
                matching_engine_time_ms: Some(1_099),
            }
        );
    }

    #[test]
    fn rejects_precision_loss_in_decimal_conversion() {
        let mut frame = ticker();
        frame.bid1 = "100.011".to_owned();

        assert_eq!(
            normalize_ticker(&spec(), &frame, 1_005, "{}"),
            Err(MexcError::DecimalPrecisionExceeded)
        );
    }

    #[test]
    fn rejects_invalid_timestamps_and_empty_raw_payload() {
        let mut frame = ticker();
        frame.timestamp_ms = -1;

        assert_eq!(
            normalize_ticker(&spec(), &frame, 1_005, "{}"),
            Err(MexcError::InvalidTimestamp)
        );

        assert_eq!(
            normalize_ticker(&spec(), &ticker(), 1_005, ""),
            Err(MexcError::EmptyRawPayload)
        );
    }
}
