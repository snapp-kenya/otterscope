use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::LlmCall;

/// USD per **million** tokens. Zero-valued cache rates mean the provider bills
/// those tokens at the plain input rate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rate {
    pub input: f64,
    pub output: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_read: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_write: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Resolves model IDs to rates by longest-prefix match, case-insensitive, so
/// `claude-sonnet-5` matches `claude-sonnet-5-20260301`.
#[derive(Debug, Clone)]
pub struct Table {
    /// Key: lowercase model-ID prefix.
    rates: HashMap<String, Rate>,
}

impl Default for Table {
    fn default() -> Self {
        let mut rates = HashMap::with_capacity(DEFAULT_RATES.len());
        for (prefix, rate) in DEFAULT_RATES {
            rates.insert(prefix.to_lowercase(), *rate);
        }
        Self { rates }
    }
}

impl Table {
    /// A table with no rates, for callers that want to supply their own.
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self {
            rates: HashMap::new(),
        }
    }

    /// Overlays user-supplied rates: `{"model-prefix": {"input": 3, ...}}`.
    /// An override replaces the whole rate, it does not field-merge.
    pub fn merge_json(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let overrides: Option<HashMap<String, Rate>> =
            serde_json::from_slice(data).map_err(|e| anyhow::anyhow!("pricing overrides: {e}"))?;
        for (prefix, rate) in overrides.unwrap_or_default() {
            self.rates.insert(prefix.to_lowercase(), rate);
        }
        Ok(())
    }

    /// The rate whose prefix is the longest match for `model`.
    pub fn lookup(&self, model: &str) -> Option<Rate> {
        let model = model.to_lowercase();
        self.rates
            .iter()
            .filter(|(prefix, _)| model.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, rate)| *rate)
    }

    /// The USD cost of one call, or `None` for unknown models.
    ///
    /// `cache_read` and `cache_write` are subsets of `input` under the OTel
    /// GenAI convention; the remainder is billed at the plain input rate.
    pub fn cost(
        &self,
        model: &str,
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
    ) -> Option<f64> {
        let rate = self.lookup(model)?;
        let cache_read_rate = if rate.cache_read == 0.0 {
            rate.input
        } else {
            rate.cache_read
        };
        let cache_write_rate = if rate.cache_write == 0.0 {
            rate.input
        } else {
            rate.cache_write
        };
        // Some emitters report input_tokens already net of cache tokens. A
        // negative remainder means the cache tokens are additive, not a
        // subset; clamping to zero instead would undercount the call. Token
        // counts arrive from OTLP attributes, so the subtractions are checked:
        // an out-of-range value falls back to the additive reading rather than
        // wrapping.
        let plain = input
            .checked_sub(cache_read)
            .and_then(|plain| plain.checked_sub(cache_write))
            .filter(|plain| *plain >= 0)
            .unwrap_or(input);
        let usd = (plain as f64 * rate.input
            + cache_read as f64 * cache_read_rate
            + cache_write as f64 * cache_write_rate
            + output as f64 * rate.output)
            / 1e6;
        Some(usd)
    }
}

/// Cost of one LLM call, when the model is known.
///
/// Reasoning tokens are a subset of `output_tokens`, so they are already
/// billed at the output rate and must not be added again.
pub fn cost(table: &Table, call: &LlmCall) -> Option<f64> {
    if call.input_tokens == 0 && call.output_tokens == 0 {
        return None;
    }
    let model = if call.response_model.is_empty() {
        &call.request_model
    } else {
        &call.response_model
    };
    if model.is_empty() {
        return None;
    }
    table.cost(
        model,
        call.input_tokens,
        call.output_tokens,
        call.cache_read_tokens,
        call.cache_creation_tokens,
    )
}

/// USD per million tokens, keyed by model-ID prefix. Verified against official
/// provider pricing pages on 2026-07-16. Tiered prices use the base tier
/// (long-context premiums above 200k prompt tokens on Gemini Pro / Grok are not
/// modeled); cache writes use the provider's default 5-minute tier.
const DEFAULT_RATES: &[(&str, Rate)] = &[
    // Anthropic — platform.claude.com/docs/en/about-claude/pricing
    (
        "claude-fable-5",
        Rate {
            input: 10.0,
            output: 50.0,
            cache_read: 1.0,
            cache_write: 12.5,
        },
    ),
    (
        "claude-mythos-5",
        Rate {
            input: 10.0,
            output: 50.0,
            cache_read: 1.0,
            cache_write: 12.5,
        },
    ),
    // Opus 5 and the 4.x Opus line are priced identically, but Opus 5 needs
    // its own key: longest-prefix matching cannot reach "claude-opus-4" from
    // "claude-opus-5", and Claude Code defaults to Opus.
    (
        "claude-opus-5",
        Rate {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
        },
    ),
    (
        "claude-opus-4",
        Rate {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
        },
    ),
    // Introductory pricing through 2026-08-31; standard becomes 3/15
    // (cacheRead 0.30, cacheWrite 3.75) on 2026-09-01 — update then.
    (
        "claude-sonnet-5",
        Rate {
            input: 2.0,
            output: 10.0,
            cache_read: 0.2,
            cache_write: 2.5,
        },
    ),
    (
        "claude-sonnet-4-6",
        Rate {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        },
    ),
    (
        "claude-sonnet-4-5",
        Rate {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        },
    ),
    (
        "claude-haiku-4-5",
        Rate {
            input: 1.0,
            output: 5.0,
            cache_read: 0.1,
            cache_write: 1.25,
        },
    ),
    // OpenAI — developers.openai.com/api/docs/pricing
    // (cache-write pricing unverified; reads-only discount modeled)
    (
        "gpt-5.6-sol",
        Rate {
            input: 5.0,
            output: 30.0,
            cache_read: 0.5,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.6-terra",
        Rate {
            input: 2.5,
            output: 15.0,
            cache_read: 0.25,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.6-luna",
        Rate {
            input: 1.0,
            output: 6.0,
            cache_read: 0.1,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.5-pro",
        Rate {
            input: 30.0,
            output: 180.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.5",
        Rate {
            input: 5.0,
            output: 30.0,
            cache_read: 0.5,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.4-pro",
        Rate {
            input: 30.0,
            output: 180.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.4-mini",
        Rate {
            input: 0.75,
            output: 4.5,
            cache_read: 0.075,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.4-nano",
        Rate {
            input: 0.2,
            output: 1.25,
            cache_read: 0.02,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.4",
        Rate {
            input: 2.5,
            output: 15.0,
            cache_read: 0.25,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.3-codex",
        Rate {
            input: 1.75,
            output: 14.0,
            cache_read: 0.175,
            cache_write: 0.0,
        },
    ),
    // Google — ai.google.dev/gemini-api/docs/pricing (base <=200k tier;
    // per-hour cache storage not modeled)
    (
        "gemini-3.5-flash",
        Rate {
            input: 1.5,
            output: 9.0,
            cache_read: 0.15,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-3.1-pro",
        Rate {
            input: 2.0,
            output: 12.0,
            cache_read: 0.2,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-3.1-flash-lite",
        Rate {
            input: 0.25,
            output: 1.5,
            cache_read: 0.025,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-3-flash",
        Rate {
            input: 0.5,
            output: 3.0,
            cache_read: 0.05,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-pro",
        Rate {
            input: 1.25,
            output: 10.0,
            cache_read: 0.125,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-flash-lite",
        Rate {
            input: 0.1,
            output: 0.4,
            cache_read: 0.01,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-flash",
        Rate {
            input: 0.3,
            output: 2.5,
            cache_read: 0.03,
            cache_write: 0.0,
        },
    ),
    // DeepSeek — api-docs.deepseek.com (deepseek-chat/reasoner are legacy
    // aliases of v4-flash, deprecated 2026-07-24)
    (
        "deepseek-v4-flash",
        Rate {
            input: 0.14,
            output: 0.28,
            cache_read: 0.0028,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-v4-pro",
        Rate {
            input: 0.435,
            output: 0.87,
            cache_read: 0.003625,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-chat",
        Rate {
            input: 0.14,
            output: 0.28,
            cache_read: 0.0028,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-reasoner",
        Rate {
            input: 0.14,
            output: 0.28,
            cache_read: 0.0028,
            cache_write: 0.0,
        },
    ),
    // Mistral — mistral.ai/pricing/api
    (
        "mistral-medium",
        Rate {
            input: 1.5,
            output: 7.5,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "mistral-large",
        Rate {
            input: 0.5,
            output: 1.5,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "mistral-small",
        Rate {
            input: 0.15,
            output: 0.6,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "magistral-medium",
        Rate {
            input: 2.0,
            output: 5.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "codestral",
        Rate {
            input: 0.3,
            output: 0.9,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "devstral-medium",
        Rate {
            input: 0.4,
            output: 2.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // xAI — docs.x.ai/docs/models (base <=200k tier)
    (
        "grok-4.5",
        Rate {
            input: 2.0,
            output: 6.0,
            cache_read: 0.5,
            cache_write: 0.0,
        },
    ),
    (
        "grok-4.3",
        Rate {
            input: 1.25,
            output: 2.5,
            cache_read: 0.2,
            cache_write: 0.0,
        },
    ),
    (
        "grok-4",
        Rate {
            input: 1.25,
            output: 2.5,
            cache_read: 0.2,
            cache_write: 0.0,
        },
    ),
    // Meta Llama via Groq — groq.com/pricing
    (
        "llama-3.3-70b-versatile",
        Rate {
            input: 0.59,
            output: 0.79,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "llama-3.1-8b-instant",
        Rate {
            input: 0.05,
            output: 0.08,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "openai/gpt-oss-120b",
        Rate {
            input: 0.15,
            output: 0.6,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
];
