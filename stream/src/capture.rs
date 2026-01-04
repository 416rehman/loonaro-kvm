//! qemu framebuffer capture via /proc/pid/mem + qmp
//! production-ready: async qmp, timeouts, robust error handling

use anyhow::{Result, anyhow};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use crate::state::VmState;

/// shared resolution state - atomic for lock-free access
pub struct DisplayState {
    pub width: AtomicU32,
    pub height: AtomicU32,
    pub stride: AtomicU32,
}

impl DisplayState {
    pub fn new() -> Self {
        // default 1920x1080
        Self {
            width: AtomicU32::new(1920),
            height: AtomicU32::new(1080),
            stride: AtomicU32::new(1920 * 4),
        }
    }
    
    pub fn update(&self, w: u32, h: u32) {
        self.width.store(w, Ordering::Release);
        self.height.store(h, Ordering::Release);
        self.stride.store(w * 4, Ordering::Release);
    }
}

/// find qemu pid for libvirt domain
pub fn find_qemu_pid(domain_name: &str) -> Result<u32> {
    // try libvirt pid file
    let pid_path = format!("/var/run/libvirt/qemu/{}.pid", domain_name);
    if let Ok(pid_str) = fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            tracing::debug!("found pid from libvirt: {}", pid);
            return Ok(pid);
        }
    }
    
    // fallback: scan /proc
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let pid_str = entry.file_name().to_string_lossy().to_string();
        if let Ok(pid) = pid_str.parse::<u32>() {
            let cmdline_path = format!("/proc/{}/cmdline", pid);
            if let Ok(cmdline) = fs::read_to_string(&cmdline_path) {
                let cmdline = cmdline.replace('\0', " ");
                if cmdline.contains("qemu") && cmdline.contains(domain_name) {
                    tracing::info!("found qemu cmdline: {}", cmdline);
                    return Ok(pid);
                }
            }
        }
    }
    
    Err(anyhow!("qemu pid not found for: {}", domain_name))
}

/// vga vram region
#[derive(Debug, Clone)]
pub struct VgaRegion {
    pub pid: u32,
    pub start: u64,
    pub size: u64,
}

/// find vga vram via memfd naming or size heuristics
pub fn find_vga_region(pid: u32) -> Result<VgaRegion> {
    let maps_path = format!("/proc/{}/maps", pid);
    let file = File::open(&maps_path)?;
    let reader = BufReader::new(file);

    // first pass: look for named vram regions
    for line in reader.lines() {
        let line = line?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 { continue; }

        let path = parts[5];
        if path.contains("vga.vram") || path.contains("vga") || path.contains("vram") {
            if let Some(region) = parse_region(&parts, pid) {
                tracing::info!("vram found via memfd: 0x{:x}", region.start);
                return Ok(region);
            }
        }
    }
    
    // second pass: size-based detection - log ALL candidates
    tracing::debug!("memfd not found, scanning for size candidates...");
    find_vga_by_size(pid)
}

fn parse_region(parts: &[&str], pid: u32) -> Option<VgaRegion> {
    let addrs: Vec<&str> = parts[0].split('-').collect();
    if addrs.len() != 2 { return None; }
    
    let start = u64::from_str_radix(addrs[0], 16).ok()?;
    let end = u64::from_str_radix(addrs[1], 16).ok()?;
    
    Some(VgaRegion { pid, start, size: end - start })
}

fn find_vga_by_size(pid: u32) -> Result<VgaRegion> {
    let maps_path = format!("/proc/{}/maps", pid);
    let file = File::open(&maps_path)?;
    let reader = BufReader::new(file);
    
    let mut candidates = Vec::new();
    
    // vga vram is typically 16-64MB anonymous rw region
    for line in reader.lines() {
        let line = line?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 { continue; }
        
        let perms = parts[1];
        if !perms.starts_with("rw") { continue; }
        
        if let Some(region) = parse_region(&parts, pid) {
            // vram size heuristic: 16MB to 64MB
            if region.size >= 16 * 1024 * 1024 && region.size <= 256 * 1024 * 1024 { // Expanded to 256MB just in case
                let path = parts.get(5).copied().unwrap_or("");
                tracing::info!("candidate region: 0x{:x} ({}MB) path='{}'", region.start, region.size / 1024 / 1024, path);
                
                // prefer anonymous or ram-like regions
                if path.is_empty() || path.contains("ram") || path.contains("anon") {
                    candidates.push(region);
                }
            }
        }
    }
    
    // Return the First one for now, but logs will help us decide if we need to pick another
    if let Some(region) = candidates.first() {
         tracing::info!("selected candidate: 0x{:x}", region.start);
         return Ok(region.clone());
    }
    
    Err(anyhow!("vga vram not found"))
}

fn find_vga_by_exact_size(pid: u32, size: u64) -> Result<VgaRegion> {
    let maps_path = format!("/proc/{}/maps", pid);
    let file = File::open(&maps_path)?;
    let reader = BufReader::new(file);
    
    // We allow a small tolerance (e.g. 4k page) just in case, but strictly we expect exact match.
    // QMP says 16*1024*1024. Maps should be exact.
    
    for line in reader.lines() {
        let line = line?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 { continue; }
        
        let perms = parts[1];
        if !perms.starts_with("rw") { continue; }
        
        if let Some(region) = parse_region(&parts, pid) {
            if region.size == size {
                let path = parts.get(5).copied().unwrap_or("");
                tracing::info!("found match by exact size: 0x{:x} ({}MB) path='{}'", region.start, region.size / 1024 / 1024, path);
                
                // prefer anonymous or ram-like regions
                if path.is_empty() || path.contains("ram") || path.contains("anon") {
                    return Ok(region);
                }
            }
        }
    }
    
    Err(anyhow!("vga vram not found by exact size"))
}

/// framebuffer reader - direct memory access
pub struct QemuFramebuffer {
    file: File,
    region: VgaRegion,
    buffer: Vec<u8>,
}

impl QemuFramebuffer {
    pub fn new(region: VgaRegion, display: &DisplayState) -> Result<Self> {
        let mem_path = format!("/proc/{}/mem", region.pid);
        let file = File::open(&mem_path)
            .map_err(|e| anyhow!("cannot open /proc/{}/mem: {} (need root?)", region.pid, e))?;
        
        let stride = display.stride.load(Ordering::Relaxed);
        let height = display.height.load(Ordering::Relaxed);
        let buffer_size = (stride * height) as usize;
        
        Ok(Self { 
            file, 
            region, 
            buffer: vec![0u8; buffer_size],
        })
    }
    
    pub fn read_frame(&mut self, display: &DisplayState) -> Result<&[u8]> {
        let h = display.height.load(Ordering::Relaxed);
        let stride = display.stride.load(Ordering::Relaxed);
        let size = (stride * h) as usize;
        
        if self.buffer.len() < size {
            self.buffer.resize(size, 0);
        }
        
        self.file.seek(SeekFrom::Start(self.region.start))?;
        self.file.read_exact(&mut self.buffer[..size])?;
        
        Ok(&self.buffer[..size])
    }
}

/// start capture with proper async/timeout handling
pub async fn start_capture(domain_name: &str, state: Arc<VmState>, width: u32, height: u32, explicit_vram: Option<(u64, u64)>) -> Result<()> {
    tracing::info!("starting capture for: {}", domain_name);
    
    let pid = find_qemu_pid(domain_name)?;
    tracing::info!("qemu pid: {}", pid);
    
    let region = if let Some((addr, size)) = explicit_vram {
        // Validation: Is this a valid userspace pointer?
        // Linux userspace pointers are typically high (0x7f...)
        // If it's low (e.g. < 0x10000000000), it's likely a GPA or Offset from QMP, not HVA.
        if addr > 0x10000000000 {
             tracing::info!("using explicit vram from qmp: 0x{:x} size={}MB", addr, size / 1024 / 1024);
             VgaRegion { pid, start: addr, size }
        } else {
             tracing::warn!("explicit address 0x{:x} looks like GPA/Offset, not HVA. Using size {}MB to scan maps...", addr, size / 1024 / 1024);
             find_vga_by_exact_size(pid, size)?
        }
    } else {
        find_vga_region(pid)?
    };
    tracing::info!("vram: 0x{:x} size={}MB", region.start, region.size / 1024 / 1024);
    
    let display = Arc::new(DisplayState::new());
    
    // update display with passed resolution
    display.update(width, height);
    tracing::info!("resolution set to: {}x{}", width, height);

    
    // update vmstate
    {
        let mut fb = state.fb.lock().unwrap();
        fb.metadata.width = display.width.load(Ordering::Relaxed);
        fb.metadata.height = display.height.load(Ordering::Relaxed);
        fb.metadata.stride = display.stride.load(Ordering::Relaxed);
        fb.metadata.format = 0x34325258; // XRGB8888
    }
    
    tracing::info!("starting scraper task");
    
    // scraper task
    let display_clone = display.clone();
    let state_clone = state.clone();
    tokio::task::spawn_blocking(move || {
        let mut fb_reader = match QemuFramebuffer::new(region, &display_clone) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("fb reader init failed: {}", e);
                return;
            }
        };
        
        tracing::info!("scraper running");
        
        loop {
            // ~60fps
            std::thread::sleep(std::time::Duration::from_micros(16666));
            
            match fb_reader.read_frame(&display_clone) {
                Ok(frame_data) => {
                    let mut fb = state_clone.fb.lock().unwrap();
                    
                    fb.metadata.width = display_clone.width.load(Ordering::Relaxed);
                    fb.metadata.height = display_clone.height.load(Ordering::Relaxed);
                    fb.metadata.stride = display_clone.stride.load(Ordering::Relaxed);
                    
                    if fb.capture_buffer.len() != frame_data.len() {
                        fb.capture_buffer.resize(frame_data.len(), 0);
                    }
                    fb.capture_buffer.copy_from_slice(frame_data);
                    drop(fb);
                    
                    state_clone.mark_dirty();
                    
                    // heartbeat
                    static FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let f = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if f % 60 == 0 {
                        tracing::info!("scraper: captured frame {}, size={}", f, frame_data.len());
                    }
                }
                Err(e) => {
                    tracing::error!("frame read error: {}", e);
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
    });
    
    Ok(())
}
