//! Claude Agent SDK / Claude Code dialect. The CLI emits its own
//! `claude_code.*` span names with mostly bare attribute keys (`model`,
//! `input_tokens`, `tool_name`) rather than the gen_ai.* conventions — only a
//! handful of gen_ai.* aliases are duplicated alongside them, and
//! `gen_ai.operation.name` is not among them. Without this handler every Agent
//! SDK span falls through to a generic step: no model calls, no tokens, no
//! cost, no tool loop.
//!
//! Tracing is explicitly beta upstream ("span names and attributes may change
//! between releases"), which is exactly the churn this module isolates.

use crate::model::{LlmCall, Status, Step, StepKind, ToolCall};

use super::attributes::Attrs;
use super::proto;

pub(crate) fn is_claude_code(span: &proto::Span) -> bool {
    span.name.starts_with("claude_code.")
}

pub(crate) fn apply(span: &proto::Span, attrs: &Attrs, step: &mut Step) {
    // A subagent's spans carry the type it was spawned as; surfacing it makes
    // the delegation chain readable in the run list.
    if step.agent_name.is_empty() {
        step.agent_name = attrs.string(&["subagent_type"]);
    }

    match span.name.as_str() {
        "claude_code.llm_request" => {
            step.kind = StepKind::Llm;
            let mut llm = LlmCall {
                // gen_ai.system is duplicated by the CLI and is always
                // "anthropic"; keep the bare-key fallbacks for older/newer
                // builds.
                provider: attrs.string(&["gen_ai.system", "gen_ai.provider.name"]),
                request_model: attrs.string(&["gen_ai.request.model", "model"]),
                input_tokens: attrs.int(&["input_tokens"]),
                output_tokens: attrs.int(&["output_tokens"]),
                cache_read_tokens: attrs.int(&["cache_read_tokens"]),
                cache_creation_tokens: attrs.int(&["cache_creation_tokens"]),
                ..LlmCall::default()
            };
            // No response model is emitted; pricing falls back to the request
            // model, which is what was billed here anyway.
            if llm.provider.is_empty() {
                llm.provider = "anthropic".to_string();
            }
            step.llm = Some(llm);
            apply_failure(attrs, step);
        }
        "claude_code.tool" => {
            step.kind = StepKind::Tool;
            let mut tool = ToolCall {
                name: attrs.string(&["tool_name"]),
                call_id: attrs.string(&["gen_ai.tool.call.id", "tool_use_id"]),
                arguments: tool_args(attrs),
                result: String::new(),
            };
            if tool.name.is_empty() {
                tool.name = span.name.clone();
            }
            // The output body arrives as a span event, not an attribute, and
            // only under OTEL_LOG_TOOL_CONTENT=1.
            if let Some(result) = tool_result(span) {
                tool.result = result;
            }
            step.tool = Some(tool);
        }
        "claude_code.tool.execution" => {
            // Timing detail under the tool span. Kept generic so it does not
            // double-count as a second tool call, but its failure is the one
            // that actually says the tool broke.
            step.kind = StepKind::Generic;
            apply_failure(attrs, step);
        }
        _ => {
            // claude_code.interaction (the run root), .tool.blocked_on_user,
            // .hook, and anything added by a later release.
            step.kind = if span.name == "claude_code.interaction" {
                StepKind::Agent
            } else {
                StepKind::Generic
            };
        }
    }
}

/// Mark a step failed from the CLI's own attributes. The span status is left
/// unset even on a failed request, so span status alone is not enough.
///
/// Upstream documents `success` as the string "true"/"false"; on the wire it
/// is a bool. `as_string` renders both as "true"/"false", so this comparison
/// holds either way — do not "simplify" it to a bool accessor.
fn apply_failure(attrs: &Attrs, step: &mut Step) {
    if attrs.string(&["success"]) != "false" {
        return;
    }
    step.status = Status::Error;
    if step.error.is_empty() {
        step.error = attrs.string(&["error"]);
    }
    if step.error.is_empty() {
        let code = attrs.string(&["status_code"]);
        step.error = if code.is_empty() {
            "error".to_string()
        } else {
            format!("request failed with status {code}")
        };
    }
}

/// Summarize what a tool was asked to do from the documented per-tool
/// attributes (all gated behind OTEL_LOG_TOOL_DETAILS=1).
fn tool_args(attrs: &Attrs) -> String {
    for key in ["full_command", "file_path", "skill_name", "subagent_type"] {
        let value = attrs.string(&[key]);
        if !value.is_empty() {
            return value;
        }
    }
    String::new()
}

/// Pull the tool's output body from the `tool.output` span event emitted under
/// OTEL_LOG_TOOL_CONTENT=1.
///
/// The upstream reference names the event but not its attributes. A real
/// capture (Claude Code 2.1.241, Read tool) carries them as `content` for the
/// body plus an echo of the tool's own input attributes — `file_path` here —
/// so there is no key called "output" to look for. `content` is the body for
/// every tool; the suffix match is a hedge for a rename in a beta dialect.
fn tool_result(span: &proto::Span) -> Option<String> {
    for event in &span.events {
        if event.name != "tool.output" {
            continue;
        }
        let attrs = Attrs::new(&event.attributes);
        let content = attrs.string(&["content"]);
        if !content.is_empty() {
            return Some(content);
        }
        for (key, value) in attrs.iter() {
            let key = key.to_lowercase();
            if key.contains("output") || key.contains("result") {
                let value = value.as_string();
                if !value.is_empty() {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::ingest::json;
    use crate::ingest::normalize::normalize;
    use crate::ingest::testutil;
    use crate::model::Status;
    use crate::model::StepKind;

    // Span IDs from the captured fixture (Claude Code 2.1.241, one headless
    // run that read a file and answered).
    const INTERACTION: &str = "99483a068ece8574";
    const LLM_TOOL_USE: &str = "29903f83637c6d1c";
    const TOOL: &str = "1bf15d19445acb22";
    const BLOCKED: &str = "9346a69ba1a118c9";
    const EXECUTION: &str = "7dd0b69d8515823e";
    const LLM_FINAL: &str = "8fc6f67c996714d4";

    fn steps() -> std::collections::HashMap<String, crate::model::Step> {
        let request = testutil::request_from_fixture("claude_agent_sdk.json");
        normalize(&request)
            .into_iter()
            .map(|step| (step.id.clone(), step))
            .collect()
    }

    #[test]
    fn claude_agent_sdk_fixture() {
        let steps = steps();
        assert_eq!(steps.len(), 6);

        let interaction = &steps[INTERACTION];
        assert_eq!(interaction.kind, StepKind::Agent);
        assert_eq!(interaction.parent_id, "");
        assert_eq!(interaction.service, "capture-probe");

        let llm = &steps[LLM_TOOL_USE];
        assert_eq!(llm.kind, StepKind::Llm);
        let call = llm.llm.as_ref().unwrap();
        assert_eq!(call.provider, "anthropic");
        assert_eq!(call.request_model, "claude-opus-5[1m]");
        assert_eq!((call.input_tokens, call.output_tokens), (2, 110));
        assert_eq!(
            (call.cache_read_tokens, call.cache_creation_tokens),
            (16024, 15181)
        );
        assert_eq!(llm.status, Status::Ok);

        let tool = &steps[TOOL];
        assert_eq!(tool.kind, StepKind::Tool);
        let call = tool.tool.as_ref().unwrap();
        assert_eq!(call.name, "Read");
        assert_eq!(call.call_id, "toolu_01A7oeJTwLNmJ5VfabAjmJds");
        assert_eq!(call.arguments, "/workspace/orders.txt");
        assert!(
            call.result.contains("A-1042 shipped 2026-08-20"),
            "tool result = {:?}",
            call.result
        );

        // Permission wait and execution are timing detail, not a second tool
        // call.
        for id in [BLOCKED, EXECUTION] {
            assert_eq!(steps[id].kind, StepKind::Generic);
            assert!(steps[id].tool.is_none());
        }
        assert_eq!(steps[LLM_FINAL].kind, StepKind::Llm);
    }

    #[test]
    fn failed_request_honours_bool_success() {
        // `success` is a bool on the wire while the reference documents it as
        // a string, and a failed request leaves the span status unset.
        let payload = br#"{"resourceSpans":[{"resource":{"attributes":[
          {"key":"service.name","value":{"stringValue":"probe"}}]},
          "scopeSpans":[{"spans":[{
            "traceId":"9f2c41a7be004d18a1c7d3e5b6084411","spanId":"1a2b3c4d5e6f7006",
            "name":"claude_code.llm_request","startTimeUnixNano":"1756900012500000000",
            "endTimeUnixNano":"1756900014100000000","attributes":[
              {"key":"gen_ai.request.model","value":{"stringValue":"claude-opus-5[1m]"}},
              {"key":"success","value":{"boolValue":false}},
              {"key":"status_code","value":{"intValue":"529"}},
              {"key":"error","value":{"stringValue":"overloaded_error"}}],
            "status":{}}]}]}]}"#;
        let request = json::from_slice(payload).unwrap();
        let steps = normalize(&request);
        assert_eq!(steps.len(), 1);
        let failed = &steps[0];
        assert_eq!(failed.kind, StepKind::Llm);
        assert_eq!(failed.status, Status::Error);
        assert_eq!(failed.error, "overloaded_error");
    }

    #[test]
    fn failed_request_without_error_uses_status_code() {
        let payload = br#"{"resourceSpans":[{"scopeSpans":[{"spans":[{
            "traceId":"9f2c41a7be004d18a1c7d3e5b6084412","spanId":"1a2b3c4d5e6f7007",
            "name":"claude_code.llm_request","startTimeUnixNano":"1756900012500000000",
            "attributes":[{"key":"success","value":{"boolValue":false}},
                          {"key":"status_code","value":{"intValue":"529"}}]}]}]}]}"#;
        let steps = normalize(&json::from_slice(payload).unwrap());
        assert_eq!(steps[0].error, "request failed with status 529");
    }

    #[test]
    fn string_success_false_fails_too() {
        let payload = br#"{"resourceSpans":[{"scopeSpans":[{"spans":[{
            "traceId":"9f2c41a7be004d18a1c7d3e5b6084413","spanId":"1a2b3c4d5e6f7008",
            "name":"claude_code.llm_request","startTimeUnixNano":"1756900012500000000",
            "attributes":[{"key":"success","value":{"stringValue":"false"}}]}]}]}]}"#;
        let steps = normalize(&json::from_slice(payload).unwrap());
        assert_eq!(steps[0].status, Status::Error);
    }
}
