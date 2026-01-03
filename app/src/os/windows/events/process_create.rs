//! process creation monitor - hooks PspInsertProcess
//!
//! uses HookManager for AMD-compatible hook handling

use std::sync::{Arc, Mutex};
use crate::vmi::Vmi;
use crate::hook::{HookManager, HookContext};
use crate::error::Result;
use crate::ffi::RCX;
use crate::os::{Event, EventContext};

/// offsets needed for reading process info
struct ProcessOffsets {
    pid_offset: u64,
    parent_pid_offset: u64,
    create_time_offset: u64,
    dtb_offset: u64,
    peb_offset: u64,
    process_params_offset: u64,
    command_line_offset: u64,
    image_path_offset: u64,
}

/// process creation monitor
pub struct ProcessCreateMonitor {
    hook_addr: Option<u64>,
}

impl Event for ProcessCreateMonitor {
    fn enable(&mut self, ctx: &EventContext) -> Result<()> {
        self.enable_internal(ctx.hooks, ctx.vmi)
    }

    fn disable(&mut self, ctx: &EventContext) -> Result<()> {
        self.disable_internal(ctx.hooks, ctx.vmi)
    }
}

impl ProcessCreateMonitor {
    pub fn new() -> Self {
        Self { hook_addr: None }
    }
    
    /// enable process monitoring - registers hook with HookManager
    fn enable_internal(&mut self, hooks: &Arc<HookManager>, vmi: &Arc<Mutex<Vmi>>) -> Result<()> {
        if self.hook_addr.is_some() { return Ok(()); }
        
        let func_addr = {
            let vmi_lock = vmi.lock().unwrap();
            // find hook target
            vmi_lock.ksym2v("PspInsertProcess")
                .or_else(|_| vmi_lock.ksym2v("NtCreateUserProcess")) 
                .map_err(|_| crate::error::VmiError::SymbolNotFound("PspInsertProcess".into()))?
        };
        
        // load offsets once
        let offsets = {
            let vmi_lock = vmi.lock().unwrap();
            Arc::new(ProcessOffsets {
                pid_offset: vmi_lock.get_offset("win_pid")?,
                parent_pid_offset: vmi_lock.get_struct_offset("_EPROCESS", "InheritedFromUniqueProcessId")?,
                create_time_offset: vmi_lock.get_struct_offset("_EPROCESS", "CreateTime")?,
                dtb_offset: vmi_lock.get_struct_offset("_KPROCESS", "DirectoryTableBase")?,
                peb_offset: vmi_lock.get_struct_offset("_EPROCESS", "Peb")?,
                process_params_offset: vmi_lock.get_struct_offset("_PEB", "ProcessParameters")?,
                command_line_offset: vmi_lock.get_struct_offset("_RTL_USER_PROCESS_PARAMETERS", "CommandLine")?,
                image_path_offset: vmi_lock.get_struct_offset("_RTL_USER_PROCESS_PARAMETERS", "ImagePathName")?,
            })
        };
        
        // callback closure captures offsets
        let offsets_clone = offsets.clone();
        
        {
            let vmi_lock = vmi.lock().unwrap();
            
            hooks.add_hook(&vmi_lock, func_addr, move |ctx: &HookContext| {
                Self::on_process_create(ctx, &offsets_clone);
            })?;
        }
        
        self.hook_addr = Some(func_addr);
        eprintln!("[ProcessCreateMonitor] Enabled on PspInsertProcess @ {:#x}", func_addr);
        Ok(())
    }
    
    /// disable monitoring
    fn disable_internal(&mut self, hooks: &Arc<HookManager>, vmi: &Arc<Mutex<Vmi>>) -> Result<()> {
        if let Some(addr) = self.hook_addr.take() {
            let vmi_lock = vmi.lock().unwrap();
            hooks.remove_hook(&vmi_lock, addr)?;
            eprintln!("[ProcessCreateMonitor] Disabled");
        }
        Ok(())
    }
    
    /// callback when PspInsertProcess is hit
    fn on_process_create(ctx: &HookContext, offsets: &ProcessOffsets) {
        // RCX = EPROCESS pointer per MSVC x64 ABI
        let eprocess_addr = match ctx.vmi.get_vcpureg(RCX as u64, ctx.vcpu_id) {
            Ok(addr) => addr,
            Err(_) => return,
        };
        
        let vmi = ctx.vmi;
        
        // read process info
        let pid = vmi.read_32_va(eprocess_addr + offsets.pid_offset, 0).unwrap_or(0);
        let ppid = vmi.read_addr_va(eprocess_addr + offsets.parent_pid_offset, 0).unwrap_or(0) as u32;
        let create_time = vmi.read_addr_va(eprocess_addr + offsets.create_time_offset, 0).unwrap_or(0);
        
        // DTB for user-space access
        let dtb = vmi.read_addr_va(eprocess_addr + offsets.dtb_offset, 0).unwrap_or(0);
        
        let mut cmd_line = String::from("<unknown>");
        let mut image_path = String::from("<unknown>");
        
        if dtb != 0 {
            if let Ok(peb_addr) = vmi.read_addr_va(eprocess_addr + offsets.peb_offset, 0) {
                if peb_addr != 0 {
                    // PEB in user space, need DTB for translation
                    if let Ok(peb_pa) = vmi.translate_uv2p(dtb, peb_addr) {
                        let params_ptr_bytes = vmi.read_pa(peb_pa + offsets.process_params_offset, 8).unwrap_or_default();
                        let params_addr = u64::from_le_bytes(params_ptr_bytes.try_into().unwrap_or([0;8]));
                        
                        if params_addr != 0 {
                            if let Ok(s) = vmi.read_unicode_string_dtb(dtb, params_addr + offsets.command_line_offset) {
                                if !s.is_empty() { cmd_line = s; }
                            }
                            if let Ok(s) = vmi.read_unicode_string_dtb(dtb, params_addr + offsets.image_path_offset) {
                                if !s.is_empty() { image_path = s; }
                            }
                        }
                    }
                }
            }
        }
        
        println!(
            "Process Create | PID: {} | PPID: {} | Image: {} | CmdLine: {} | Time: {}",
            pid, ppid, image_path, cmd_line, create_time
        );
    }
}
