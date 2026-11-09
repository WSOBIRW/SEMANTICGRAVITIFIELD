/// Нормализованное рыночное событие.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketEvent {
    pub schema_version: u16,
    pub event_id: String,
    pub source_id: String,
    pub instrument_id: String,
    /// Время события в Unix milliseconds UTC.
    pub event_time_ms: i64,
    /// Время получения события в Unix milliseconds UTC.
    pub received_time_ms: i64,
    /// Последовательность внутри потока источника, если предоставляется.
    pub sequence: Option<u64>,
    pub payload: EventPayload,
}

/// Типизированная котировка в целых тиках.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    pub bid_ticks: i64,
    pub ask_ticks: i64,
    pub price_scale: u32,
}

/// Минимальные типизированные payload для первого этапа.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventPayload {
    Quote {
        bid_ticks: i64,
        ask_ticks: i64,
        price_scale: u32,
    },
    Trade {
        price_ticks: i64,
        quantity_lots: u64,
        price_scale: u32,
    },
}

impl MarketEvent {
    pub fn quote(
        event_id: impl Into<String>,
        source_id: impl Into<String>,
        instrument_id: impl Into<String>,
        event_time_ms: i64,
        received_time_ms: i64,
        sequence: Option<u64>,
        quote: Quote,
    ) -> Self {
        Self {
            schema_version: 1,
            event_id: event_id.into(),
            source_id: source_id.into(),
            instrument_id: instrument_id.into(),
            event_time_ms,
            received_time_ms,
            sequence,
            payload: EventPayload::Quote {
                bid_ticks: quote.bid_ticks,
                ask_ticks: quote.ask_ticks,
                price_scale: quote.price_scale,
            },
        }
    }
}
