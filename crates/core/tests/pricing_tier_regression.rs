use std::collections::BTreeMap;

use codex_helper_core::pricing::{
    CostAdjustments, CostConfidence, ModelPrice, ModelPriceCatalog, ModelPriceTier,
    PriceMultiplier, UsdAmount, estimate_usage_cost_with_accounting,
};
use codex_helper_core::usage::{CacheInputAccounting, UsageMetrics};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditFixture {
    model: String,
    provider: String,
    rows: Vec<AuditRow>,
    expected_days: BTreeMap<String, String>,
    expected_total: String,
}

#[derive(Debug, Deserialize)]
struct AuditRow(String, i64, i64, i64, i64, String, String);

fn gpt_56_sol_price() -> ModelPrice {
    ModelPrice::from_per_million_usd_for_provider(
        "openai",
        "gpt-5.6-sol",
        Some("GPT-5.6 Sol".to_string()),
        "5",
        "30",
        Some("0.5"),
        Some("6.25"),
        "basellm:test",
    )
    .expect("base price")
    .with_tiers(vec![
        ModelPriceTier::from_per_million_usd(
            272_000,
            Some("10"),
            Some("45"),
            Some("1"),
            Some("12.5"),
        )
        .expect("long-context tier"),
    ])
    .expect("valid tiers")
    .with_source_generation("fixture-generation")
}

#[test]
fn pricing_context_tier_boundary_uses_whole_request_and_strict_greater_than() {
    let price = gpt_56_sol_price();
    let base_usage = UsageMetrics {
        input_tokens: 100_000,
        output_tokens: 10,
        cache_read_input_tokens: 172_000,
        cache_creation_input_tokens: 10,
        ..UsageMetrics::default()
    };
    let base = estimate_usage_cost_with_accounting(
        &base_usage,
        &price,
        CostAdjustments::default(),
        CacheInputAccounting::DirectReadSeparate,
    );
    assert_eq!(base.selected_tier, None);
    assert_eq!(base.input_cost_usd.as_deref(), Some("0.5"));
    assert_eq!(base.cache_read_cost_usd.as_deref(), Some("0.086"));
    assert_eq!(base.output_cost_usd.as_deref(), Some("0.0003"));
    assert_eq!(base.cache_creation_cost_usd.as_deref(), Some("0.0000625"));

    let tiered_usage = UsageMetrics {
        input_tokens: 100_001,
        output_tokens: 10,
        cache_read_input_tokens: 172_000,
        cache_creation_input_tokens: 10,
        ..UsageMetrics::default()
    };
    let tiered = estimate_usage_cost_with_accounting(
        &tiered_usage,
        &price,
        CostAdjustments::default(),
        CacheInputAccounting::DirectReadSeparate,
    );
    let selected = tiered.selected_tier.expect("long-context tier selected");
    assert_eq!(selected.threshold_tokens, 272_000);
    assert_eq!(selected.matched_input_tokens, 272_001);
    assert_eq!(tiered.input_cost_usd.as_deref(), Some("1.00001"));
    assert_eq!(tiered.cache_read_cost_usd.as_deref(), Some("0.172"));
    assert_eq!(tiered.output_cost_usd.as_deref(), Some("0.00045"));
    assert_eq!(tiered.cache_creation_cost_usd.as_deref(), Some("0.000125"));
    assert_eq!(tiered.pricing_provider.as_deref(), Some("openai"));
    assert_eq!(
        tiered.pricing_generation.as_deref(),
        Some("fixture-generation")
    );
}

#[test]
fn pricing_cache_read_counts_once_for_context_threshold_and_once_for_billing() {
    let price = gpt_56_sol_price();
    let usage = UsageMetrics {
        // Codex reports input including direct cache reads.
        input_tokens: 272_001,
        output_tokens: 1,
        cache_read_input_tokens: 172_000,
        ..UsageMetrics::default()
    };
    let cost = estimate_usage_cost_with_accounting(
        &usage,
        &price,
        CostAdjustments::default(),
        CacheInputAccounting::DirectReadIncludedInInput,
    );

    let selected = cost.selected_tier.expect("long-context tier selected");
    assert_eq!(selected.matched_input_tokens, 272_001);
    // ordinary input is 100001, so the cache read is not charged as regular input.
    assert_eq!(cost.input_cost_usd.as_deref(), Some("1.00001"));
    assert_eq!(cost.cache_read_cost_usd.as_deref(), Some("0.172"));
}

#[test]
fn pricing_partial_tier_overlay_does_not_turn_missing_nonzero_component_into_free_cost() {
    let price = ModelPrice::from_per_million_usd_for_provider(
        "openai", "partial", None, "1", "2", None, None, "manual",
    )
    .expect("base price")
    .with_tiers(vec![
        ModelPriceTier::from_per_million_usd(1, Some("3"), None, None, None).expect("partial tier"),
    ])
    .expect("valid tier");
    let usage = UsageMetrics {
        input_tokens: 2,
        cache_creation_input_tokens: 2,
        ..UsageMetrics::default()
    };

    let cost = estimate_usage_cost_with_accounting(
        &usage,
        &price,
        CostAdjustments::default(),
        CacheInputAccounting::DirectReadSeparate,
    );
    assert_eq!(cost.confidence, CostConfidence::Partial);
    assert_eq!(cost.input_cost_usd.as_deref(), Some("0.000006"));
    assert_eq!(cost.cache_creation_cost_usd, None);
    assert_eq!(cost.total_cost_usd.as_deref(), Some("0.000006"));
}

#[test]
fn pricing_service_and_provider_multipliers_apply_once_after_tiered_components() {
    let price = ModelPrice::from_per_million_usd_for_provider(
        "openai",
        "multiplied",
        None,
        "1",
        "2",
        None,
        None,
        "test",
    )
    .expect("base price")
    .with_tiers(vec![
        ModelPriceTier::from_per_million_usd(1, Some("10"), None, None, None).expect("tier"),
    ])
    .expect("valid tier");
    let usage = UsageMetrics {
        input_tokens: 1_000_000,
        ..UsageMetrics::default()
    };

    let cost = estimate_usage_cost_with_accounting(
        &usage,
        &price,
        CostAdjustments {
            service_tier_multiplier: PriceMultiplier::from_decimal_str("2"),
            provider_multiplier: PriceMultiplier::from_decimal_str("3"),
        },
        CacheInputAccounting::DirectReadSeparate,
    );

    assert_eq!(cost.input_cost_usd.as_deref(), Some("10"));
    assert_eq!(cost.total_cost_usd.as_deref(), Some("60"));
}

#[test]
fn pricing_combines_fractional_multipliers_before_one_final_rounding() {
    let femto = "0.000000000000001";
    let price = ModelPrice::from_per_million_usd_for_provider(
        "openai", "rounding", None, femto, femto, None, None, "test",
    )
    .expect("femto price");
    let adjustments = CostAdjustments {
        service_tier_multiplier: PriceMultiplier::from_decimal_str("0.5"),
        provider_multiplier: PriceMultiplier::from_decimal_str("0.8"),
    };
    let multi_component = estimate_usage_cost_with_accounting(
        &UsageMetrics {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..UsageMetrics::default()
        },
        &price,
        adjustments,
        CacheInputAccounting::DirectReadSeparate,
    );
    assert_eq!(multi_component.input_cost_usd.as_deref(), Some(femto));
    assert_eq!(multi_component.output_cost_usd.as_deref(), Some(femto));
    assert_eq!(multi_component.total_cost_usd.as_deref(), Some(femto));

    let fractional_boundary = estimate_usage_cost_with_accounting(
        &UsageMetrics {
            input_tokens: 1_000_000,
            ..UsageMetrics::default()
        },
        &price,
        CostAdjustments {
            service_tier_multiplier: PriceMultiplier::from_decimal_str("0.5"),
            provider_multiplier: PriceMultiplier::from_decimal_str("3"),
        },
        CacheInputAccounting::DirectReadSeparate,
    );
    assert_eq!(
        fractional_boundary.total_cost_usd.as_deref(),
        Some("0.000000000000002")
    );
}

#[test]
fn pricing_provider_namespaces_keep_colliding_models_separate_and_codex_selects_openai() {
    let openai = ModelPrice::from_per_million_usd_for_provider(
        "openai",
        "same-model",
        None,
        "1",
        "2",
        None,
        None,
        "openai",
    )
    .expect("openai price");
    let routing = ModelPrice::from_per_million_usd_for_provider(
        "routing-run",
        "same-model",
        None,
        "9",
        "18",
        None,
        None,
        "routing",
    )
    .expect("routing price");
    let catalog = ModelPriceCatalog::with_prices([openai, routing]);

    assert_eq!(
        catalog
            .price_for_service_model("codex", "same-model")
            .expect("codex provider")
            .provider,
        "openai"
    );
    assert_eq!(
        catalog
            .price_for_provider_model("routing-run", "same-model")
            .expect("routing provider")
            .input_per_1m
            .format_usd(),
        "9"
    );
}

#[test]
fn pricing_manual_row_shadows_tiers_only_in_its_provider_namespace() {
    let remote_openai = ModelPrice::from_per_million_usd_for_provider(
        "openai",
        "shared-model",
        None,
        "1",
        "2",
        None,
        None,
        "remote",
    )
    .expect("remote openai")
    .with_tiers(vec![
        ModelPriceTier::from_per_million_usd(10, Some("3"), None, None, None).expect("openai tier"),
    ])
    .expect("openai tiers");
    let remote_routing = ModelPrice::from_per_million_usd_for_provider(
        "routing-run",
        "shared-model",
        None,
        "4",
        "8",
        None,
        None,
        "remote",
    )
    .expect("remote routing")
    .with_tiers(vec![
        ModelPriceTier::from_per_million_usd(10, Some("12"), None, None, None)
            .expect("routing tier"),
    ])
    .expect("routing tiers");
    let manual_openai = ModelPrice::from_per_million_usd_for_provider(
        "openai",
        "shared-model",
        None,
        "9",
        "18",
        None,
        None,
        "manual",
    )
    .expect("manual openai");

    let mut catalog = ModelPriceCatalog::with_prices([remote_openai, remote_routing]);
    catalog.insert(manual_openai);

    let openai = catalog
        .price_for_service_model("codex", "shared-model")
        .expect("manual openai row");
    assert_eq!(openai.source, "manual");
    assert!(openai.tiers.is_empty());
    let routing = catalog
        .price_for_provider_model("routing-run", "shared-model")
        .expect("routing row");
    assert_eq!(routing.source, "remote");
    assert_eq!(routing.tiers.len(), 1);
}

#[test]
fn pricing_csv_audit_fixture_reproduces_daily_and_total_cost_without_residual() {
    let fixture_text = include_str!("fixtures/pricing/gpt-5.6-sol-audit.json");
    let scrubbed = fixture_text.to_ascii_lowercase();
    for forbidden in [
        "\"api_key\"",
        "\"authorization\"",
        "bearer ",
        "sk-",
        "\"secret\"",
        "\"credential\"",
        "\"base_url\"",
        "\"endpoint\"",
        "\"request_body\"",
        "\"response_body\"",
    ] {
        assert!(
            !scrubbed.contains(forbidden),
            "audit fixture contains forbidden sensitive field or marker {forbidden}"
        );
    }
    let fixture: AuditFixture = serde_json::from_str(fixture_text).expect("audit fixture");
    assert_eq!(fixture.model, "gpt-5.6-sol");
    assert_eq!(fixture.provider, "openai");
    assert_eq!(fixture.rows.len(), 8_405);
    assert_eq!(
        fixture.expected_days.get("2026-07-11").map(String::as_str),
        Some("501.493510")
    );
    assert_eq!(
        fixture.expected_days.get("2026-07-12").map(String::as_str),
        Some("500.108175")
    );
    assert_eq!(fixture.expected_total, "1001.601685");

    let price = gpt_56_sol_price();
    let mut daily = BTreeMap::<String, i128>::new();
    let mut total = 0_i128;
    for AuditRow(
        day,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        multiplier,
        expected_cost,
    ) in &fixture.rows
    {
        let multiplier = PriceMultiplier::from_decimal_str(multiplier).expect("multiplier");
        let usage = UsageMetrics {
            input_tokens: *input_tokens,
            output_tokens: *output_tokens,
            cache_read_input_tokens: *cache_read_tokens,
            cache_creation_input_tokens: *cache_creation_tokens,
            ..UsageMetrics::default()
        };
        let cost = estimate_usage_cost_with_accounting(
            &usage,
            &price,
            CostAdjustments {
                service_tier_multiplier: None,
                provider_multiplier: Some(multiplier),
            },
            CacheInputAccounting::DirectReadSeparate,
        );
        assert_eq!(cost.confidence, CostConfidence::Estimated);
        let actual = cost.total_cost_femto_usd().expect("priced row");
        let expected = UsdAmount::from_decimal_str(expected_cost)
            .expect("expected cost")
            .femto_usd();
        assert_eq!(actual, expected, "row on {day}");
        *daily.entry(day.clone()).or_default() += actual;
        total += actual;
    }

    for (day, expected) in fixture.expected_days {
        let expected = UsdAmount::from_decimal_str(&expected).expect("expected daily total");
        assert_eq!(
            *daily.get(&day).expect("day"),
            expected.femto_usd(),
            "daily total {day}"
        );
    }
    let expected_total =
        UsdAmount::from_decimal_str(&fixture.expected_total).expect("expected total");
    assert_eq!(total, expected_total.femto_usd());
}
