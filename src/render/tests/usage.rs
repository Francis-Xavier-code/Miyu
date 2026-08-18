//! token 用量显示。

use crate::render::*;

#[test]
fn token_usage_hides_zero_turn_tokens() {
    assert_eq!(
        format_token_usage_inline(&TokenMeter {
            session_tokens: 1_300,
            context_window: Some(272_000),
            ..Default::default()
        }),
        "1.3k/272k(0.5%)"
    );
    assert_eq!(
        format_token_usage_inline(&TokenMeter {
            turn_tokens: 1_300,
            session_tokens: 1_300,
            context_window: Some(272_000),
            ..Default::default()
        }),
        "1.3k · 1.3k/272k(0.5%)"
    );
    assert_eq!(
        format_token_usage_inline(&TokenMeter {
            turn_tokens: 5_300,
            session_tokens: 10_000,
            context_window: Some(200_000),
            cumulative_tokens: Some(86_200),
            ..Default::default()
        }),
        "5.3k · 10k/200k(5.0%) · Σ86.2k"
    );
}

#[test]
fn a_cache_rate_divides_by_the_prompt_not_the_whole_turn() {
    // 24.8k turn = 12.0k prompt + 12.8k output, 11.2k of the prompt cached.
    // Dividing by the turn total would report 45% and would sag further the
    // longer the model talked, which says nothing about the cache.
    let meter = TokenMeter {
        turn_tokens: 24_800,
        turn_prompt_tokens: 12_000,
        turn_cached_tokens: 11_200,
        session_tokens: 12_000,
        context_window: Some(200_000),
        cumulative_tokens: Some(380_000),
        cumulative_prompt_tokens: 248_000,
        cumulative_cached_tokens: 226_000,
    };
    assert_eq!(
        format_token_usage_inline(&meter),
        "24.8k(C93%) · 12k/200k(6.0%) · Σ380k(C91%)"
    );
}

#[test]
fn a_provider_that_reports_no_cache_shows_no_rate() {
    // Turns recorded before the cache columns existed read as zeros; a flat
    // "C0%" would be a claim the database cannot support.
    let meter = TokenMeter {
        turn_tokens: 5_300,
        turn_prompt_tokens: 4_000,
        session_tokens: 10_000,
        context_window: Some(200_000),
        cumulative_tokens: Some(86_200),
        cumulative_prompt_tokens: 70_000,
        ..Default::default()
    };
    assert_eq!(
        format_token_usage_inline(&meter),
        "5.3k · 10k/200k(5.0%) · Σ86.2k"
    );
}
