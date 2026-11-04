//! Vercel AI SDK dialect (`experimental_telemetry`): span names double as
//! operation identifiers (`ai.generateText`, `ai.generateText.doGenerate`,
//! `ai.toolCall`) with camelCase `ai.*` attributes. Provider-call spans
//! (`*.doGenerate`/`*.doStream`) additionally duplicate `gen_ai.*`
//! attributes — a hybrid emitter, so this handler runs before the gen_ai
//! heuristics.

use crate::model::{LlmCall, Message, Step, StepKind, ToolCall};

use super::attributes::Attrs;
use super::messages::vercel_prompt_messages;
use super::proto;

pub(crate) fn is_vercel(span: &proto::Span) -> bool {
    span.name.starts_with("ai.")
}

pub(crate) fn apply(span: &proto::Span, attrs: &Attrs, step: &mut Step) {
    let name = span.name.as_str();
    if name == "ai.toolCall" {
        step.kind = StepKind::Tool;
        let mut tool = ToolCall {
            name: attrs.string(&["ai.toolCall.name"]),
            call_id: attrs.string(&["ai.toolCall.id"]),
            arguments: attrs.string(&["ai.toolCall.args"]),
            result: attrs.string(&["ai.toolCall.result"]),
        };
        if tool.name.is_empty() {
            tool.name = name.to_string();
        }
        step.tool = Some(tool);
    } else if name.ends_with(".doGenerate")
        || name.ends_with(".doStream")
        || name.ends_with(".doEmbed")
    {
        step.kind = StepKind::Llm;
        let mut llm = LlmCall {
            provider: attrs.string(&["gen_ai.provider.name", "gen_ai.system", "ai.model.provider"]),
            request_model: attrs.string(&["gen_ai.request.model", "ai.model.id"]),
            response_model: attrs.string(&["gen_ai.response.model", "ai.response.model"]),
            input_tokens: attrs.int(&["gen_ai.usage.input_tokens", "ai.usage.promptTokens"]),
            output_tokens: attrs.int(&["gen_ai.usage.output_tokens", "ai.usage.completionTokens"]),
            input_messages: vercel_prompt_messages(attrs, "ai.prompt.messages"),
            ..LlmCall::default()
        };
        if let Some(text) = non_empty(attrs.string(&["ai.response.text"])) {
            llm.output_messages = vec![Message {
                role: "assistant".to_string(),
                content: text,
            }];
        } else if let Some(calls) = non_empty(attrs.string(&["ai.response.toolCalls"])) {
            llm.output_messages = vec![Message {
                role: "assistant".to_string(),
                content: format!("[tool_calls {calls}]"),
            }];
        }
        step.llm = Some(llm);
    } else {
        // Wrapper spans (ai.generateText, ai.streamText, ...) repeat the
        // child provider-call's ai.usage.* totals; classifying them as
        // generic (token-less) prevents double counting per run.
        step.kind = StepKind::Generic;
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use crate::ingest::normalize::normalize;
    use crate::ingest::testutil;
    use crate::ingest::testutil::{Attr, request_with_spans};
    use crate::model::StepKind;

    #[test]
    fn vercel_fixture_counts_provider_call_once() {
        let request = testutil::request_from_fixture("vercel_ai_sdk.json");
        let steps = normalize(&request);
        assert_eq!(steps.len(), 3);
        let by_name = |name: &str| steps.iter().find(|step| step.name == name).unwrap();

        let wrapper = by_name("ai.generateText");
        assert_eq!(wrapper.kind, StepKind::Generic);
        assert!(wrapper.llm.is_none());

        let llm = by_name("ai.generateText.doGenerate");
        assert_eq!(llm.kind, StepKind::Llm);
        let call = llm.llm.as_ref().unwrap();
        assert_eq!(call.provider, "anthropic");
        assert_eq!(call.request_model, "claude-sonnet-5");
        assert_eq!((call.input_tokens, call.output_tokens), (950, 120));
        assert_eq!(call.input_messages.len(), 1);
        assert_eq!(call.input_messages[0].content, "Where is order A-1042?");
        assert_eq!(call.output_messages.len(), 1);
        assert!(call.output_messages[0].content.contains("searchOrders"));

        let tool = by_name("ai.toolCall");
        assert_eq!(tool.kind, StepKind::Tool);
        let call = tool.tool.as_ref().unwrap();
        assert_eq!(call.name, "searchOrders");
        assert_eq!(call.call_id, "call_v1");
        assert_eq!(call.arguments, r#"{"query": "A-1042"}"#);
        assert_eq!(call.result, r#"{"status": "shipped"}"#);

        let input_tokens: i64 = steps
            .iter()
            .filter_map(|step| step.llm.as_ref())
            .map(|call| call.input_tokens)
            .sum();
        assert_eq!(input_tokens, 950, "wrapper and child double counted");
    }

    #[test]
    fn vercel_falls_back_to_ai_attributes_without_gen_ai() {
        let mut span = testutil::span("ai.streamText.doStream", "0000000000000001", 1);
        span.attributes = vec![
            testutil::key_value("ai.model.id", Attr::Str("gpt-5.2")),
            testutil::key_value("ai.model.provider", Attr::Str("openai")),
            testutil::key_value("ai.usage.promptTokens", Attr::Int(40)),
            testutil::key_value("ai.usage.completionTokens", Attr::Int(7)),
        ];
        let steps = normalize(&request_with_spans("svc", vec![span]));
        let call = steps[0].llm.as_ref().unwrap();
        assert_eq!(call.provider, "openai");
        assert_eq!(call.request_model, "gpt-5.2");
        assert_eq!((call.input_tokens, call.output_tokens), (40, 7));
    }
}
