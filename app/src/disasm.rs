//! instruction decoder for hook emulation
//!
//! when we place an INT3 (0xCC) at the start of a function to hook it,
//! we overwrite the first byte of the original instruction. after our
//! callback runs, we need to "replay" that instruction so execution
//! can continue normally. this module figures out what that instruction
//! was and how to emulate it.
//!
//! we handle these common prolog patterns:
//!   - push reg           (save callee-saved)
//!   - mov [base+disp],reg (save to stack/shadow space)
//!   - mov reg, reg       (e.g. mov rbp, rsp)
//!   - sub rsp, imm       (allocate stack frame)
//!   - lea reg, [base+disp] (frame pointer setup)
//!
//! anything else and the hook becomes one-shot (restore original, bail).

use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, Register, OpKind};

use crate::ffi::{RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8, R9, R10, R11, R12, R13, R14, R15};
use crate::error::{Result, VmiError};

/// guest cpu mode - needed because x86 encoding differs between modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bitness {
    Bits32,
    Bits64,
}

impl Bitness {
    pub fn as_u32(self) -> u32 {
        match self {
            Bitness::Bits32 => 32,
            Bitness::Bits64 => 64,
        }
    }

    /// convert from libvmi's vmi_get_address_width() return value
    pub fn from_address_width(width: u8) -> Self {
        // libvmi returns 4 for 32-bit, 8 for 64-bit (pointer size in bytes)
        if width == 8 { Bitness::Bits64 } else { Bitness::Bits32 }
    }
}

/// describes how to emulate a hooked instruction after callback fires
#[derive(Debug, Clone)]
pub enum EmulationStrategy {
    /// mov [base + disp], src
    /// e.g. `mov [rsp+0x20], rbx` - saving callee-saved reg to shadow space
    MoveToMem {
        src_reg: u64,
        base_reg: u64,
        displacement: i64,
        len: u64,
    },
    /// push reg
    /// e.g. `push rbp` - classic prolog start
    Push {
        src_reg: u64,
        len: u64,
    },
    /// mov dst_reg, src_reg
    /// e.g. `mov rbp, rsp` - frame pointer setup
    MovRegReg {
        dst_reg: u64,
        src_reg: u64,
        len: u64,
    },
    /// sub reg, imm
    /// e.g. `sub rsp, 0x40` - stack allocation
    SubImm {
        reg: u64,
        imm: u64,
        len: u64,
    },
    /// lea dst, [base + disp]
    /// e.g. `lea rbp, [rsp+0x20]` - another frame setup pattern
    Lea {
        dst_reg: u64,
        base_reg: u64,
        displacement: i64,
        len: u64,
    },
}

/// analyze first instruction at addr, returns emulation strategy if we can handle it
pub fn analyze_instruction(code: &[u8], addr: u64, bitness: Bitness) -> Result<Option<EmulationStrategy>> {
    if code.is_empty() {
        return Err(VmiError::Other("empty code buffer".into()));
    }

    let mut decoder = Decoder::with_ip(bitness.as_u32(), code, addr, DecoderOptions::NONE);
    let instr = decoder.decode();
    
    if instr.is_invalid() {
        return Err(VmiError::Other(format!("invalid instruction at {:#x}", addr)));
    }

    let strategy = match instr.mnemonic() {
        Mnemonic::Push => decode_push(&instr),
        Mnemonic::Mov => decode_mov(&instr),
        Mnemonic::Sub => decode_sub_imm(&instr),
        Mnemonic::Lea => decode_lea(&instr),
        _ => None,
    };

    Ok(strategy)
}

/// decode push reg
fn decode_push(instr: &Instruction) -> Option<EmulationStrategy> {
    if instr.op_count() != 1 || instr.op0_kind() != OpKind::Register {
        return None;
    }

    let vmi_reg = iced_reg_to_vmi(instr.op0_register())?;
    
    Some(EmulationStrategy::Push {
        src_reg: vmi_reg,
        len: instr.len() as u64,
    })
}

/// decode mov - handles both reg-to-mem and reg-to-reg
fn decode_mov(instr: &Instruction) -> Option<EmulationStrategy> {
    if instr.op_count() != 2 {
        return None;
    }

    // mov [mem], reg - saving to stack
    if matches!(instr.op0_kind(), OpKind::Memory) && instr.op1_kind() == OpKind::Register {
        // no indexed addressing
        if instr.memory_index() != Register::None {
            return None;
        }
        
        let vmi_src = iced_reg_to_vmi(instr.op1_register())?;
        let vmi_base = iced_reg_to_vmi(instr.memory_base())?;
        
        return Some(EmulationStrategy::MoveToMem {
            src_reg: vmi_src,
            base_reg: vmi_base,
            displacement: instr.memory_displacement64() as i64,
            len: instr.len() as u64,
        });
    }
    
    // mov reg, reg - frame pointer setup like mov rbp, rsp
    if instr.op0_kind() == OpKind::Register && instr.op1_kind() == OpKind::Register {
        let vmi_dst = iced_reg_to_vmi(instr.op0_register())?;
        let vmi_src = iced_reg_to_vmi(instr.op1_register())?;
        
        return Some(EmulationStrategy::MovRegReg {
            dst_reg: vmi_dst,
            src_reg: vmi_src,
            len: instr.len() as u64,
        });
    }

    None
}

/// decode sub reg, imm - stack allocation
fn decode_sub_imm(instr: &Instruction) -> Option<EmulationStrategy> {
    if instr.op_count() != 2 {
        return None;
    }
    
    // first op must be register, second must be immediate
    if instr.op0_kind() != OpKind::Register {
        return None;
    }
    
    let imm = match instr.op1_kind() {
        OpKind::Immediate8 => instr.immediate8() as u64,
        OpKind::Immediate8to32 => instr.immediate8to32() as i32 as u64,
        OpKind::Immediate8to64 => instr.immediate8to64() as u64,
        OpKind::Immediate32 => instr.immediate32() as u64,
        OpKind::Immediate32to64 => instr.immediate32to64() as u64,
        _ => return None,
    };
    
    let vmi_reg = iced_reg_to_vmi(instr.op0_register())?;
    
    Some(EmulationStrategy::SubImm {
        reg: vmi_reg,
        imm,
        len: instr.len() as u64,
    })
}

/// decode lea dst, [base+disp] - frame pointer setup
fn decode_lea(instr: &Instruction) -> Option<EmulationStrategy> {
    if instr.op_count() != 2 {
        return None;
    }
    
    if instr.op0_kind() != OpKind::Register || !matches!(instr.op1_kind(), OpKind::Memory) {
        return None;
    }
    
    // no indexed addressing
    if instr.memory_index() != Register::None {
        return None;
    }
    
    let vmi_dst = iced_reg_to_vmi(instr.op0_register())?;
    let vmi_base = iced_reg_to_vmi(instr.memory_base())?;
    
    Some(EmulationStrategy::Lea {
        dst_reg: vmi_dst,
        base_reg: vmi_base,
        displacement: instr.memory_displacement64() as i64,
        len: instr.len() as u64,
    })
}

/// map iced-x86 register to libvmi register constant
fn iced_reg_to_vmi(reg: Register) -> Option<u64> {
    match reg {
        Register::RAX | Register::EAX => Some(RAX as u64),
        Register::RCX | Register::ECX => Some(RCX as u64),
        Register::RDX | Register::EDX => Some(RDX as u64),
        Register::RBX | Register::EBX => Some(RBX as u64),
        Register::RSP | Register::ESP => Some(RSP as u64),
        Register::RBP | Register::EBP => Some(RBP as u64),
        Register::RSI | Register::ESI => Some(RSI as u64),
        Register::RDI | Register::EDI => Some(RDI as u64),
        Register::R8 | Register::R8D => Some(R8 as u64),
        Register::R9 | Register::R9D => Some(R9 as u64),
        Register::R10 | Register::R10D => Some(R10 as u64),
        Register::R11 | Register::R11D => Some(R11 as u64),
        Register::R12 | Register::R12D => Some(R12 as u64),
        Register::R13 | Register::R13D => Some(R13 as u64),
        Register::R14 | Register::R14D => Some(R14 as u64),
        Register::R15 | Register::R15D => Some(R15 as u64),
        _ => None,
    }
}