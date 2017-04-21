use std::sync::Arc;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use parking_lot::RwLock;
use elements::Module;
use interpreter::Error;
use interpreter::module::ModuleInstance;

/// Program instance. Program is a set of instantiated modules.
pub struct ProgramInstance {
	/// Shared data reference.
	essence: Arc<ProgramInstanceEssence>,
}

/// Program instance essence.
pub struct ProgramInstanceEssence {
	/// Loaded modules.
	modules: RwLock<HashMap<String, Arc<ModuleInstance>>>,
}

impl ProgramInstance {
	/// Create new program instance.
	pub fn new() -> Self {
		ProgramInstance {
			essence: Arc::new(ProgramInstanceEssence::new()),
		}
	}

	/// Instantiate module.
	pub fn add_module(&self, name: &str, module: Module) -> Result<(), Error> {
		let mut modules = self.essence.modules.write();
		match modules.entry(name.into()) {
			Entry::Occupied(_) => Err(Error::Program(format!("module {} already instantiated", name))),
			Entry::Vacant(entry) => {
				entry.insert(Arc::new(ModuleInstance::new(Arc::downgrade(&self.essence), module)?));
				Ok(())
			},
		}
	}
}

impl ProgramInstanceEssence {
	/// Create new program essence.
	pub fn new() -> Self {
		ProgramInstanceEssence {
			modules: RwLock::new(HashMap::new()),
		}
	}

	/// Get module reference.
	pub fn module(&self, name: &str) -> Option<Arc<ModuleInstance>> {
		self.modules.read().get(name).cloned()
	}
}
