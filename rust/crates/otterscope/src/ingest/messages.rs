//! Message extraction, best-effort by design: multimodal parts flatten to
//! text, tool interactions render inline. The raw batch is retained, so richer
//! extraction later can backfill via renormalization.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::Message;

use super::attributes::Attrs;
use super::proto;

/// Parse the `gen_ai_latest_experimental` message format: an array of
/// `{role, parts:[{type,...}]}` recorded as a structured attribute or as a
/// JSON string.
pub(crate) fn gen_ai_messages(attrs: &Attrs, key: &str) -> Vec<Message> {
    let Some(value) = attrs.get(key) else {
        return Vec::new();
    };
    let raw = match &value.value {
        Some(proto::any_value::Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(parsed) => parsed,
            Err(_) => return Vec::new(),
        },
        _ => value.as_raw(),
    };
    let Some(items) = raw.as_array() else {
        return Vec::new();
    };

    let mut messages = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let mut content = String::new();
        if let Some(parts) = object.get("parts").and_then(Value::as_array) {
            for part in parts {
                let Some(part) = part.as_object() else {
                    continue;
                };
                if !content.is_empty() {
                    content.push('\n');
                }
                match json_str(part.get("type")) {
                    "text" => content.push_str(json_str(part.get("content"))),
                    "tool_call" => content.push_str(&format!(
                        "[tool_call {}({})]",
                        json_str(part.get("name")),
                        compact(part.get("arguments"))
                    )),
                    "tool_call_response" => content
                        .push_str(&format!("[tool_result {}]", compact(part.get("response")))),
                    other => content.push_str(&format!("[{other}]")),
                }
            }
        }
        messages.push(Message {
            role: json_str(object.get("role")).to_string(),
            content,
        });
    }
    messages
}

/// `gen_ai.system_instructions`, either a plain string or a parts array, as a
/// leading system message.
pub(crate) fn gen_ai_system_instructions(attrs: &Attrs) -> Vec<Message> {
    let Some(value) = attrs.get("gen_ai.system_instructions") else {
        return Vec::new();
    };
    let raw = if let Some(proto::any_value::Value::String(s)) = &value.value {
        if !s.trim_start().starts_with('[') {
            return vec![Message {
                role: "system".to_string(),
                content: s.clone(),
            }];
        }
        match serde_json::from_str::<Value>(s) {
            Ok(parsed) => parsed,
            Err(_) => {
                return vec![Message {
                    role: "system".to_string(),
                    content: s.clone(),
                }];
            }
        }
    } else {
        value.as_raw()
    };
    let Some(parts) = raw.as_array() else {
        return Vec::new();
    };
    let mut content = String::new();
    for part in parts {
        if let Some(part) = part.as_object() {
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(json_str(part.get("content")));
        }
    }
    if content.is_empty() {
        return Vec::new();
    }
    vec![Message {
        role: "system".to_string(),
        content,
    }]
}

/// Reassemble OpenInference's dot-indexed flattened messages
/// (`llm.input_messages.0.message.role`, ...) under `prefix`.
pub(crate) fn open_inference_messages(attrs: &Attrs, prefix: &str) -> Vec<Message> {
    #[derive(Default)]
    struct Acc {
        role: String,
        content: String,
        parts: BTreeMap<i64, String>,
        tool_calls: BTreeMap<i64, String>,
    }

    let mut by_index: BTreeMap<i64, Acc> = BTreeMap::new();
    for (key, value) in attrs.iter() {
        let Some(rest) = key.strip_prefix(prefix) else {
            continue;
        };
        let Some((index, suffix)) = split_index(rest) else {
            continue;
        };
        let acc = by_index.entry(index).or_default();
        if suffix == "message.role" {
            acc.role = value.as_string();
        } else if suffix == "message.content" {
            acc.content = value.as_string();
        } else if let Some(contents) = suffix.strip_prefix("message.contents.") {
            if let Some((part, "message_content.text")) = split_index(contents) {
                acc.parts.insert(part, value.as_string());
            }
        } else if let Some(tool_calls) = suffix.strip_prefix("message.tool_calls.")
            && let Some((call, tail)) = split_index(tool_calls)
        {
            match tail {
                "tool_call.function.name" => {
                    let entry = acc.tool_calls.entry(call).or_default();
                    *entry = format!("{}{entry}", value.as_string());
                }
                "tool_call.function.arguments" => {
                    let entry = acc.tool_calls.entry(call).or_default();
                    entry.push('(');
                    entry.push_str(&value.as_string());
                    entry.push(')');
                }
                _ => {}
            }
        }
    }

    let mut messages = Vec::with_capacity(by_index.len());
    for (_, acc) in by_index {
        let mut content = acc.content;
        if content.is_empty() && !acc.parts.is_empty() {
            content = acc.parts.values().cloned().collect::<Vec<_>>().join("\n");
        }
        if !acc.tool_calls.is_empty() {
            let mut rendered = content;
            for call in acc.tool_calls.values() {
                if !rendered.is_empty() {
                    rendered.push('\n');
                }
                rendered.push_str(&format!("[tool_call {call}]"));
            }
            content = rendered;
        }
        messages.push(Message {
            role: acc.role,
            content,
        });
    }
    messages
}

/// Parse `ai.prompt.messages`: a JSON string of `{role, content}` where content
/// is a string or an array of parts.
pub(crate) fn vercel_prompt_messages(attrs: &Attrs, key: &str) -> Vec<Message> {
    let Some(value) = attrs.get(key) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Map<String, Value>>>(&value.as_string())
    else {
        return Vec::new();
    };
    let mut messages = Vec::new();
    for item in items {
        let mut content = String::new();
        match item.get("content") {
            Some(Value::String(s)) => content.push_str(s),
            Some(Value::Array(parts)) => {
                for part in parts {
                    let Some(part) = part.as_object() else {
                        continue;
                    };
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    let text = json_str(part.get("text"));
                    if text.is_empty() {
                        content.push('[');
                        content.push_str(json_str(part.get("type")));
                        content.push(']');
                    } else {
                        content.push_str(text);
                    }
                }
            }
            _ => {}
        }
        messages.push(Message {
            role: json_str(item.get("role")).to_string(),
            content,
        });
    }
    messages
}

fn split_index(s: &str) -> Option<(i64, &str)> {
    let (head, rest) = s.split_once('.')?;
    if head.is_empty() {
        return None;
    }
    let index = head.parse::<i64>().ok()?;
    Some((index, rest))
}

fn json_str(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

/// Render an arbitrary decoded JSON value on one line (Go's `compact`).
fn compact(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::any_value::Value as V;

    fn attrs(values: &[(&str, V)]) -> Vec<proto::KeyValue> {
        values
            .iter()
            .map(|(key, value)| proto::KeyValue {
                key: (*key).to_string(),
                value: Some(proto::AnyValue {
                    value: Some(value.clone()),
                }),
            })
            .collect()
    }

    fn attrs_ref(list: &[proto::KeyValue]) -> Attrs<'_> {
        Attrs::new(list)
    }

    #[test]
    fn gen_ai_messages_flatten_parts() {
        let payload = r#"[{"role":"assistant","parts":[
            {"type":"text","content":"Here are the findings..."},
            {"type":"tool_call","name":"fetch_paper","arguments":{"id":42}}]}]"#;
        let list = attrs(&[("gen_ai.output.messages", V::String(payload.into()))]);
        let messages = gen_ai_messages(&attrs_ref(&list), "gen_ai.output.messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(
            messages[0].content,
            "Here are the findings...\n[tool_call fetch_paper({\"id\":42})]"
        );
    }

    #[test]
    fn gen_ai_messages_tolerate_non_json() {
        let list = attrs(&[("gen_ai.input.messages", V::String("nope".into()))]);
        assert!(gen_ai_messages(&attrs_ref(&list), "gen_ai.input.messages").is_empty());
    }

    #[test]
    fn system_instructions_plain_and_parts() {
        let plain = attrs(&[(
            "gen_ai.system_instructions",
            V::String("You are careful.".into()),
        )]);
        let messages = gen_ai_system_instructions(&attrs_ref(&plain));
        assert_eq!(
            messages,
            vec![Message {
                role: "system".into(),
                content: "You are careful.".into()
            }]
        );

        let parts = attrs(&[(
            "gen_ai.system_instructions",
            V::String(r#"[{"type":"text","content":"a"},{"type":"text","content":"b"}]"#.into()),
        )]);
        let messages = gen_ai_system_instructions(&attrs_ref(&parts));
        assert_eq!(messages[0].content, "a\nb");
    }

    #[test]
    fn open_inference_messages_order_parts_and_tool_calls() {
        let list = attrs(&[
            (
                "llm.input_messages.0.message.role",
                V::String("user".into()),
            ),
            (
                "llm.input_messages.0.message.contents.1.message_content.text",
                V::String("second".into()),
            ),
            (
                "llm.input_messages.0.message.contents.0.message_content.text",
                V::String("first".into()),
            ),
            (
                "llm.output_messages.0.message.role",
                V::String("assistant".into()),
            ),
            (
                "llm.output_messages.0.message.tool_calls.0.tool_call.function.arguments",
                V::String(r#"{"q":1}"#.into()),
            ),
            (
                "llm.output_messages.0.message.tool_calls.0.tool_call.function.name",
                V::String("lookup".into()),
            ),
        ]);
        let input = open_inference_messages(&attrs_ref(&list), "llm.input_messages.");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].content, "first\nsecond");
        let output = open_inference_messages(&attrs_ref(&list), "llm.output_messages.");
        assert_eq!(output[0].content, "[tool_call lookup({\"q\":1})]");
    }

    #[test]
    fn vercel_prompt_messages_flatten_text_parts() {
        let payload = r#"[{"role":"user","content":[{"type":"text","text":"Where?"}]},
                          {"role":"assistant","content":"Here."}]"#;
        let list = attrs(&[("ai.prompt.messages", V::String(payload.into()))]);
        let messages = vercel_prompt_messages(&attrs_ref(&list), "ai.prompt.messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Where?");
        assert_eq!(messages[1].content, "Here.");
    }
}
