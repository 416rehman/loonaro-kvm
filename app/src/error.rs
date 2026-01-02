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
}

pub type Result<T> = std::result::Result<T, VmiError>;
