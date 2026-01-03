//! safe wrapper around libvmi ffi

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Result, VmiError};
use crate::ffi::*;

/// wrapper around vmi_instance_t
pub struct Vmi {
    handle: vmi_instance_t,
    paused: AtomicBool,
}

/// os type detected in the VM
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
    /// create Vmi wrapper from raw handle (unsafe)
    pub unsafe fn from_handle(handle: vmi_instance_t) -> Self {
        Self {
            handle,
            paused: AtomicBool::new(false),
        }
    }

    /// get raw handle
    pub fn get_handle(&self) -> vmi_instance_t {
        self.handle
    }

    /// check if CPU supports singlestep (Intel=true, AMD=false)
    pub fn supports_singlestep(&self) -> bool {
        unsafe {
            let status = vmi_toggle_single_step_vcpu(self.handle, ptr::null_mut(), 0, true);
            if status == status_VMI_SUCCESS {
                let _ = vmi_toggle_single_step_vcpu(self.handle, ptr::null_mut(), 0, false);
                return true;
            }
            false
        }
    }

    /// init libvmi with domain name, json profile path, and kvmi socket
    pub(crate) fn new(domain_name: &str, json_path: &str, socket_path: &str) -> Result<Self> {
        let name_cstr = CString::new(domain_name)
            .map_err(|_| VmiError::InitFailed("invalid domain name".into()))?;
        let json_cstr = CString::new(json_path)
            .map_err(|_| VmiError::InitFailed("invalid json path".into()))?;
        let socket_cstr = CString::new(socket_path)
            .map_err(|_| VmiError::InitFailed("invalid socket path".into()))?;

        let mut handle: vmi_instance_t = ptr::null_mut();
        let mut error: vmi_init_error_t = 0;

        // setup init data for kvmi socket - manual alloc for flexible array
        let init_data_ptr = unsafe {
            let header_size = size_of::<vmi_init_data_t>();
            let entry_align = align_of::<vmi_init_data_entry_t>();
            // align offset to entry_align
            let entry_offset = (header_size + entry_align - 1) & !(entry_align - 1);
            let total_size = entry_offset + size_of::<vmi_init_data_entry_t>();

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
                (VMI_INIT_DOMAINNAME | VMI_INIT_EVENTS) as u64,
                init_data_ptr,
                vmi_config_VMI_CONFIG_JSON_PATH,
                json_cstr.as_ptr() as *mut _,
                &mut error,
            )
        };

        // socket_cstr ownership transferred to libvmi
        std::mem::forget(socket_cstr);

        unsafe { libc::free(init_data_ptr as *mut _) };

        if status != status_VMI_SUCCESS {
            return Err(VmiError::InitFailed(format!("error code: {}", error)));
        }

        Ok(Self {
            handle,
            paused: AtomicBool::new(false),
        })
    }

    /// pause vm for consistent memory access
    pub fn pause(&self) -> Result<()> {
        let status = unsafe { vmi_pause_vm(self.handle) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: 0,
                msg: "failed to pause vm".into(),
            });
        }
        self.paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// resume vm after introspection
    pub fn resume(&self) -> Result<()> {
        let status = unsafe { vmi_resume_vm(self.handle) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: 0,
                msg: "failed to resume vm".into(),
            });
        }
        self.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// get os type
    pub fn os_type(&self) -> OsType {
        let os = unsafe { vmi_get_ostype(self.handle) };
        OsType::from(os)
    }

    /// get guest address width in bytes (4 for 32-bit, 8 for 64-bit)
    pub fn address_width(&self) -> u8 {
        unsafe { vmi_get_address_width(self.handle) }
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

    /// get struct member offset from JSON profile via libvmi API
    pub fn get_struct_offset(&self, struct_name: &str, field_name: &str) -> Result<u64> {
        let s_cstr =
            CString::new(struct_name).map_err(|_| VmiError::SymbolNotFound(struct_name.into()))?;
        let m_cstr =
            CString::new(field_name).map_err(|_| VmiError::SymbolNotFound(field_name.into()))?;

        let mut offset: u64 = 0;
        let status = unsafe {
            vmi_get_kernel_struct_offset(self.handle, s_cstr.as_ptr(), m_cstr.as_ptr(), &mut offset)
        };

        if status != status_VMI_SUCCESS {
            return Err(VmiError::SymbolNotFound(format!(
                "{}.{}",
                struct_name, field_name
            )));
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
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "read_addr_va failed".into(),
            });
        }
        Ok(addr)
    }

    /// read 32-bit value at virtual address
    pub fn read_32_va(&self, vaddr: u64, pid: u32) -> Result<u32> {
        let mut val: u32 = 0;
        let status = unsafe { vmi_read_32_va(self.handle, vaddr, pid as i32, &mut val) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "read_32_va failed".into(),
            });
        }
        Ok(val)
    }

    /// read 8-bit value at virtual address
    pub fn read_8_va(&self, vaddr: u64, pid: u32) -> Result<u8> {
        let mut val: u8 = 0;
        let status = unsafe { vmi_read_8_va(self.handle, vaddr, pid as i32, &mut val) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "read_8_va failed".into(),
            });
        }
        Ok(val)
    }

    /// write 8-bit value at virtual address
    pub fn write_8_va(&self, vaddr: u64, pid: u32, val: u8) -> Result<()> {
        let ptr = &val as *const u8;
        let status = unsafe { vmi_write_8_va(self.handle, vaddr, pid as i32, ptr as *mut u8) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "write_8_va failed".into(),
            });
        }
        Ok(())
    }

    /// translate kernel virtual to physical address
    pub fn v2p(&self, vaddr: u64) -> Result<u64> {
        let mut paddr: u64 = 0;
        let status = unsafe { vmi_translate_kv2p(self.handle, vaddr, &mut paddr) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::TranslateFailed { addr: vaddr });
        }
        Ok(paddr)
    }

    /// read 8-bit value at physical address
    pub fn read_8_pa(&self, paddr: u64) -> Result<u8> {
        let mut val: u8 = 0;
        let status = unsafe { vmi_read_8_pa(self.handle, paddr, &mut val) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: paddr,
                msg: "read_8_pa failed".into(),
            });
        }
        Ok(val)
    }

    /// read 16-bit memory at virtual address
    pub fn read_16_va(&self, vaddr: u64, pid: u32) -> Result<u16> {
        let mut val: u16 = 0;
        let status = unsafe { vmi_read_16_va(self.handle, vaddr, pid as i32, &mut val) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "read_16_va failed".into(),
            });
        }
        Ok(val)
    }

    /// read string at virtual address
    pub fn read_str_va(&self, vaddr: u64, pid: u32) -> Result<String> {
        let ptr = unsafe { vmi_read_str_va(self.handle, vaddr, pid as i32) };
        if ptr.is_null() {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "read_str_va returned null".into(),
            });
        }
        let cstr = unsafe { CStr::from_ptr(ptr) };
        let result = cstr.to_string_lossy().into_owned();
        unsafe { libc::free(ptr as *mut _) };
        Ok(result)
    }

    /// read unicode string struct at virtual address
    pub fn read_unicode_string(&self, vaddr: u64, pid: u32) -> Result<String> {
        // manual implementation:
        // avoids FFI complexity of `vmi_read_unicode_str` (requires context structs)
        // by reading UNICODE_STRING Length and Buffer, then reading UTF-16 data.

        let length = self.read_16_va(vaddr, pid).unwrap_or(0);
        let _max_len = self.read_16_va(vaddr + 2, pid).unwrap_or(0);
        // buffer is pointer at offset 8 (on 64-bit)
        let buffer_addr = self.read_addr_va(vaddr + 8, pid).unwrap_or(0);

        if length == 0 || buffer_addr == 0 {
            return Ok(String::new());
        }

        // read UTF-16 bytes
        // length is in bytes
        let mut data = Vec::with_capacity((length / 2) as usize);
        for i in (0..length).step_by(2) {
            let c = self.read_16_va(buffer_addr + i as u64, pid).unwrap_or(0);
            data.push(c);
        }

        // convert
        Ok(String::from_utf16_lossy(&data))
    }

    /// register an event
    pub fn register_event(&self, event: *mut vmi_event_t) -> Result<()> {
        let status = unsafe { vmi_register_event(self.handle, event) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::InitFailed("failed to register event".into()));
        }
        Ok(())
    }

    /// clear an event
    pub fn clear_event(&self, event: *mut vmi_event_t) -> Result<()> {
        let status = unsafe { vmi_clear_event(self.handle, event, None) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: 0,
                msg: "failed to clear event".into(),
            });
        }
        Ok(())
    }

    /// listen for events (blocking)
    pub fn events_listen(&self, timeout: u32) -> Result<()> {
        let status = unsafe { vmi_events_listen(self.handle, timeout) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: 0,
                msg: "error listening for events".into(),
            });
        }
        Ok(())
    }

    /// get vcpu register
    pub fn get_vcpureg(&self, reg: u64, vcpu: u32) -> Result<u64> {
        let mut val: u64 = 0;
        let status = unsafe { vmi_get_vcpureg(self.handle, &mut val, reg, vcpu as u64) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: 0,
                msg: "failed to get vcpu reg".into(),
            });
        }
        Ok(val)
    }

    /// set vcpu register
    pub fn set_vcpureg(&self, reg: u64, val: u64, vcpu: u32) -> Result<()> {
        let status = unsafe { vmi_set_vcpureg(self.handle, val, reg, vcpu as u64) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: 0,
                msg: "failed to set vcpu reg".into(),
            });
        }
        Ok(())
    }

    /// write 16-bit value at virtual address
    pub fn write_16_va(&self, vaddr: u64, pid: u32, val: u16) -> Result<()> {
        let ptr = &val as *const u16;
        let status = unsafe { vmi_write_16_va(self.handle, vaddr, pid as i32, ptr as *mut u16) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "write_16_va failed".into(),
            });
        }
        Ok(())
    }

    /// write 32-bit value at virtual address
    pub fn write_32_va(&self, vaddr: u64, pid: u32, val: u32) -> Result<()> {
        let ptr = &val as *const u32;
        let status = unsafe { vmi_write_32_va(self.handle, vaddr, pid as i32, ptr as *mut u32) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "write_32_va failed".into(),
            });
        }
        Ok(())
    }

    /// write 64-bit value at virtual address
    pub fn write_64_va(&self, vaddr: u64, pid: u32, val: u64) -> Result<()> {
        let ptr = &val as *const u64;
        let status = unsafe { vmi_write_64_va(self.handle, vaddr, pid as i32, ptr as *mut u64) };
        if status != status_VMI_SUCCESS {
            return Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "write_64_va failed".into(),
            });
        }
        Ok(())
    }
}

/// wrapper for vmi_event_t to clean up usage
pub struct VmiEvent {
    pub inner: vmi_event_t,
}

impl VmiEvent {
    pub fn new(version: u32) -> Self {
        let mut inner: vmi_event_t = unsafe { std::mem::zeroed() };
        inner.version = version;
        Self { inner }
    }

    pub fn set_interrupt(&mut self, intr: u32, gfn: u64, offset: u64) {
        self.inner.type_ = VMI_EVENT_INTERRUPT as u16;
        self.inner.__bindgen_anon_1.interrupt_event.intr = intr as u8;
        self.inner
            .__bindgen_anon_1
            .interrupt_event
            .__bindgen_anon_1
            .__bindgen_anon_1
            .gfn = gfn;
        self.inner
            .__bindgen_anon_1
            .interrupt_event
            .__bindgen_anon_1
            .__bindgen_anon_1
            .offset = offset;
    }

    pub fn set_singlestep(&mut self, vcpu_id: u32) {
        self.inner.type_ = VMI_EVENT_SINGLESTEP as u16;
        // ss_event is the field name for single_step_event in the union
        self.inner.__bindgen_anon_1.ss_event.vcpus = vcpu_id;
    }

    pub fn set_mem_event(&mut self, gfn: u64, access: u32, gla: u64) {
        self.inner.type_ = VMI_EVENT_MEMORY as u16;
        self.inner.__bindgen_anon_1.mem_event.gfn = gfn;
        self.inner.__bindgen_anon_1.mem_event.in_access = access as u8;
        self.inner.__bindgen_anon_1.mem_event.gla = gla;
    }

    pub fn set_callback(&mut self, cb: event_callback_t) {
        self.inner.callback = cb;
    }

    pub fn set_data<T>(&mut self, data: *mut T) {
        self.inner.data = data as *mut std::ffi::c_void;
    }

    pub fn as_mut_ptr(&mut self) -> *mut vmi_event_t {
        &mut self.inner
    }

    pub fn get_vcpu_id(&self) -> u32 {
        self.inner.vcpu_id
    }

    /// get x86 registers pointer from event
    pub unsafe fn get_x86_regs(&self) -> *mut x86_regs {
        unsafe { self.inner.__bindgen_anon_2.__bindgen_anon_1.x86_regs }
    }

    /// set reinject flag for interrupt events
    pub unsafe fn set_reinject(&mut self, reinject: i8) {
        self.inner
            .__bindgen_anon_1
            .interrupt_event
            .__bindgen_anon_1
            .__bindgen_anon_1
            .reinject = reinject;
    }

    /// get gfn from mem_event
    pub unsafe fn get_mem_event_gfn(&self) -> u64 {
        unsafe { self.inner.__bindgen_anon_1.mem_event.gfn }
    }

    /// configure generic memory event (for AMD path)
    pub fn set_generic_mem_event(&mut self, gfn: u64, access: u8, generic: u8) {
        self.inner.type_ = VMI_EVENT_MEMORY as u16;
        self.inner.__bindgen_anon_1.mem_event.gfn = gfn;
        self.inner.__bindgen_anon_1.mem_event.in_access = access;
        self.inner.__bindgen_anon_1.mem_event.generic = generic;
    }
}

/// helper functions for raw vmi_event_t pointers (used in FFI callbacks)
pub mod event_helpers {
    use crate::ffi::{vmi_event_t, x86_regs};

    /// set reinject flag on raw event pointer
    pub unsafe fn set_reinject(event: *mut vmi_event_t, val: i8) {
        unsafe {
            (*event)
                .__bindgen_anon_1
                .interrupt_event
                .__bindgen_anon_1
                .__bindgen_anon_1
                .reinject = val;
        }
    }

    /// get x86_regs pointer from raw event
    pub unsafe fn get_x86_regs(event: *mut vmi_event_t) -> *mut x86_regs {
        unsafe { (*event).__bindgen_anon_2.__bindgen_anon_1.x86_regs }
    }

    /// get mem_event gfn from raw event
    pub unsafe fn get_mem_gfn(event: *mut vmi_event_t) -> u64 {
        unsafe { (*event).__bindgen_anon_1.mem_event.gfn }
    }
}

impl Vmi {
    /// translate virtual address to physical address using specific DTB
    pub fn translate_uv2p(&self, dtb: u64, vaddr: u64) -> Result<u64> {
        let mut paddr: addr_t = 0;
        let status = unsafe { vmi_pagetable_lookup(self.handle, dtb, vaddr, &mut paddr) };
        if status == status_VMI_SUCCESS {
            Ok(paddr)
        } else {
            Err(VmiError::ReadFailed {
                addr: vaddr,
                msg: "Page table lookup failed".into(),
            })
        }
    }

    /// translate kernel virtual address to physical address
    pub fn translate_kv2p(&self, vaddr: u64) -> Result<u64> {
        let mut paddr: addr_t = 0;
        let status = unsafe { vmi_translate_kv2p(self.handle, vaddr, &mut paddr) };
        if status == status_VMI_SUCCESS {
            Ok(paddr)
        } else {
            Err(VmiError::TranslateFailed { addr: vaddr })
        }
    }

    /// read physical memory
    pub fn read_pa(&self, paddr: u64, length: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; length];
        let mut read: usize = 0;
        let status = unsafe {
            vmi_read_pa(
                self.handle,
                paddr,
                length,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                &mut read,
            )
        };
        if status == status_VMI_SUCCESS && read == length {
            Ok(buffer)
        } else {
            Err(VmiError::ReadFailed {
                addr: paddr,
                msg: "Physical read failed".into(),
            })
        }
    }

    /// read unicode string using a specific DTB (for new processes not in PID cache)
    pub fn read_unicode_string_dtb(&self, dtb: u64, vaddr: u64) -> Result<String> {
        // read length (first 2 bytes)
        let len_pa = self.translate_uv2p(dtb, vaddr)?;
        let len_buf = self.read_pa(len_pa, 2)?;
        let length = u16::from_le_bytes([len_buf[0], len_buf[1]]) as usize;

        if length == 0 {
            return Ok(String::new());
        }
        if length > 4096 {
            return Ok("<too_long>".into());
        }

        // read buffer address (offset 8 on x64)
        let buf_ptr_pa = self.translate_uv2p(dtb, vaddr + 8)?;
        let buf_ptr_raw = self.read_pa(buf_ptr_pa, 8)?;
        let buf_vaddr = u64::from_le_bytes([
            buf_ptr_raw[0],
            buf_ptr_raw[1],
            buf_ptr_raw[2],
            buf_ptr_raw[3],
            buf_ptr_raw[4],
            buf_ptr_raw[5],
            buf_ptr_raw[6],
            buf_ptr_raw[7],
        ]);

        if buf_vaddr == 0 {
            return Ok(String::new());
        }

        let mut data = Vec::with_capacity(length);
        let mut curr_vaddr = buf_vaddr;
        let end_vaddr = buf_vaddr + length as u64;

        while curr_vaddr < end_vaddr {
            // translate current page
            let paddr = self.translate_uv2p(dtb, curr_vaddr)?;
            // how much can we read in this page?
            let page_offset = curr_vaddr & 0xFFF;
            let remainder = 0x1000 - page_offset;
            let to_read = std::cmp::min(remainder, end_vaddr - curr_vaddr);

            let chunk = self.read_pa(paddr, to_read as usize)?;
            data.extend_from_slice(&chunk);

            curr_vaddr += to_read;
        }

        // convert UTF-16
        let u16s: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();

        Ok(String::from_utf16_lossy(&u16s))
    }

    pub fn pause_vm(&self) -> Result<()> {
        let status = unsafe { vmi_pause_vm(self.handle) };
        if status == status_VMI_SUCCESS {
            Ok(())
        } else {
            Err(VmiError::VmControlFailed)
        }
    }

    pub fn resume_vm(&self) -> Result<()> {
        let status = unsafe { vmi_resume_vm(self.handle) };
        if status == status_VMI_SUCCESS {
            Ok(())
        } else {
            Err(VmiError::VmControlFailed)
        }
    }
}

unsafe impl Send for Vmi {}
unsafe impl Sync for Vmi {}

impl Drop for Vmi {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                // only resume if we are actually paused to avoid heap corruption in libvmi
                if self.paused.load(Ordering::SeqCst) {
                    vmi_resume_vm(self.handle);
                }
                vmi_destroy(self.handle);
            }
            self.handle = ptr::null_mut();
        }
    }
}
