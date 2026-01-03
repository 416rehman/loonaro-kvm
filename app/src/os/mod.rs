pub mod windows;

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: i32,
    pub name: String,
    pub addr: u64,
}

use crate::error::Result;
use crate::hook::HookManager;
use crate::vmi::Vmi;
use std::sync::{Arc, Mutex};

/// context passed to events for enabling/disabling
pub struct EventContext<'a> {
    pub vmi: &'a Arc<Mutex<Vmi>>,
    pub hooks: &'a Arc<HookManager>,
}

/// trait for actions that perform a specific operation (e.g. list processes)
pub trait Action<T> {
    fn execute(&self, vmi: &Vmi) -> Result<T>;
}

/// trait for events that can be enabled/disabled (e.g. process monitoring)
/// implementations handle cleanup on Drop.
pub trait Event: Send {
    fn enable(&mut self, ctx: &EventContext) -> Result<()>;
    fn disable(&mut self, ctx: &EventContext) -> Result<()>;
}

/// trait for OS abstractions
pub trait Os {
    fn new(vmi: Vmi) -> Self;
    fn vmi(&self) -> &Vmi;
    fn execute<A: Action<T>, T>(&self, action: A) -> Result<T> {
        action.execute(self.vmi())
    }
    fn enable_event<E: Event>(&self, _event: &mut E) -> Result<()> {
        Err(crate::error::VmiError::InitFailed(
            "Os trait does not support events directly".into(),
        ))
    }
    fn disable_event<E: Event>(&self, _event: &mut E) -> Result<()> {
        Err(crate::error::VmiError::InitFailed(
            "Not implemented for Os trait".into(),
        ))
    }
}
