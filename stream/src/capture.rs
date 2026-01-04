//! zero-copy vram capture via mmap /dev/shm
//! collapsed hot path: scrape -> diff -> yuv -> encode in one thread

use aligned_vec::{AVec, ConstAlign};
use anyhow::Result;

use bytes::Bytes;
use nix::sys::uio::{process_vm_readv, RemoteIoVec};
use std::io::IoSliceMut;
use nix::unistd::Pid;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::encoder::Encoder;

/// encoded frame - just nals, no pixel data
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub nals: Vec<Bytes>,
    pub timestamp: Instant,
    pub frame_num: u64,
}

/// mmap-backed vram with collapsed pipeline
pub struct VramPipeline {
    pid: Pid,
    hva: usize,
    buffer: Vec<u8>,
    shadow: AVec<u8, ConstAlign<64>>,
    yuv: AVec<u8, ConstAlign<64>>,
    encoder: Encoder,
    width: u32,
    height: u32,
    tick_count: u64,
}

impl VramPipeline {
    pub fn new(pid: i32, hva: usize, width: u32, height: u32) -> Result<Self> {
        tracing::info!("initializing scraper: pid={}, hva=0x{:x}, res={}x{}", pid, hva, width, height);

        let frame_size = (width as usize) * (height as usize) * 4;
        let yuv_size = (width as usize) * (height as usize) * 3 / 2;

        let buffer = vec![0u8; frame_size];
        
        // Zero-init shadow
        let shadow = AVec::from_iter(64, vec![0u8; frame_size]);
        
        // Initialize YUV to Black (Y=16, U=128, V=128)
        // This prevents the "green flash" on startup if VRAM is empty
        let yuv = AVec::from_iter(64, {
             let mut b = vec![16u8; (width*height) as usize]; // Y plane
             b.extend(std::iter::repeat(128u8).take(yuv_size - b.len())); // UV planes
             b
        });

        let encoder = Encoder::new(width as i32, height as i32)?;

        let mut pipeline = Self {
            pid: Pid::from_raw(pid),
            hva,
            buffer,
            shadow,
            yuv,
            encoder,
            width,
            height,
            tick_count: 0,
        };
        
        // Verify VRAM access immediately
        pipeline.verify_vram()?;
        
        Ok(pipeline)
    }
    
    fn verify_vram(&mut self) -> Result<()> {
         let len = 4096.min(self.buffer.len());
         let mut local_iov = [IoSliceMut::new(&mut self.buffer[..len])];
         let remote_iov = [RemoteIoVec { base: self.hva, len }];
         
         process_vm_readv(self.pid, &mut local_iov, &remote_iov)?;
         
         if self.buffer[..len].iter().all(|&b| b == 0) {
             tracing::warn!("VRAM verification: Region is ALL ZEROS. Possible wrong HVA or VRAM not initialized.");
         } else {
             tracing::info!("VRAM verification: Success (non-zero content found).");
         }
         Ok(())
    }

    /// collapsed hot path: scrape -> diff -> yuv -> encode
    pub fn process_frame(&mut self) -> Option<EncodedFrame> {
        // Increment tick at start (time always moves forward)
        let current_tick = self.tick_count;
        self.tick_count += 1;

        // Scrape
        let len = self.buffer.len();
        {
            let mut local_iov = [IoSliceMut::new(&mut self.buffer)];
            let remote_iov = [RemoteIoVec {
                base: self.hva,
                len,
            }];

            if let Err(e) = process_vm_readv(self.pid, &mut local_iov, &remote_iov) {
                 if current_tick % 60 == 0 {
                     tracing::error!("vm_readv failed: {}", e);
                 }
                 return None;
            }
        } // drop mutable borrow

        let vram = &self.buffer;
        let shadow = self.shadow.as_slice();

        if current_tick % 120 == 0 {
             if let Some(pos) = vram.iter().position(|&b| b != 0) {
                 tracing::debug!("VRAM active. Sample: {:?}", &vram[pos..pos+8.min(vram.len()-pos)]);
             }
        }

        let mut changed = true;
        let header_size = 256.min(vram.len());
        
        if vram[..header_size] == shadow[..header_size] && vram == shadow {
            if current_tick > 0 {
                changed = false;
            }
        }

        // Heartbeat: 1s
        if !changed {
            if current_tick % 60 != 0 {
                return None;
            }
        }

        if changed {
             convert_to_yuv(&mut self.yuv, vram, self.width, self.height);
             self.shadow.as_mut_slice().copy_from_slice(vram);
        }

        let force_idr = current_tick % 60 == 0;
        
        if force_idr || changed {
             tracing::debug!("encoding frame {} (changed={}, IDR={})", current_tick, changed, force_idr);
        }

        let nals = match self.encoder.encode(&self.yuv, force_idr) {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("encode failed: {}", e);
                return None;
            }
        };

        Some(EncodedFrame {
            nals,
            timestamp: Instant::now(),
            frame_num: current_tick,
        })
    }
}

/// BGRA -> YUV420 using yuv crate
#[inline]
fn convert_to_yuv(yuv_buffer: &mut [u8], bgra: &[u8], width: u32, height: u32) {
    use yuv::{
        bgra_to_yuv420, BufferStoreMut, YuvConversionMode, YuvPlanarImageMut, YuvRange,
        YuvStandardMatrix,
    };

    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;
    let uv_size = y_size / 4;

    let (y_plane, uv_planes) = yuv_buffer.split_at_mut(y_size);
    let (u_plane, v_plane) = uv_planes.split_at_mut(uv_size);

    let mut planar = YuvPlanarImageMut {
        y_plane: BufferStoreMut::Borrowed(y_plane),
        y_stride: w as u32,
        u_plane: BufferStoreMut::Borrowed(u_plane),
        u_stride: (w / 2) as u32,
        v_plane: BufferStoreMut::Borrowed(v_plane),
        v_stride: (w / 2) as u32,
        width: w as u32,
        height: h as u32,
    };

    let stride = (w * 4) as u32;
    let _ = bgra_to_yuv420(
        &mut planar,
        bgra,
        stride,
        YuvRange::Limited,
        YuvStandardMatrix::Bt601,
        YuvConversionMode::Fast,
    );
}

/// run pipeline loop writing to frame latch
pub fn run_pipeline_loop<F>(
    pid: i32,
    hva: usize,
    width: u32,
    height: u32,
    latch: Arc<arc_swap::ArcSwapOption<EncodedFrame>>,
    is_shutdown: F,
) -> Result<()> 
where
    F: Fn() -> bool,
{
    let mut pipeline = VramPipeline::new(pid, hva, width, height)?;
    tracing::info!("pipeline loop started");

    let interval = Duration::from_micros(16666); // 60 FPS

    loop {
        let frame_start = Instant::now();

        if is_shutdown() {
            tracing::info!("pipeline: shutdown");
            break;
        }

        if let Some(encoded) = pipeline.process_frame() {
            // Atomic store - instant update, no channel blocking
            latch.store(Some(Arc::new(encoded)));
        }

        // Precise sleep for frame pacing (no random jitter)
        if let Some(sleep_time) = interval.checked_sub(frame_start.elapsed()) {
            spin_sleep::sleep(sleep_time);
        }
    }

    Ok(())
}

pub fn find_vram_hva(pid: i32, domain: &str, size: usize) -> Vec<usize> {
    let mut candidates = Vec::new();
    let maps_path = format!("/proc/{}/maps", pid);
    
    if let Ok(file) = std::fs::File::open(&maps_path) {
        let reader = std::io::BufReader::new(file);
        let specific_name = format!("loonaro-vram-{}", domain);
        
        tracing::info!("scanning maps for region '{}' or size {}", specific_name, size);

        for line in std::io::BufRead::lines(reader) {
            if let Ok(line) = line {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 1 { continue; }
                
                let range_parts: Vec<&str> = parts[0].split('-').collect();
                if range_parts.len() != 2 { continue; }

                if let (Ok(start), Ok(end)) = (usize::from_str_radix(range_parts[0], 16), usize::from_str_radix(range_parts[1], 16)) {
                    let region_size = end - start;
                    
                    // Priority 1: Named match
                    if line.contains(&specific_name) {
                        tracing::info!("found candidate (named): 0x{:x}", start);
                        candidates.push(start);
                        continue;
                    }

                    // Priority 2: Size match (rw-s or rw-p)
                    if region_size == size {
                         if parts.len() > 1 && (parts[1].starts_with("rw-s") || parts[1].starts_with("rw-p")) {
                             tracing::info!("found candidate (size match {}): 0x{:x}", parts[1], start);
                             candidates.push(start);
                         }
                    }
                }
            }
        }
    }
    
    candidates
}

pub fn find_qemu_pid(domain: &str) -> Option<i32> {
    let proc = std::fs::read_dir("/proc").ok()?;
    for entry in proc {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Ok(pid) = path.file_name()?.to_string_lossy().parse::<i32>() {
                // check cmdline
                let cmdline_path = path.join("cmdline");
                if let Ok(cmdline) = std::fs::read_to_string(cmdline_path) {
                    // looking for "qemu-system" and "guest=<domain>,"
                    // cmdline is null-delimited
                    if cmdline.contains("qemu-system") && cmdline.contains(&format!("guest={},", domain)) {
                        return Some(pid);
                    }
                }
            }
        }
    }
    None
}
