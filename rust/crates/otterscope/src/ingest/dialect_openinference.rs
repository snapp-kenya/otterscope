//! OpenInference (Arize) dialect — the de-facto convention for the OpenAI
//! Agents SDK and CrewAI instrumentors. `openinference.span.kind` is required
//! on every OpenInference span, which makes detection unambiguous.

use crate::model::{LlmCall, Step, StepKind, ToolCall};

use super::attributes::Attrs;
use super::messages::open_inference_messages;
use super::normalize::prompt_identity;
use super::proto;

pub(crate) fn is_open_inference(attrs: &Attrs) -> bool {
    attrs.contains("openinference.span.kind")
}

pub(crate) fn apply(span: &proto::Span, attrs: &Attrs, step: &mut Step) {
    match attrs.string(&["openinference.span.kind"]).as_str() {
        "LLM" => {
            step.kind = StepKind::Llm;
            step.llm = Some(LlmCall {
                provider: attrs.string(&["llm.provider", "llm.system"]),
                request_model: attrs.string(&["llm.model_name"]),
                input_tokens: attrs.int(&["llm.token_count.prompt"]),
                output_tokens: attrs.int(&["llm.token_count.completion"]),
                cache_read_tokens: attrs.int(&["llm.token_count.prompt_details.cache_read"]),
                cache_creation_tokens: attrs.int(&["llm.token_count.prompt_details.cache_write"]),
                reasoning_tokens: attrs.int(&["llm.token_count.completion_details.reasoning"]),
                input_messages: open_inference_messages(attrs, "llm.input_messages."),
                output_messages: open_inference_messages(attrs, "llm.output_messages."),
                prompt: prompt_identity(
                    attrs.string(&["llm.prompt_template.name", "prompt.name"]),
                    attrs.string(&["llm.prompt_template.version"]),
                ),
                ..LlmCall::default()
            });
        }
        "EMBEDDING" => {
            step.kind = StepKind::Llm;
            step.llm = Some(LlmCall {
                provider: attrs.string(&["llm.provider", "llm.system"]),
                request_model: attrs.string(&["embedding.model_name", "llm.model_name"]),
                input_tokens: attrs.int(&["llm.token_count.prompt"]),
                ..LlmCall::default()
            });
        }
        "TOOL" => {
            step.kind = StepKind::Tool;
            let mut tool = ToolCall {
                name: attrs.string(&["tool.name"]),
                call_id: attrs.string(&["tool_call.id"]),
                arguments: attrs.string(&["tool_call.function.arguments", "input.value"]),
                result: attrs.string(&["output.value"]),
            };
            if tool.name.is_empty() {
                tool.name = span.name.clone();
            }
            step.tool = Some(tool);
        }
        "AGENT" => {
            step.kind = StepKind::Agent;
            if step.agent_name.is_empty() {
                step.agent_name = attrs.string(&["agent.name"]);
            }
            if step.agent_name.is_empty() {
                step.agent_name = span.name.clone();
            }
        }
        // CHAIN, RETRIEVER, RERANKER, GUARDRAIL, EVALUATOR, PROMPT, and future
        // kinds stay visible as generic steps.
        _ => step.kind = StepKind::Generic,
    }
}

#[cfg(test)]
mod tests {
    use crate::ingest::normalize::normalize;
    use crate::ingest::testutil;
    use crate::model::StepKind;

    #[test]
    fn openinference_fixture_classifies_every_span() {
        let request = testutil::request_from_fixture("openai_agents_openinference.json");
        let steps = normalize(&request);
        assert_eq!(steps.len(), 5);
        let by_name = |name: &str| steps.iter().find(|step| step.name == name).unwrap();

        let agent = by_name("Triage workflow");
        assert_eq!(agent.kind, StepKind::Agent);
        assert_eq!(agent.agent_name, "Triage Agent");
        assert_eq!(agent.service, "triage-agents");

        let llm = by_name("Response");
        assert_eq!(llm.kind, StepKind::Llm);
        let call = llm.llm.as_ref().unwrap();
        assert_eq!(call.provider, "openai");
        assert_eq!(call.request_model, "gpt-5.2");
        assert_eq!(call.input_tokens, 1450);
        assert_eq!(call.output_tokens, 210);
        assert_eq!(call.cache_read_tokens, 1200);

        // Messages reassemble from the dot-indexed attributes.
        assert_eq!(call.input_messages.len(), 1);
        assert_eq!(call.input_messages[0].role, "user");
        assert_eq!(call.input_messages[0].content, "Where is my order?");
        assert_eq!(call.output_messages.len(), 1);
        assert!(
            call.output_messages[0]
                .content
                .contains("[tool_call lookup_order]")
        );

        let tool = by_name("lookup_order");
        assert_eq!(tool.kind, StepKind::Tool);
        let call = tool.tool.as_ref().unwrap();
        assert_eq!(call.name, "lookup_order");
        assert_eq!(call.call_id, "call_88");
        assert_eq!(call.arguments, r#"{"order_id": "A-1042"}"#);

        let handoff = by_name("handoff to Order Agent");
        assert_eq!(handoff.kind, StepKind::Tool);
        assert_eq!(
            handoff.tool.as_ref().unwrap().name,
            "handoff to Order Agent"
        );

        assert_eq!(by_name("guardrail check").kind, StepKind::Generic);
    }
}
