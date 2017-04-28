use super::invoke::{Invoke, Identity};

pub struct MemoryDefinition {
    pub min: u32,
    pub max: Option<u32>,
}

pub struct MemoryBuilder<F=Identity> {
    callback: F,
    memory: MemoryDefinition,
}

impl MemoryBuilder {
    pub fn new() -> Self {
        MemoryBuilder::with_callback(Identity)
    }
}

impl<F> MemoryBuilder<F> where F: Invoke<MemoryDefinition> {
    pub fn with_callback(callback: F) -> Self {
        MemoryBuilder {
            callback: callback,
            memory: Default::default(),
        }
    }

    pub fn with_min(mut self, min: u32) -> Self {
        self.memory.min = min;
        self
    }

    pub fn with_max(mut self, max: Option<u32>) -> Self {
        self.memory.max = max;
        self
    }

    pub fn build(self) -> F::Result {
        self.callback.invoke(self.memory)
    }
}

impl Default for MemoryDefinition {
    fn default() -> Self {
        MemoryDefinition {
            min: 1,
            max: None,
        }
    }
}
