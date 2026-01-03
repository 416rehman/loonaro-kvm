//! error types for loonaro-vmi

use thiserror::Error;

#[derive(Error, Debug)]
pub enum VmiError {
    #[error("LibVMI initialization failed: {0}")]
    InitFailed(String),
    
    #[error("Failed to read memory at {addr:#x}: {msg}")]
    ReadFailed { addr: u64, msg: String },
    
    #[error("Failed to translate address {addr:#x}")]
    TranslateFailed { addr: u64 },
    
    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),
    
    #[error("Invalid UTF-8 in process name")]
    InvalidProcessName,
    
    #[error("Null pointer returned from LibVMI")]
    NullPointer,

    #[error("Failed to pause/resume VM")]
    VmControlFailed,
    
    #[error("Hook already exists at {0:#x}")]
    HookExists(u64),
    
    #[error("Failed to set memory access for GFN {0:#x}")]
    MemAccessFailed(u64),

    #[error("Error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, VmiError>;
