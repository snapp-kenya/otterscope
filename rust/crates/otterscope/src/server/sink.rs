//! The ingest sink: prices LLM calls, persists the batch, wakes live-tail
//! clients and schedules assertion evaluation. The ingest module hands the
//! normalized batch to its caller, so this lives on the server side.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;

use super::Hub;
use super::evaluator::Evaluator;
use crate::ingest::{IngestResult, Sink};
use crate::model::Step;
use crate::pricing;
use crate::store::Store;

pub struct StoreSink {
    store: Arc<Store>,
    prices: pricing::Table,
    evaluator: Arc<Evaluator>,
    hub: Hub,
}

impl StoreSink {
    pub fn new(
        store: Arc<Store>,
        prices: pricing::Table,
        evaluator: Arc<Evaluator>,
        hub: Hub,
    ) -> Self {
        Self {
            store,
            prices,
            evaluator,
            hub,
        }
    }
}

impl Sink for StoreSink {
    fn consume(&self, project: &str, raw: &[u8], steps: &[Step]) -> anyhow::Result<IngestResult> {
        let mut priced = steps.to_vec();
        price_steps(&mut priced, &self.prices);
        let rejected = self
            .store
            .ingest_batch(project, raw, &priced)
            .context("persist ingest batch")?;

        self.hub.broadcast();

        let mut seen = HashSet::new();
        let mut refs = Vec::new();
        for step in &priced {
            if seen.insert(step.run_id.clone()) {
                refs.push((project.to_string(), step.run_id.clone()));
            }
        }
        self.evaluator.enqueue(refs);
        Ok(IngestResult::with_rejections(
            i64::try_from(rejected).unwrap_or(i64::MAX),
        ))
    }
}

/// Stamp `cost_usd` on LLM steps with a known model. Unknown models stay
/// `None` — never a fabricated cost. The response model is what was billed.
pub fn price_steps(steps: &mut [Step], prices: &pricing::Table) {
    for step in steps.iter_mut() {
        let Some(llm) = step.llm.as_mut() else {
            continue;
        };
        if llm.input_tokens == 0 && llm.output_tokens == 0 {
            continue;
        }
        let model = if llm.response_model.is_empty() {
            &llm.request_model
        } else {
            &llm.response_model
        };
        if model.is_empty() {
            continue;
        }
        if let Some(usd) = prices.cost(
            model,
            llm.input_tokens,
            llm.output_tokens,
            llm.cache_read_tokens,
            llm.cache_creation_tokens,
        ) {
            llm.cost_usd = Some(usd);
        }
    }
}
