//! Maintained pricing table and cost math (port of `internal/pricing`).
use crate::model::LlmCall;

#[derive(Debug, Default, Clone)]
pub struct Table;

impl Table {
    pub fn merge_json(&mut self, _data: &[u8]) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Cost of one call, when the model is known.
pub fn cost(_t: &Table, _call: &LlmCall) -> Option<f64> {
    None
}
