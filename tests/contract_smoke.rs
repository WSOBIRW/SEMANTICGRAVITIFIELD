use semantic_gravity_field::{ErrorPreventionEngine, MarketEvent, Quote};

#[test]
fn public_api_accepts_valid_market_quote() {
    let event = MarketEvent::quote(
        "event-0001",
        "provider-demo",
        "EURUSD",
        1_700_000_000_000,
        1_700_000_000_005,
        Some(1),
        Quote {
            bid_ticks: 108_250,
            ask_ticks: 108_252,
            price_scale: 100_000,
        },
    );

    let mut engine = ErrorPreventionEngine::new();
    let report = engine.inspect(&event);
    assert!(report.accepted);
    assert!(report.findings.is_empty());
}
