use crate::vmi::Vmi;
use crate::error::Result;
use super::ProcessInfo;

pub struct WindowsOs<'a> {
    vmi: &'a Vmi,
}

impl<'a> WindowsOs<'a> {
    pub fn new(vmi: &'a Vmi) -> Self {
        Self { vmi }
    }

    pub fn list_processes(&self) -> Result<Vec<ProcessInfo>> {
        let tasks_offset = self.vmi.get_offset("win_tasks")?;
        let name_offset = self.vmi.get_offset("win_pname")?;
        let pid_offset = self.vmi.get_offset("win_pid")?;
        
        let list_head = self.vmi.read_addr_ksym("PsActiveProcessHead")?;
        
        let mut processes = Vec::new();
        let mut cur_list_entry = list_head;
        let mut next_list_entry = self.vmi.read_addr_va(cur_list_entry, 0)?;
        
        // limit loop to avoid infinite loops if list is corrupted
        for _ in 0..10000 {
            let current_process = cur_list_entry - tasks_offset;
            
            let pid = self.vmi.read_32_va(current_process + pid_offset, 0).unwrap_or(0) as i32;
            let name = self.vmi.read_str_va(current_process + name_offset, 0).unwrap_or_else(|_| "<unknown>".into());
            
            processes.push(ProcessInfo {
                pid,
                name,
                addr: current_process,
            });
            
            cur_list_entry = next_list_entry;
            next_list_entry = self.vmi.read_addr_va(cur_list_entry, 0)?;
            
            if next_list_entry == list_head {
                break;
            }
        }
        
        Ok(processes)
    }
}
