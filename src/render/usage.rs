//! token 用量的计量与显示。
//!
//! 缓存命中率（`cache_percent`）是这里最该显眼的数字——命中的 token 只按十分
//! 之一计价，掉下来就是账单翻十倍，而功能上一点症状都没有。

use crate::render::*;

/// Everything the token meters show. Grouped into one struct because the two
/// cache rates each need a numerator *and* a denominator, and threading eight
/// loose `u64`s through four call layers was already past readable.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokenMeter {
    pub turn_tokens: u64,
    /// Denominator of the turn cache rate. A cache hit is an input-side
    /// property — output tokens only enter the prompt on the *next* turn — so
    /// the rate is read/prompt, never read/total, which is what every provider
    /// reports too (DeepSeek splits the prompt into hit+miss; OpenAI's
    /// `cached_tokens` is a subset of `prompt_tokens`; Anthropic names all
    /// three fields `*_input_tokens`).
    pub turn_prompt_tokens: u64,
    pub turn_cached_tokens: u64,
    pub session_tokens: u64,
    pub context_window: Option<usize>,
    /// Σ: session-lifetime total. `None` hides it on narrow terminals.
    pub cumulative_tokens: Option<u64>,
    pub cumulative_prompt_tokens: u64,
    pub cumulative_cached_tokens: u64,
}

/// `None` when there is nothing honest to report: a provider that never said
/// anything about caching must not be rendered as a flat 0%.
pub(crate) fn cache_percent(cached: u64, prompt: u64) -> Option<u64> {
    (cached > 0 && prompt > 0)
        .then(|| ((cached as f64 / prompt as f64) * 100.0).round().min(100.0) as u64)
}

pub(crate) fn cache_suffix(cached: u64, prompt: u64) -> String {
    cache_percent(cached, prompt)
        .map(|percent| format!("(C{percent}%)"))
        .unwrap_or_default()
}

pub fn print_token_usage(meter: &TokenMeter, estimated: bool) -> Result<()> {
    let output = token_usage_output(meter, estimated);
    let mut stdout = io::stdout();
    write!(stdout, "{output}")?;
    stdout.flush()?;
    Ok(())
}

pub(crate) fn token_usage_output(meter: &TokenMeter, estimated: bool) -> String {
    let prefix = if estimated {
        t("Estimated ", "估算")
    } else {
        ""
    };
    let line = format!("{prefix}Token: {}", format_token_usage_inline(meter));
    format!("\x1b[2m{line}\x1b[0m\n\n")
}

pub(crate) fn format_token_usage_inline(meter: &TokenMeter) -> String {
    format_token_usage_inline_opts(meter, true)
}

pub(crate) fn format_token_usage_inline_opts(meter: &TokenMeter, show_percent: bool) -> String {
    let context_window = meter.context_window.map(|value| value as u64);
    let context = context_window
        .map(format_compact_count)
        .unwrap_or_else(|| "?".to_string());
    let usage_ratio = if let Some(context_window) = context_window.filter(|value| *value > 0) {
        format!(
            "{:.1}%",
            meter.session_tokens as f64 / context_window as f64 * 100.0
        )
    } else {
        "?".to_string()
    };

    let mut session = if show_percent {
        format!(
            "{}/{}({usage_ratio})",
            format_compact_count(meter.session_tokens),
            context,
        )
    } else {
        format!("{}/{}", format_compact_count(meter.session_tokens), context)
    };
    if let Some(cumulative_tokens) = meter.cumulative_tokens {
        session.push_str(&format!(
            " · Σ{}{}",
            format_compact_count(cumulative_tokens),
            cache_suffix(
                meter.cumulative_cached_tokens,
                meter.cumulative_prompt_tokens
            ),
        ));
    }
    if meter.turn_tokens == 0 {
        session
    } else {
        format!(
            "{}{} · {session}",
            format_compact_count(meter.turn_tokens),
            cache_suffix(meter.turn_cached_tokens, meter.turn_prompt_tokens),
        )
    }
}

pub fn usage_total(usage: &Usage) -> u64 {
    usage.effective_total_tokens()
}

pub(crate) fn format_compact_count(value: u64) -> String {
    const K: f64 = 1_000.0;
    const M: f64 = 1_000_000.0;
    if value >= 1_000_000 {
        format_compact_unit(value as f64 / M, "M")
    } else if value >= 1_000 {
        format_compact_unit(value as f64 / K, "k")
    } else {
        value.to_string()
    }
}

pub(crate) fn format_compact_unit(value: f64, suffix: &str) -> String {
    if (value.fract() - 0.0).abs() < f64::EPSILON {
        format!("{value:.0}{suffix}")
    } else {
        format!("{value:.1}{suffix}")
    }
}
