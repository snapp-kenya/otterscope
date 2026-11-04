//! OTLP spans → domain steps.
//!
//! Every span becomes a step — spans we don't recognize are kept as generic
//! steps, never dropped. Dialect quirks live here and nowhere downstream
//! (ADR-0002): the server sees only [`crate::model`] types.

use chrono::{DateTime, Utc};

use crate::model::{LlmCall, Status, Step, StepKind, ToolCall};

use super::attributes::Attrs;
use super::dialect_claudecode;
use super::dialect_openinference;
use super::dialect_vercel;
use super::messages;
use super::proto;

pub(crate) fn normalize(request: &proto::ExportTraceServiceRequest) -> Vec<Step> {
    let mut steps = Vec::new();
    for resource_spans in &request.resource_spans {
        let resource_attrs = match &resource_spans.resource {
            Some(resource) => Attrs::new(&resource.attributes),
            None => Attrs::new(&[]),
        };
        // openinference.project.name is Phoenix-style project routing — the
        // best service identity OpenInference emitters provide.
        let service = resource_attrs.string(&["service.name", "openinference.project.name"]);
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                // Drop spans with an all-zero trace or span ID: buggy SDKs
                // emit them, and globally they would collapse unrelated runs
                // together. A zero start timestamp would also poison
                // duration/percentile stats.
                if proto::is_empty_id(&span.trace_id)
                    || proto::is_empty_id(&span.span_id)
                    || span.start_time_unix_nano == 0
                {
                    continue;
                }
                steps.push(normalize_span(span, &service));
            }
        }
    }
    steps
}

/// Replay a retained raw batch through the current normalizer. The caller
/// stamps the project and upserts; this keeps OTel types inside the module.
pub fn renormalize_batch(raw: &[u8]) -> anyhow::Result<Vec<Step>> {
    let request = proto::decompress(raw)?;
    Ok(normalize(&request))
}

fn normalize_span(span: &proto::Span, service: &str) -> Step {
    let attrs = Attrs::new(&span.attributes);
    let mut step = Step {
        id: proto::hex_encode(&span.span_id),
        project: String::new(),
        run_id: proto::hex_encode(&span.trace_id),
        parent_id: if proto::is_empty_id(&span.parent_span_id) {
            String::new()
        } else {
            proto::hex_encode(&span.parent_span_id)
        },
        kind: StepKind::Generic,
        name: span.name.clone(),
        service: service.to_string(),
        agent_name: attrs.string(&["gen_ai.agent.name"]),
        status: Status::Ok,
        start: timestamp(span.start_time_unix_nano),
        end: Some(timestamp(span.end_time_unix_nano)),
        error: String::new(),
        llm: None,
        tool: None,
    };

    if span.status_code() == proto::StatusCode::Error as i32 {
        step.status = Status::Error;
        step.error = span
            .status
            .as_ref()
            .map(|status| status.message.clone())
            .unwrap_or_default();
        if step.error.is_empty() {
            step.error = "error".to_string();
        }
    }

    if dialect_openinference::is_open_inference(&attrs) {
        dialect_openinference::apply(span, &attrs, &mut step);
    } else if dialect_vercel::is_vercel(span) {
        dialect_vercel::apply(span, &attrs, &mut step);
    } else if dialect_claudecode::is_claude_code(span) {
        dialect_claudecode::apply(span, &attrs, &mut step);
    } else {
        apply_gen_ai(span, &attrs, &mut step);
    }

    // Zero timestamps confuse duration math downstream; clamp end >= start.
    if step.end.is_none_or(|end| end < step.start) {
        step.end = Some(step.start);
    }
    step
}

fn apply_gen_ai(span: &proto::Span, attrs: &Attrs, step: &mut Step) {
    match attrs.string(&["gen_ai.operation.name"]).as_str() {
        "chat" | "text_completion" | "generate_content" | "embeddings" => {
            step.kind = StepKind::Llm;
            // gen_ai.provider.name is the gen_ai_latest_experimental rename of
            // gen_ai.system; both are in the wild (ADR-0002).
            let mut input_messages = messages::gen_ai_system_instructions(attrs);
            input_messages.extend(messages::gen_ai_messages(attrs, "gen_ai.input.messages"));
            step.llm = Some(LlmCall {
                provider: attrs.string(&["gen_ai.provider.name", "gen_ai.system"]),
                request_model: attrs.string(&["gen_ai.request.model"]),
                response_model: attrs.string(&["gen_ai.response.model"]),
                input_tokens: attrs
                    .int(&["gen_ai.usage.input_tokens", "gen_ai.usage.prompt_tokens"]),
                output_tokens: attrs.int(&[
                    "gen_ai.usage.output_tokens",
                    "gen_ai.usage.completion_tokens",
                ]),
                cache_read_tokens: attrs.int(&["gen_ai.usage.cache_read.input_tokens"]),
                cache_creation_tokens: attrs.int(&["gen_ai.usage.cache_creation.input_tokens"]),
                reasoning_tokens: attrs.int(&["gen_ai.usage.reasoning.output_tokens"]),
                input_messages,
                output_messages: messages::gen_ai_messages(attrs, "gen_ai.output.messages"),
                prompt: prompt_identity(
                    attrs.string(&["gen_ai.prompt.name"]),
                    attrs.string(&["gen_ai.prompt.version"]),
                ),
                ..LlmCall::default()
            });
        }
        "execute_tool" => {
            step.kind = StepKind::Tool;
            let mut tool = ToolCall {
                name: attrs.string(&["gen_ai.tool.name"]),
                call_id: attrs.string(&["gen_ai.tool.call.id"]),
                arguments: attrs.string(&["gen_ai.tool.call.arguments"]),
                result: attrs.string(&["gen_ai.tool.call.result"]),
            };
            if tool.name.is_empty() {
                tool.name = span.name.clone();
            }
            step.tool = Some(tool);
        }
        "invoke_agent" | "create_agent" | "invoke_workflow" | "plan" => {
            step.kind = StepKind::Agent;
            if step.agent_name.is_empty() {
                step.agent_name = attrs.string(&["gen_ai.workflow.name"]);
            }
        }
        _ => step.kind = StepKind::Generic,
    }
}

/// Build a display identity from a prompt name and/or version, e.g.
/// "support-reply v3", "v3", or "" when neither is present.
pub(crate) fn prompt_identity(name: String, version: String) -> String {
    match (name.is_empty(), version.is_empty()) {
        (false, false) => format!("{name} v{version}"),
        (false, true) => name,
        (true, false) => format!("v{version}"),
        (true, true) => String::new(),
    }
}

fn timestamp(nanos: u64) -> DateTime<Utc> {
    let secs = i64::try_from(nanos / 1_000_000_000).ok();
    let subsec = u32::try_from(nanos % 1_000_000_000).ok();
    secs.zip(subsec)
        .and_then(|(secs, subsec)| DateTime::<Utc>::from_timestamp(secs, subsec))
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::testutil;

    #[test]
    fn fixture_normalizes_all_three_spans() {
        let request = testutil::request_from_fixture("pydantic_ai_chat.json");
        let steps = normalize(&request);
        assert_eq!(steps.len(), 3);

        let by_name = |name: &str| steps.iter().find(|s| s.name == name).unwrap();

        let agent = by_name("invoke_agent support-agent");
        assert_eq!(agent.kind, StepKind::Agent);
        assert_eq!(agent.agent_name, "support-agent");
        assert_eq!(agent.service, "support-agent");
        assert_eq!(agent.parent_id, "");
        assert_eq!(agent.run_id, "5b8efff798038103d269b633813fc60c");

        let llm = by_name("chat claude-sonnet-5");
        assert_eq!(llm.kind, StepKind::Llm);
        let call = llm.llm.as_ref().unwrap();
        assert_eq!(call.provider, "anthropic");
        assert_eq!(call.request_model, "claude-sonnet-5");
        assert_eq!(call.input_tokens, 812);
        assert_eq!(call.output_tokens, 142);
        assert_eq!(llm.parent_id, agent.id);
        assert!(llm.end.unwrap() > llm.start);

        let tool = by_name("execute_tool get_ticket");
        assert_eq!(tool.kind, StepKind::Tool);
        let call = tool.tool.as_ref().unwrap();
        assert_eq!(call.name, "get_ticket");
        assert_eq!(call.call_id, "toolu_01A");
    }

    #[test]
    fn non_genai_span_is_kept_generic() {
        let request = testutil::single_span(None, &[]);
        let steps = normalize(&request);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, StepKind::Generic);
    }

    #[test]
    fn unknown_operation_is_kept_generic() {
        let request = testutil::single_span(Some("summon_demon"), &[]);
        let steps = normalize(&request);
        assert_eq!(steps[0].kind, StepKind::Generic);
    }

    #[test]
    fn legacy_prompt_completion_token_aliases() {
        let request = testutil::single_span(
            Some("chat"),
            &[
                ("gen_ai.usage.prompt_tokens", testutil::Attr::Int(100)),
                ("gen_ai.usage.completion_tokens", testutil::Attr::Int(25)),
            ],
        );
        let call = normalize(&request)[0].llm.clone().unwrap();
        assert_eq!((call.input_tokens, call.output_tokens), (100, 25));
    }

    #[test]
    fn tool_without_name_falls_back_to_span_name() {
        let request = testutil::single_span(Some("execute_tool"), &[]);
        let tool = normalize(&request)[0].tool.clone().unwrap();
        assert_eq!(tool.name, "test-span");
    }

    #[test]
    fn error_status_carries_message() {
        let mut request = testutil::single_span(Some("chat"), &[]);
        request.resource_spans[0].scope_spans[0].spans[0].status = Some(proto::Status {
            message: "rate limited".to_string(),
            code: proto::StatusCode::Error as i32,
        });
        let step = &normalize(&request)[0];
        assert_eq!(step.status, Status::Error);
        assert_eq!(step.error, "rate limited");
    }

    #[test]
    fn error_status_without_message_says_error() {
        let mut request = testutil::single_span(Some("chat"), &[]);
        request.resource_spans[0].scope_spans[0].spans[0].status = Some(proto::Status {
            message: String::new(),
            code: proto::StatusCode::Error as i32,
        });
        assert_eq!(normalize(&request)[0].error, "error");
    }

    #[test]
    fn zero_ids_and_zero_start_are_dropped() {
        let mut request = testutil::single_span(Some("chat"), &[]);
        request.resource_spans[0].scope_spans[0].spans[0].span_id = vec![0; 8];
        assert!(normalize(&request).is_empty());

        let mut request = testutil::single_span(Some("chat"), &[]);
        request.resource_spans[0].scope_spans[0].spans[0].start_time_unix_nano = 0;
        assert!(normalize(&request).is_empty());
    }

    #[test]
    fn end_before_start_is_clamped() {
        let mut request = testutil::single_span(Some("chat"), &[]);
        request.resource_spans[0].scope_spans[0].spans[0].end_time_unix_nano = 1;
        let step = &normalize(&request)[0];
        assert_eq!(step.end, Some(step.start));
    }

    #[test]
    fn prompt_identity_variants() {
        assert_eq!(prompt_identity("x".into(), "3".into()), "x v3");
        assert_eq!(prompt_identity("x".into(), String::new()), "x");
        assert_eq!(prompt_identity(String::new(), "3".into()), "v3");
        assert_eq!(prompt_identity(String::new(), String::new()), "");
    }
}

#[cfg(test)]
mod dialect_tests {
    use crate::ingest::normalize::{normalize, renormalize_batch};
    use crate::ingest::proto;
    use crate::ingest::testutil;
    use crate::model::StepKind;

    #[test]
    fn experimental_dialect_fixture() {
        let request = testutil::request_from_fixture("genai_experimental_chat.json");
        let steps = normalize(&request);
        assert_eq!(steps.len(), 4);
        let by_name = |name: &str| steps.iter().find(|step| step.name == name).unwrap();

        let workflow = by_name("invoke_workflow deep-research");
        assert_eq!(workflow.kind, StepKind::Agent);
        assert_eq!(workflow.agent_name, "deep-research");

        let llm = by_name("chat claude-fable-5");
        assert_eq!(llm.kind, StepKind::Llm);
        let call = llm.llm.as_ref().unwrap();
        assert_eq!(call.provider, "anthropic");
        assert_eq!((call.input_tokens, call.output_tokens), (20000, 3200));
        assert_eq!(
            (
                call.cache_read_tokens,
                call.cache_creation_tokens,
                call.reasoning_tokens
            ),
            (18000, 1500, 2100)
        );

        let plan = by_name("plan researcher");
        assert_eq!(plan.kind, StepKind::Agent);
        assert_eq!(plan.agent_name, "researcher");

        assert_eq!(by_name("retrieval docs-index").kind, StepKind::Generic);
    }

    #[test]
    fn experimental_messages_flatten_parts() {
        let request = testutil::request_from_fixture("genai_experimental_chat.json");
        let steps = normalize(&request);
        let llm = steps
            .iter()
            .find(|step| step.name == "chat claude-fable-5")
            .unwrap();
        let call = llm.llm.as_ref().unwrap();

        assert_eq!(call.input_messages.len(), 2, "{:?}", call.input_messages);
        assert_eq!(call.input_messages[0].role, "system");
        assert_eq!(
            call.input_messages[0].content,
            "You are a careful research assistant."
        );
        assert_eq!(call.input_messages[1].role, "user");
        assert_eq!(
            call.input_messages[1].content,
            "Summarize the latest findings."
        );

        assert_eq!(call.output_messages.len(), 1);
        assert_eq!(call.output_messages[0].role, "assistant");
        assert!(
            call.output_messages[0]
                .content
                .contains("Here are the findings...")
        );
        assert!(
            call.output_messages[0]
                .content
                .contains(r#"[tool_call fetch_paper({"id":42})]"#),
            "output content = {:?}",
            call.output_messages[0].content
        );
    }

    #[test]
    fn mixed_dialect_batch_stays_coherent() {
        let mut request = testutil::request_from_fixture("pydantic_ai_chat.json");
        let mut experimental = testutil::request_from_fixture("genai_experimental_chat.json");
        let old_len = normalize(&request).len();
        request
            .resource_spans
            .append(&mut experimental.resource_spans);

        let steps = normalize(&request);
        assert_eq!(steps.len(), old_len + 4);
        let providers: std::collections::BTreeSet<&str> = steps
            .iter()
            .filter_map(|step| step.llm.as_ref())
            .map(|call| call.provider.as_str())
            .collect();
        assert_eq!(providers.into_iter().collect::<Vec<_>>(), vec!["anthropic"]);
    }

    #[test]
    fn raw_batch_is_retained_and_replay_is_idempotent() {
        let request = testutil::request_from_fixture("pydantic_ai_chat.json");
        let raw = proto::compress(&request).unwrap();

        // The raw batch keeps attributes normalization ignores, so a later
        // extractor can backfill from it.
        let retained = proto::decompress(&raw).unwrap();
        let attrs = &retained.resource_spans[0]
            .resource
            .as_ref()
            .unwrap()
            .attributes;
        assert!(
            attrs
                .iter()
                .any(|kv| kv.key == "telemetry.sdk.language" && kv.value.is_some())
        );

        let first = renormalize_batch(&raw).unwrap();
        let second = renormalize_batch(&raw).unwrap();
        assert_eq!(first.len(), 3);
        assert!(first.iter().all(|step| step.project.is_empty()));
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second).unwrap(),
            "renormalizing the same raw batch must produce identical steps"
        );
    }
}
