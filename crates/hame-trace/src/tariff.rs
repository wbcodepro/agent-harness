//! Model prices, dated.
//!
//! Prices are US dollars per million tokens, first-party API, standard
//! speed, no batch discount and no data-residency multiplier. A model that
//! is not in the table has no price: its cost is `None`, never zero.

use hame_session::Usage;
use serde_json::Value;

/// The day the table below was checked against the source.
pub const TARIFF_DATE: &str = "2026-11-15";
pub const TARIFF_SOURCE: &str = "https://platform.claude.com/docs/en/about-claude/pricing";

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Uncached input.
    pub input: f64,
    /// Cache writes with the default five-minute lifetime.
    pub cache_write_5m: f64,
    /// Cache writes with a one-hour lifetime.
    pub cache_write_1h: f64,
    /// Cache hits and refreshes.
    pub cache_read: f64,
    pub output: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tariff {
    /// The model id, without a date suffix.
    pub model: String,
    pub rates: Rates,
    /// A higher price for requests whose prompt (uncached input plus cache
    /// reads and writes) is longer than the threshold.
    pub long_prompt: Option<(u64, Rates)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TariffTable {
    pub date: String,
    pub source: String,
    pub entries: Vec<Tariff>,
}

const fn rates(input: f64, w5: f64, w1h: f64, read: f64, output: f64) -> Rates {
    Rates {
        input,
        cache_write_5m: w5,
        cache_write_1h: w1h,
        cache_read: read,
        output,
    }
}

/// Anthropic's published prices on [`TARIFF_DATE`], from [`TARIFF_SOURCE`].
const ANTHROPIC: &[(&str, Rates)] = &[
    ("claude-fable-5-2", rates(8.0, 10.0, 16.0, 0.20, 40.0)),
    ("claude-mythos-5-2", rates(8.0, 10.0, 16.0, 0.20, 40.0)),
    ("claude-fable-5-1", rates(10.0, 12.5, 20.0, 0.25, 50.0)),
    ("claude-mythos-5-1", rates(10.0, 12.5, 20.0, 0.25, 50.0)),
    ("claude-fable-5", rates(10.0, 12.5, 20.0, 1.0, 50.0)),
    ("claude-mythos-5", rates(10.0, 12.5, 20.0, 1.0, 50.0)),
    ("claude-opus-5-5", rates(4.0, 5.0, 8.0, 0.20, 20.0)),
    ("claude-opus-5", rates(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("claude-opus-4-8", rates(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("claude-opus-4-7", rates(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("claude-opus-4-6", rates(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("claude-opus-4-5", rates(5.0, 6.25, 10.0, 0.50, 25.0)),
    ("claude-opus-4-1", rates(15.0, 18.75, 30.0, 1.50, 75.0)),
    ("claude-opus-4", rates(15.0, 18.75, 30.0, 1.50, 75.0)),
    ("claude-sonnet-5-5", rates(2.0, 2.5, 4.0, 0.10, 10.0)),
    ("claude-sonnet-5", rates(2.0, 2.5, 4.0, 0.20, 10.0)),
    ("claude-sonnet-4-6", rates(3.0, 3.75, 6.0, 0.30, 15.0)),
    ("claude-sonnet-4-5", rates(3.0, 3.75, 6.0, 0.30, 15.0)),
    ("claude-sonnet-4", rates(3.0, 3.75, 6.0, 0.30, 15.0)),
    ("claude-haiku-5-5", rates(0.10, 0.125, 0.20, 0.01, 0.50)),
    ("claude-haiku-4-5", rates(1.0, 1.25, 2.0, 0.10, 5.0)),
];

/// Claude Haiku 5.5 costs more for prompts over 100,000 tokens.
const HAIKU_5_5_LONG: (u64, Rates) = (100_000, rates(0.50, 0.625, 1.0, 0.05, 2.50));

impl Default for TariffTable {
    fn default() -> Self {
        Self::anthropic()
    }
}

impl TariffTable {
    pub fn anthropic() -> Self {
        Self {
            date: TARIFF_DATE.into(),
            source: TARIFF_SOURCE.into(),
            entries: ANTHROPIC
                .iter()
                .map(|(model, rates)| Tariff {
                    model: (*model).into(),
                    rates: *rates,
                    long_prompt: (*model == "claude-haiku-5-5").then_some(HAIKU_5_5_LONG),
                })
                .collect(),
        }
    }

    /// The tariff for `model`: an exact id, or an id plus a `-YYYYMMDD`
    /// snapshot date. `claude-opus-5` does not match `claude-opus-5-5`.
    pub fn lookup(&self, model: &str) -> Option<&Tariff> {
        let model = model.strip_prefix("anthropic/").unwrap_or(model);
        self.entries.iter().find(|tariff| {
            model == tariff.model
                || model
                    .strip_prefix(tariff.model.as_str())
                    .is_some_and(|rest| {
                        rest.len() == 9
                            && rest.starts_with('-')
                            && rest[1..].bytes().all(|b| b.is_ascii_digit())
                    })
        })
    }

    /// What one request cost, or `None` when the model has no price.
    ///
    /// Cache writes are priced at the five-minute rate unless the raw
    /// provider usage breaks them down by lifetime
    /// (`cache_creation.ephemeral_1h_input_tokens`).
    pub fn cost(&self, model: &str, usage: &Usage, raw: Option<&Value>) -> Option<f64> {
        let tariff = self.lookup(model)?;
        let prompt = usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
        let rates = match tariff.long_prompt {
            Some((threshold, long)) if prompt > threshold => long,
            _ => tariff.rates,
        };
        let one_hour = raw
            .and_then(|raw| raw["cache_creation"]["ephemeral_1h_input_tokens"].as_u64())
            .unwrap_or(0)
            .min(usage.cache_write_tokens);
        let five_minute = usage.cache_write_tokens - one_hour;
        let dollars = usage.input_tokens as f64 * rates.input
            + five_minute as f64 * rates.cache_write_5m
            + one_hour as f64 * rates.cache_write_1h
            + usage.cache_read_tokens as f64 * rates.cache_read
            + usage.output_tokens as f64 * rates.output;
        Some(dollars / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usage(input: u64, read: u64, write: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: read,
            cache_write_tokens: write,
        }
    }

    #[test]
    fn ids_match_exactly_or_with_a_snapshot_date() {
        let table = TariffTable::anthropic();
        assert_eq!(table.lookup("claude-opus-5").unwrap().rates.input, 5.0);
        assert_eq!(table.lookup("claude-opus-5-5").unwrap().rates.input, 4.0);
        assert_eq!(
            table
                .lookup("claude-haiku-4-5-20251001")
                .unwrap()
                .rates
                .input,
            1.0
        );
        assert!(table.lookup("claude-opus-5-5-x").is_none());
        assert!(table.lookup("gpt-4o-mini").is_none());
        assert!(table.lookup("deepseek-flash").is_none());
    }

    #[test]
    fn unknown_models_have_no_cost_not_zero() {
        let table = TariffTable::anthropic();
        assert_eq!(
            table.cost("mystery-model", &usage(10, 0, 0, 10), None),
            None
        );
    }

    #[test]
    fn each_token_class_is_priced_at_its_own_rate() {
        let table = TariffTable::anthropic();
        // 1M of each class on Opus 5.5: 4 + 5 + 0.20 + 20.
        let cost = table
            .cost(
                "claude-opus-5-5",
                &usage(1_000_000, 1_000_000, 1_000_000, 1_000_000),
                None,
            )
            .unwrap();
        assert!((cost - 29.2).abs() < 1e-9, "{cost}");
        // Half the writes were one-hour writes.
        let raw = json!({"cache_creation": {"ephemeral_1h_input_tokens": 500_000}});
        let cost = table
            .cost("claude-opus-5-5", &usage(0, 0, 1_000_000, 0), Some(&raw))
            .unwrap();
        assert!((cost - (2.5 + 4.0)).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn haiku_5_5_has_a_long_prompt_price() {
        let table = TariffTable::anthropic();
        let short = table
            .cost("claude-haiku-5-5", &usage(100_000, 0, 0, 1_000_000), None)
            .unwrap();
        assert!((short - (0.01 + 0.50)).abs() < 1e-9, "{short}");
        // 60k uncached + 50k from cache is a 110k prompt.
        let long = table
            .cost("claude-haiku-5-5", &usage(60_000, 50_000, 0, 0), None)
            .unwrap();
        assert!((long - (0.06 * 0.50 + 0.05 * 0.05)).abs() < 1e-9, "{long}");
    }
}
