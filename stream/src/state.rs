use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
use memmap2::Mmap;

#[derive(Debug, Clone, Default)]
pub struct FramebufferMetadata {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
}

pub struct Framebuffer {
    pub _mmap: Option<Mmap>,
    /// writable buffer for capture mode when mmap not available
    pub capture_buffer: Vec<u8>,
    pub metadata: FramebufferMetadata,
}

pub struct VmState {
    pub fb: Mutex<Framebuffer>,
    /// atomic dirty flag - dbus Update just sets this
    pub dirty: AtomicBool,
}

impl VmState {
    pub fn new() -> Self {
        Self {
            fb: Mutex::new(Framebuffer {
                _mmap: None,
                capture_buffer: Vec::new(),
                metadata: FramebufferMetadata::default(),
            }),
            dirty: AtomicBool::new(false),
        }
    }

    /// mark frame as dirty - called from dbus update signal
    #[inline]
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// check and clear dirty flag
    #[inline]
    pub fn check_and_clear_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }
}
