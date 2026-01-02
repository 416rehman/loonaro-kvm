

//! safe wrapper around libvmi ffi

use std::ffi::{CStr, CString};
use std::ptr;

use crate::error::{Result, VmiError};
use crate::ffi::*;

/// wrapper around vmi_instance_t
pub struct Vmi {
    handle: vmi_instance_t,
}

/// OS type detected in the VM
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsType {
    Linux,
    Windows,
    FreeBSD,
    Osx,
    Unknown,
}

impl From<os_t> for OsType {
    fn from(os: os_t) -> Self {
        match os {
            os_VMI_OS_LINUX => OsType::Linux,
            os_VMI_OS_WINDOWS => OsType::Windows,
            os_VMI_OS_FREEBSD => OsType::FreeBSD,
            os_VMI_OS_OSX => OsType::Osx,
            _ => OsType::Unknown,
        }
    }
}



impl Vmi {
    /// init libvmi with domain name, json profile path, and kvmi socket
    pub fn new(domain_name: &str, json_path: &str, socket_path: &str) -> Result<Self> {
        let name_cstr = CString::new(domain_name).map_err(|_| VmiError::InitFailed("invalid domain name".into()))?;
        let json_cstr = CString::new(json_path).map_err(|_| VmiError::InitFailed("invalid json path".into()))?;
        let socket_cstr = CString::new(socket_path).map_err(|_| VmiError::InitFailed("invalid socket path".into()))?;
        
        let mut handle: vmi_instance_t = ptr::null_mut();
        let mut error: vmi_init_error_t = 0;
        
        // setup init data for kvmi socket
        // we need to allocate this manually because of the flexible array
        let init_data_ptr = unsafe {
            let header_size = std::mem::size_of::<vmi_init_data_t>();
            let entry_align = std::mem::align_of::<vmi_init_data_entry_t>();
            // align offset to entry_align
            let entry_offset = (header_size + entry_align - 1) & !(entry_align - 1);
            let total_size = entry_offset + std::mem::size_of::<vmi_init_data_entry_t>();
            
            let ptr = libc::calloc(1, total_size) as *mut vmi_init_data_t;
            if ptr.is_null() {
                return Err(VmiError::InitFailed("failed to allocate init data".into()));
            }
            (*ptr).count = 1;
            let entry_ptr = (ptr as *mut u8).add(entry_offset) as *mut vmi_init_data_entry_t;
            (*entry_ptr).type_ = vmi_init_data_type_t_VMI_INIT_DATA_KVMI_SOCKET as u64;
            (*entry_ptr).data = socket_cstr.as_ptr() as *mut _;
            ptr
        };
        
        let status = unsafe {
            vmi_init_complete(
                &mut handle,
                name_cstr.as_ptr() as *mut _,
                VMI_INIT_DOMAINNAME as u64, // Cast to u64
                init_data_ptr,
                vmi_config_VMI_CONFIG_JSON_PATH,
                json_cstr.as_ptr() as *mut _,
                &mut error,
            )
        };
        
        // dont free socket_cstr yet
        std::mem::forget(socket_cstr);
        
        unsafe { libc::free(init_data_ptr as *mut _) };
        
        if status != status_VMI_SUCCESS { // Use correct constant
            return Err(VmiError::InitFailed(format!("error code: {}", error)));
        }
        
        Ok(Self { handle })
    }
    
    /// pause vm for consistent memory access
    pub fn pause(&self) -> Result<()> {
        let status = unsafe { vmi_pause_vm(self.handle) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed { addr: 0, msg: "failed to pause vm".into() });
        }
        Ok(())
    }
    
    /// resume vm after introspection
    pub fn resume(&self) -> Result<()> {
        let status = unsafe { vmi_resume_vm(self.handle) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed { addr: 0, msg: "failed to resume vm".into() });
        }
        Ok(())
    }
    
    /// get os type
    pub fn os_type(&self) -> OsType {
        let os = unsafe { vmi_get_ostype(self.handle) };
        OsType::from(os)
    }
    
    /// get vm name
    pub fn name(&self) -> Option<String> {
        let name_ptr = unsafe { vmi_get_name(self.handle) };
        if name_ptr.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(name_ptr) };
        let result = name.to_string_lossy().into_owned();
        unsafe { libc::free(name_ptr as *mut _) };
        Some(result)
    }
    
    /// get vm id
    pub fn vmid(&self) -> u64 {
        unsafe { vmi_get_vmid(self.handle) }
    }
    
    /// get offset from config
    pub fn get_offset(&self, name: &str) -> Result<u64> {
        let name_cstr = CString::new(name).map_err(|_| VmiError::SymbolNotFound(name.into()))?;
        let mut offset: u64 = 0;
        let status = unsafe { vmi_get_offset(self.handle, name_cstr.as_ptr(), &mut offset) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::SymbolNotFound(name.into()));
        }
        Ok(offset)
    }
    
    /// translate kernel symbol to virtual address
    pub fn ksym2v(&self, symbol: &str) -> Result<u64> {
        let sym_cstr = CString::new(symbol).map_err(|_| VmiError::SymbolNotFound(symbol.into()))?;
        let mut addr: u64 = 0;
        let status = unsafe { vmi_translate_ksym2v(self.handle, sym_cstr.as_ptr(), &mut addr) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::SymbolNotFound(symbol.into()));
        }
        Ok(addr)
    }
    
    /// read address at kernel symbol
    pub fn read_addr_ksym(&self, symbol: &str) -> Result<u64> {
        let sym_cstr = CString::new(symbol).map_err(|_| VmiError::SymbolNotFound(symbol.into()))?;
        let mut addr: u64 = 0;
        let status = unsafe { vmi_read_addr_ksym(self.handle, sym_cstr.as_ptr(), &mut addr) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::SymbolNotFound(symbol.into()));
        }
        Ok(addr)
    }
    
    /// read address at virtual address
    pub fn read_addr_va(&self, vaddr: u64, pid: u32) -> Result<u64> {
        let mut addr: u64 = 0;
        let status = unsafe { vmi_read_addr_va(self.handle, vaddr, pid as i32, &mut addr) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed { addr: vaddr, msg: "read_addr_va failed".into() });
        }
        Ok(addr)
    }
    
    /// read 32-bit value at virtual address
    pub fn read_32_va(&self, vaddr: u64, pid: u32) -> Result<u32> {
        let mut val: u32 = 0;
        let status = unsafe { vmi_read_32_va(self.handle, vaddr, pid as i32, &mut val) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed { addr: vaddr, msg: "read_32_va failed".into() });
        }
        Ok(val)
    }
    
    /// read string at virtual address
    pub fn read_str_va(&self, vaddr: u64, pid: u32) -> Result<String> {
        let ptr = unsafe { vmi_read_str_va(self.handle, vaddr, pid as i32) };
        if ptr.is_null() {
            return Err(VmiError::ReadFailed { addr: vaddr, msg: "read_str_va returned null".into() });
        }
        let cstr = unsafe { CStr::from_ptr(ptr) };
        let result = cstr.to_string_lossy().into_owned();
        unsafe { libc::free(ptr as *mut _) };
        Ok(result)
    }
    

}

impl Drop for Vmi {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                vmi_resume_vm(self.handle);
                vmi_destroy(self.handle);
            }
        }
    }
}
