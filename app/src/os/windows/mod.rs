use crate::vmi::Vmi;

pub mod actions;
pub mod events;

use super::Os;

pub struct WindowsOs {
    vmi: Vmi,
}

impl WindowsOs {
    // custom new removed to avoid double-free. use Os::new(vmi) instead.
}

impl Os for WindowsOs {
    fn new(vmi: Vmi) -> Self {
        Self { vmi }
    }

    fn vmi(&self) -> &Vmi {
        &self.vmi
    }
}
