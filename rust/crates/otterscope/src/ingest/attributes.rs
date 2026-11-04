//! Attribute access over decoded OTLP key/value lists.
//!
//! Mirrors `pcommon.Map` semantics: lookup is by first matching key, string
//! conversion is lenient (numbers and bools stringify), while integer lookup
//! only accepts values that arrived as integers.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use super::proto::{self, any_value::Value};

#[derive(Clone, Copy)]
pub(crate) struct Attrs<'a> {
    items: &'a [proto::KeyValue],
}

impl<'a> Attrs<'a> {
    pub(crate) fn new(items: &'a [proto::KeyValue]) -> Self {
        Self { items }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&'a proto::AnyValue> {
        self.items
            .iter()
            .find(|kv| kv.key == key)
            .and_then(|kv| kv.value.as_ref())
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.items.iter().any(|kv| kv.key == key)
    }

    /// First present key's string value, like the Go `stringAttr` helper.
    pub(crate) fn string(&self, keys: &[&str]) -> String {
        for key in keys {
            if let Some(value) = self.get(key) {
                return value.as_string();
            }
        }
        String::new()
    }

    /// First present integer key, ignoring non-integer values (`intAttr`).
    pub(crate) fn int(&self, keys: &[&str]) -> i64 {
        for key in keys {
            if let Some(proto::AnyValue {
                value: Some(Value::Int(v)),
            }) = self.get(key)
            {
                return *v;
            }
        }
        0
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&'a str, &'a proto::AnyValue)> {
        self.items
            .iter()
            .filter_map(|kv| kv.value.as_ref().map(|v| (kv.key.as_str(), v)))
    }
}

impl proto::AnyValue {
    pub(crate) fn as_string(&self) -> String {
        match &self.value {
            None => String::new(),
            Some(Value::String(v)) => v.clone(),
            Some(Value::Bool(v)) => v.to_string(),
            Some(Value::Int(v)) => v.to_string(),
            Some(Value::Double(v)) => format_double(*v),
            Some(Value::Array(v)) => {
                serde_json::to_string(&self.as_raw_of_array(v)).unwrap_or_default()
            }
            Some(Value::Kvlist(v)) => {
                serde_json::to_string(&self.as_raw_of_kvlist(v)).unwrap_or_default()
            }
            Some(Value::Bytes(v)) => BASE64.encode(v),
        }
    }

    /// Raw JSON view used by the message extractors (Go's `AsRaw`).
    pub(crate) fn as_raw(&self) -> serde_json::Value {
        match &self.value {
            None => serde_json::Value::Null,
            Some(Value::String(v)) => serde_json::Value::String(v.clone()),
            Some(Value::Bool(v)) => serde_json::Value::Bool(*v),
            Some(Value::Int(v)) => serde_json::Value::from(*v),
            Some(Value::Double(v)) => serde_json::Number::from_f64(*v)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
            Some(Value::Array(v)) => self.as_raw_of_array(v),
            Some(Value::Kvlist(v)) => self.as_raw_of_kvlist(v),
            Some(Value::Bytes(v)) => serde_json::Value::String(BASE64.encode(v)),
        }
    }

    fn as_raw_of_array(&self, array: &proto::ArrayValue) -> serde_json::Value {
        serde_json::Value::Array(array.values.iter().map(|v| v.as_raw()).collect())
    }

    fn as_raw_of_kvlist(&self, list: &proto::KeyValueList) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for kv in &list.values {
            let value = kv.value.as_ref().map(|v| v.as_raw()).unwrap_or_default();
            map.insert(kv.key.clone(), value);
        }
        serde_json::Value::Object(map)
    }
}

fn format_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v.is_sign_positive() {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::any_value::Value;

    fn attrs(values: &[(&str, Value)]) -> Vec<proto::KeyValue> {
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

    #[test]
    fn string_lookup_falls_through_keys_and_stringifies() {
        let list = attrs(&[
            ("gen_ai.system", Value::String("anthropic".into())),
            ("gen_ai.provider.name", Value::String("openai".into())),
            ("success", Value::Bool(false)),
            ("status_code", Value::Int(529)),
        ]);
        let attrs = Attrs::new(&list);
        assert_eq!(
            attrs.string(&["gen_ai.provider.name", "gen_ai.system"]),
            "openai"
        );
        assert_eq!(attrs.string(&["missing", "gen_ai.system"]), "anthropic");
        assert_eq!(attrs.string(&["success"]), "false");
        assert_eq!(attrs.string(&["status_code"]), "529");
        assert_eq!(attrs.string(&["missing"]), "");
    }

    #[test]
    fn int_lookup_ignores_non_integers() {
        let list = attrs(&[
            ("a", Value::String("12".into())),
            ("b", Value::Double(1.5)),
            ("c", Value::Int(7)),
        ]);
        let attrs = Attrs::new(&list);
        assert_eq!(attrs.int(&["a", "b", "c"]), 7);
        assert_eq!(attrs.int(&["a"]), 0);
    }

    #[test]
    fn array_values_render_as_json() {
        let list = attrs(&[(
            "gen_ai.response.finish_reasons",
            Value::Array(proto::ArrayValue {
                values: vec![proto::AnyValue {
                    value: Some(Value::String("end_turn".into())),
                }],
            }),
        )]);
        assert_eq!(
            Attrs::new(&list).string(&["gen_ai.response.finish_reasons"]),
            r#"["end_turn"]"#
        );
    }
}
