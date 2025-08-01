use super::super::*;
use std::collections::HashSet;

pub struct DeduplicationPipe {
    seen_ids: HashSet<[u8; 32]>,
    max_size: usize,
    name: String,
}

impl DeduplicationPipe {
    pub fn new(max_size: usize) -> Self {
        Self {
            seen_ids: HashSet::new(),
            max_size,
            name: format!("Deduplication(max:{})", max_size),
        }
    }
}

#[async_trait(?Send)]
impl Pipe for DeduplicationPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        if self.seen_ids.contains(&event.id) {
            return Ok(PipeOutput::Drop);
        }

        // Add to seen set, with size limit
        if self.seen_ids.len() < self.max_size {
            self.seen_ids.insert(event.id);
        }

        Ok(PipeOutput::Event(event))
    }

    fn name(&self) -> &str {
        &self.name
    }
}
