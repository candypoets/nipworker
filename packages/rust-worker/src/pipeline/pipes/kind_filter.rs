use super::super::*;
use std::collections::HashSet;

pub struct KindFilterPipe {
    kinds: HashSet<u16>,
    name: String,
}

impl KindFilterPipe {
    pub fn new(kinds: Vec<u64>) -> Self {
        Self {
            name: format!("KindFilter({:?})", kinds),
            kinds: kinds.into_iter().map(|k| k as u16).collect(),
        }
    }
}

impl Pipe for KindFilterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Get kind from either raw or parsed event
        let kind = if let Some(ref raw) = event.raw {
            raw.kind
        } else if let Some(ref parsed) = event.parsed {
            parsed.event.kind
        } else {
            return Ok(PipeOutput::Drop);
        };

        if self.kinds.contains(&kind) {
            Ok(PipeOutput::Event(event))
        } else {
            Ok(PipeOutput::Drop)
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
