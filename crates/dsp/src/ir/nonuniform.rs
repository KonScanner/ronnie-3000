use super::{IrEngineKind, uniform::PartitionEngine};

pub(crate) struct HeadTailEngine {
    head: PartitionEngine,
    tail: Option<PartitionEngine>,
    tail_output: Vec<f32>,
}

impl HeadTailEngine {
    pub(crate) fn new(
        samples: &[f32],
        kind: IrEngineKind,
        maximum_block_size: usize,
        tail_scratch: usize,
    ) -> Self {
        let (head_samples, tail_block) = match kind {
            IrEngineKind::Uniform => (samples.len(), maximum_block_size),
            IrEngineKind::NonUniform { head } => (head.min(samples.len()), head),
        };
        let head = PartitionEngine::new(&samples[..head_samples], maximum_block_size);
        let tail = (head_samples < samples.len())
            .then(|| PartitionEngine::new(&samples[head_samples..], tail_block));
        Self {
            head,
            tail,
            tail_output: vec![0.0; tail_scratch],
        }
    }

    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        if let Some(tail) = &mut self.tail {
            tail.process(input, &mut self.tail_output[..input.len()], true);
        }
        self.head.process(input, output, false);
        if self.tail.is_some() {
            for (index, sample) in output.iter_mut().enumerate() {
                *sample += self.tail_output[index];
            }
        }
    }
}
