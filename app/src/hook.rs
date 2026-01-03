//! hook manager - INT3 hooks with dynamic instruction emulation

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, RwLock};

use crate::disasm::{self, EmulationStrategy};
use crate::error::{Result, VmiError};
use crate::ffi::{
    event_response_t, vmi_event_t, vmi_instance_t, INT3, RIP, RSP,
    VMI_EVENTS_VERSION, VMI_EVENT_RESPONSE_SET_REGISTERS,
};
use crate::vmi::{event_helpers, Vmi, VmiEvent};

/// context passed to hook callbacks
pub struct HookContext<'a> {
    pub vmi: &'a Vmi,
    pub vcpu_id: u32,
    pub rip: u64,
    pub regs: *mut crate::ffi::x86_regs,
}

impl HookContext<'_> {
    pub fn with_vmi<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Vmi) -> R,
    {
        f(self.vmi)
    }
}

pub type HookCallback = Box<dyn Fn(&HookContext) + Send + Sync>;

struct Hook {
    addr: u64,
    orig_byte: u8,
    callback: HookCallback,
    strategy: Option<EmulationStrategy>,
}

struct HookState {
    hooks: HashMap<u64, Hook>,
}

pub struct HookManager {
    vmi: Arc<Mutex<Vmi>>,
    state: Arc<RwLock<HookState>>,
    int_event: *mut VmiEvent,
    mgr_ptr: Mutex<Option<*const HookManager>>,
}

unsafe impl Send for HookManager {}
unsafe impl Sync for HookManager {}

impl HookManager {
    pub fn init(vmi: Arc<Mutex<Vmi>>) -> Result<Arc<Self>> {
        let state = Arc::new(RwLock::new(HookState {
            hooks: HashMap::new(),
        }));

        let int_event = Box::into_raw(Box::new(VmiEvent::new(VMI_EVENTS_VERSION)));

        let mgr = Arc::new(Self {
            vmi: vmi.clone(),
            state,
            int_event,
            mgr_ptr: Mutex::new(None),
        });

        let mgr_ptr = Arc::into_raw(mgr.clone());
        {
            let mut p = mgr.mgr_ptr.lock().unwrap();
            *p = Some(mgr_ptr);
        }

        unsafe {
            let vmi_lock = vmi.lock().unwrap();
            (*int_event).set_interrupt(INT3, 0, 0);
            (*int_event).set_callback(Some(Self::interrupt_cb));
            (*int_event).set_data(mgr_ptr as *mut c_void);
            vmi_lock.register_event((*int_event).as_mut_ptr())?;
        }

        eprintln!("[HookManager] initialized");
        Ok(mgr)
    }

    pub fn add_hook<F>(&self, vmi_lock: &Vmi, addr: u64, callback: F) -> Result<()>
    where
        F: Fn(&HookContext) + Send + Sync + 'static,
    {
        let mut state = self.state.write().unwrap();

        if state.hooks.contains_key(&addr) {
            return Err(VmiError::HookExists(addr));
        }

        let phys = vmi_lock.v2p(addr)?;
        let orig_byte = vmi_lock.read_8_pa(phys)?;

        // if the byte is already 0xCC, we might be overlapping with another hook
        // or a previous crashed session. we cannot safely hook this without the real orig_byte.
        if orig_byte == 0xCC {
            return Err(VmiError::Other(format!(
                "intent3 already at {:#x}, previous session may have crashed?",
                addr
            )));
        }

        // read 16 bytes for instruction decode (max x86 instr is 15)
        let mut code_bytes = [0u8; 16];
        for i in 0..16 {
            if let Ok(b) = vmi_lock.read_8_va(addr + i as u64, 0) {
                code_bytes[i] = b;
            } else {
                break;
            }
        }

        // use guest bitness for correct decoding - matters for 32 vs 64 bit
        let bitness = disasm::Bitness::from_address_width(vmi_lock.address_width());
        let strategy = match disasm::analyze_instruction(&code_bytes, addr, bitness) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[HookManager] disasm failed at {:#x}: {}", addr, e);
                None
            }
        };

        if let Some(ref s) = strategy {
            eprintln!(
                "[HookManager] Auto-Emulation enabled for {:#x}: {:?}",
                addr, s
            );
        } else {
            eprintln!(
                "[HookManager] no emulation for {:#x}, hook is one-shot",
                addr
            );
        }

        vmi_lock.write_8_va(addr, 0, 0xCC)?;

        state.hooks.insert(
            addr,
            Hook {
                addr,
                orig_byte,
                callback: Box::new(callback),
                strategy,
            },
        );

        eprintln!("[HookManager] Hook added at {:#x}", addr);
        Ok(())
    }

    pub fn remove_hook(&self, vmi_lock: &Vmi, addr: u64) -> Result<()> {
        let mut state = self.state.write().unwrap();
        if let Some(hook) = state.hooks.remove(&addr) {
            vmi_lock.write_8_va(addr, 0, hook.orig_byte)?;
            eprintln!("[HookManager] Hook removed at {:#x}", addr);
        }
        Ok(())
    }

    /// restore all hooks and clear event. must be called before dropping the session.
    pub fn shutdown(&self) {
        let vmi = self.vmi.lock().unwrap();
        let mut state = self.state.write().unwrap();

        if state.hooks.is_empty() {
            return;
        }

        eprintln!(
            "[HookManager] restoring {} hooks during shutdown...",
            state.hooks.len()
        );
        for (_, hook) in state.hooks.drain() {
            if let Err(e) = vmi.write_8_va(hook.addr, 0, hook.orig_byte) {
                eprintln!("[HookManager] restore failed at {:#x}: {}", hook.addr, e);
            }
        }

        if !self.int_event.is_null() {
            let _ = vmi.clear_event(self.int_event as *mut _);
        }

        // recover the Arc to decrement count and allow Drop to run
        let mut p = self.mgr_ptr.lock().unwrap();
        if let Some(ptr) = p.take() {
            unsafe {
                let _ = Arc::from_raw(ptr);
            }
        }
    }

    unsafe extern "C" fn interrupt_cb(
        vmi_handle: vmi_instance_t,
        event: *mut vmi_event_t,
    ) -> event_response_t {
        unsafe {
            event_helpers::set_reinject(event, 1);

            let data = (*event).data as *const HookManager;
            if data.is_null() {
                return 0;
            }

            let mgr = &*data;
            let vmi_events = ManuallyDrop::new(Vmi::from_handle(vmi_handle));

            let vcpu_id = (*event).vcpu_id;
            let rip = match vmi_events.get_vcpureg(RIP as u64, vcpu_id) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("[HookManager] RIP read failed: {:?}", e);
                    return 0;
                }
            };

            let state = mgr.state.read().unwrap();

            let hook_data = state.hooks.get(&rip).map(|h| (h.addr, h.orig_byte));

            if let Some((addr, orig_byte)) = hook_data {
                event_helpers::set_reinject(event, 0);

                if let Some(hook) = state.hooks.get(&rip) {
                    let ctx = HookContext {
                        vmi: &vmi_events,
                        vcpu_id,
                        rip,
                        regs: event_helpers::get_x86_regs(event),
                    };
                    (hook.callback)(&ctx);

                    if let Some(strategy) = &hook.strategy {
                        match strategy {
                            EmulationStrategy::MoveToMem {
                                src_reg,
                                base_reg,
                                displacement,
                                len,
                                operand_size_bits,
                            } => {
                                let execute_emulation = || -> Result<()> {
                                    let src_val = vmi_events.get_vcpureg(*src_reg, vcpu_id)?;
                                    let base_val = vmi_events.get_vcpureg(*base_reg, vcpu_id)?;
                                    let target = base_val.wrapping_add(*displacement as u64);

                                    match operand_size_bits {
                                        8 => vmi_events.write_8_va(target, 0, src_val as u8)?,
                                        16 => vmi_events.write_16_va(target, 0, src_val as u16)?,
                                        32 => vmi_events.write_32_va(target, 0, src_val as u32)?,
                                        64 => vmi_events.write_64_va(target, 0, src_val)?,
                                        _ => {
                                            return Err(VmiError::Other(format!(
                                                "unsupported operand size {}",
                                                operand_size_bits
                                            )));
                                        }
                                    }

                                    (*event_helpers::get_x86_regs(event)).rip = rip + len;
                                    Ok(())
                                };

                                if let Err(e) = execute_emulation() {
                                    eprintln!(
                                        "[HookManager] emulation failed: {}, removing hook",
                                        e
                                    );
                                    let _ = vmi_events.write_8_va(addr, 0, orig_byte);
                                    event_helpers::set_reinject(event, 1);
                                } else {
                                    return VMI_EVENT_RESPONSE_SET_REGISTERS;
                                }
                            }
                            EmulationStrategy::Push { src_reg, len } => {
                                let execute_emulation = || -> Result<()> {
                                    let src_val = vmi_events.get_vcpureg(*src_reg, vcpu_id)?;
                                    let mut rsp = vmi_events.get_vcpureg(RSP as u64, vcpu_id)?;
                                    rsp = rsp.wrapping_sub(8);
                                    vmi_events.write_64_va(rsp, 0, src_val)?;
                                    (*event_helpers::get_x86_regs(event)).rip = rip + len;
                                    vmi_events.set_vcpureg(RSP as u64, rsp, vcpu_id)?;
                                    Ok(())
                                };

                                if let Err(e) = execute_emulation() {
                                    eprintln!(
                                        "[HookManager] emulation failed: {}, removing hook",
                                        e
                                    );
                                    let _ = vmi_events.write_8_va(addr, 0, orig_byte);
                                    event_helpers::set_reinject(event, 1);
                                } else {
                                    return VMI_EVENT_RESPONSE_SET_REGISTERS;
                                }
                            }
                            EmulationStrategy::MovRegReg {
                                dst_reg,
                                src_reg,
                                len,
                            } => {
                                let execute_emulation = || -> Result<()> {
                                    let src_val = vmi_events.get_vcpureg(*src_reg, vcpu_id)?;
                                    vmi_events.set_vcpureg(*dst_reg, src_val, vcpu_id)?;
                                    (*event_helpers::get_x86_regs(event)).rip = rip + len;
                                    Ok(())
                                };

                                if let Err(e) = execute_emulation() {
                                    eprintln!(
                                        "[HookManager] emulation failed: {}, removing hook",
                                        e
                                    );
                                    let _ = vmi_events.write_8_va(addr, 0, orig_byte);
                                    event_helpers::set_reinject(event, 1);
                                } else {
                                    return VMI_EVENT_RESPONSE_SET_REGISTERS;
                                }
                            }
                            EmulationStrategy::SubImm { reg, imm, len } => {
                                let execute_emulation = || -> Result<()> {
                                    let val = vmi_events.get_vcpureg(*reg, vcpu_id)?;
                                    vmi_events.set_vcpureg(
                                        *reg,
                                        val.wrapping_sub(*imm),
                                        vcpu_id,
                                    )?;
                                    (*event_helpers::get_x86_regs(event)).rip = rip + len;
                                    Ok(())
                                };

                                if let Err(e) = execute_emulation() {
                                    eprintln!(
                                        "[HookManager] emulation failed: {}, removing hook",
                                        e
                                    );
                                    let _ = vmi_events.write_8_va(addr, 0, orig_byte);
                                    event_helpers::set_reinject(event, 1);
                                } else {
                                    return VMI_EVENT_RESPONSE_SET_REGISTERS;
                                }
                            }
                            EmulationStrategy::Lea {
                                dst_reg,
                                base_reg,
                                displacement,
                                len,
                            } => {
                                let execute_emulation = || -> Result<()> {
                                    let base_val = vmi_events.get_vcpureg(*base_reg, vcpu_id)?;
                                    let result = base_val.wrapping_add(*displacement as u64);
                                    vmi_events.set_vcpureg(*dst_reg, result, vcpu_id)?;
                                    (*event_helpers::get_x86_regs(event)).rip = rip + len;
                                    Ok(())
                                };

                                if let Err(e) = execute_emulation() {
                                    eprintln!(
                                        "[HookManager] emulation failed: {}, removing hook",
                                        e
                                    );
                                    let _ = vmi_events.write_8_va(addr, 0, orig_byte);
                                    event_helpers::set_reinject(event, 1);
                                } else {
                                    return VMI_EVENT_RESPONSE_SET_REGISTERS;
                                }
                            }
                        }
                    } else {
                        eprintln!(
                            "[HookManager] no emulation for {:#x}, removing hook (one-shot)",
                            addr
                        );
                        let _ = vmi_events.write_8_va(addr, 0, orig_byte);
                        event_helpers::set_reinject(event, 1);
                    }
                }
            }

            0
        }
    }
}

impl Drop for HookManager {
    fn drop(&mut self) {
        let state = self.state.read().unwrap();
        let vmi = self.vmi.lock().unwrap();

        eprintln!("[HookManager] restoring {} hooks...", state.hooks.len());
        for (_, hook) in state.hooks.iter() {
            if let Err(e) = vmi.write_8_va(hook.addr, 0, hook.orig_byte) {
                eprintln!("[HookManager] restore failed at {:#x}: {}", hook.addr, e);
            }
        }

        if !self.int_event.is_null() {
            unsafe {
                let _ = vmi.clear_event(self.int_event as *mut _);
                let _ = Box::from_raw(self.int_event);
            }
        }
        eprintln!("[HookManager] cleanup complete");
    }
}
