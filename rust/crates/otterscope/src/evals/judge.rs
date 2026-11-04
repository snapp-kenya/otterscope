use std::time::Duration;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::model::{Run, Step};

use super::assert::{Assertion, Result, final_output};

/// Parsed config of an `llm_judge` assertion. Endpoint and API key are
/// deliberately not part of it: assertions are writable through the API, so
/// naming a URL or environment variable would allow SSRF and secret
/// exfiltration (audit finding #48).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct JudgeConfig {
    pub prompt: String,
    pub model: String,
    /// 0..1; 0 means 1.0 (all).
    pub sample_rate: f64,
}

/// Server-configured judge target: base URL, key, and the operator's
/// allow-list of models an assertion may name.
#[derive(Clone, Default)]
pub struct Endpoint {
    pub base_url: String,
    pub key: String,
    /// Empty means no model is allowed: judge assertions are refused rather
    /// than silently billed.
    pub models: Vec<String>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base_url", &self.base_url)
            .field("key", &crate::redacted(&self.key))
            .field("models", &self.models)
            .finish()
    }
}

impl Endpoint {
    /// Case- and surrounding-space-insensitive, because provider model ids
    /// differ only in case across their own docs and SDKs.
    pub fn allows_model(&self, model: &str) -> bool {
        let wanted = model.trim().to_lowercase();
        self.models
            .iter()
            .any(|allowed| allowed.trim().to_lowercase() == wanted)
    }

    /// Names both the refused model and the allow-list, so a caller can see
    /// what was rejected and what would be accepted.
    pub fn check_model(&self, model: &str) -> anyhow::Result<()> {
        if self.allows_model(model) {
            return Ok(());
        }
        let allowed = if self.models.is_empty() {
            "none configured (set OTTERSCOPE_JUDGE_MODELS or judgeModels)".to_string()
        } else {
            self.models.join(", ")
        };
        bail!("judge model {model:?} is not in the judge model allow-list [{allowed}]")
    }
}

impl Assertion {
    /// The model an `llm_judge` assertion names, or `""` for any other type.
    /// A malformed config is reported so the caller can refuse rather than
    /// evaluate.
    pub fn judge_model(&self) -> anyhow::Result<String> {
        if !matches!(self.kind(), Ok(super::AssertionKind::LlmJudge)) {
            return Ok(String::new());
        }
        Ok(parse_judge_config(&self.config)?.model)
    }
}

pub fn parse_judge_config(raw: &str) -> anyhow::Result<JudgeConfig> {
    let c: JudgeConfig = serde_json::from_str(raw).map_err(|e| {
        anyhow::anyhow!(
            "llm_judge config invalid (only prompt, model, sampleRate are allowed): {e}"
        )
    })?;
    if c.prompt.is_empty() || c.model.is_empty() {
        bail!("llm_judge config needs prompt and model");
    }
    if c.sample_rate < 0.0 || c.sample_rate > 1.0 {
        bail!("sampleRate must be within 0..1");
    }
    Ok(c)
}

/// Scores a run with an LLM via the server-configured OpenAI-compatible
/// endpoint. The verdict must start with PASS or FAIL; the rest becomes the
/// detail.
pub async fn judge(ep: &Endpoint, a: &Assertion, _run: &Run, steps: &[Step]) -> Result {
    let mut res = Result::for_assertion(a);
    let cfg = match parse_judge_config(&a.config) {
        Ok(c) => c,
        Err(e) => {
            res.detail = e.to_string();
            return res;
        }
    };
    if ep.key.is_empty() {
        res.detail = "judge skipped: OTTERSCOPE_JUDGE_KEY (or OPENAI_API_KEY) is not set".into();
        return res;
    }
    if let Err(e) = ep.check_model(&cfg.model) {
        res.detail = e.to_string();
        return res;
    }

    let input = first_user_input(steps);
    let output = final_output(steps);
    let body = serde_json::json!({
        "model": cfg.model,
        "messages": [
            {
                "role": "system",
                "content": format!(
                    "{}\n\nRespond with a single line starting with PASS or FAIL, followed by a brief reason.",
                    cfg.prompt
                ),
            },
            {
                "role": "user",
                "content": format!("Agent input:\n{input}\n\nAgent output:\n{output}"),
            },
        ],
        "max_tokens": 300,
    });

    let url = format!("{}/chat/completions", ep.base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(&url)
        .timeout(Duration::from_secs(60))
        .bearer_auth(&ep.key)
        .json(&body)
        .send()
        .await;
    let mut response = match response {
        Ok(r) => r,
        Err(e) => {
            res.detail = format!("judge call failed: {e}");
            return res;
        }
    };

    let status = response.status();
    let raw = read_limited(&mut response, 1 << 20).await;
    if status != reqwest::StatusCode::OK {
        res.detail = format!(
            "judge endpoint {}: {}",
            status.as_u16(),
            truncate(&raw, 200)
        );
        return res;
    }

    #[derive(Deserialize)]
    struct ChatResponse {
        #[serde(default)]
        choices: Option<Vec<Choice>>,
    }
    #[derive(Deserialize)]
    struct Choice {
        #[serde(default)]
        message: Option<ChatMessage>,
    }
    #[derive(Default, Deserialize)]
    struct ChatMessage {
        #[serde(default)]
        content: Option<String>,
    }

    let parsed: ChatResponse = match serde_json::from_slice(&raw) {
        Ok(p) => p,
        Err(_) => {
            res.detail = "judge returned an unparseable response".into();
            return res;
        }
    };
    let Some(choices) = parsed.choices else {
        res.detail = "judge returned an unparseable response".into();
        return res;
    };
    let Some(choice) = choices.into_iter().next() else {
        res.detail = "judge returned an unparseable response".into();
        return res;
    };
    let verdict = choice
        .message
        .and_then(|m| m.content)
        .unwrap_or_default()
        .trim()
        .to_string();
    res.pass = verdict.to_uppercase().starts_with("PASS");
    res.detail = verdict;
    res
}

/// First LLM step's input messages, for judge context.
fn first_user_input(steps: &[Step]) -> String {
    for step in steps {
        let Some(llm) = &step.llm else { continue };
        if llm.input_messages.is_empty() {
            continue;
        }
        return llm
            .input_messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");
    }
    "(no input recorded)".into()
}

async fn read_limited(response: &mut reqwest::Response, limit: usize) -> Vec<u8> {
    let mut raw = Vec::new();
    while raw.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = limit - raw.len();
                raw.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    raw
}

fn truncate(raw: &[u8], max: usize) -> String {
    let end = raw.len().min(max);
    String::from_utf8_lossy(&raw[..end]).into_owned()
}
