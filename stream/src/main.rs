mod capture;
mod encoder;
mod input;
mod scancodes;
mod signaling;
mod webrtc;

use crate::capture::find_qemu_pid;

use clap::Parser;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about = "Loonaro Stream - WebRTC VM Streaming")]
struct Args {
    /// VM domain name to capture
    #[arg(long, default_value = "dev")]
    domain: String,

    /// WebRTC signaling server port
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// QMP socket path
    #[arg(long, default_value = "/tmp/qemu-monitor.sock")]
    qmp_socket: String,
}

/// global shutdown flag with acquire/release ordering
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn is_shutdown() -> bool {
    SHUTDOWN.load(Ordering::Acquire)
}

pub fn signal_shutdown() {
    SHUTDOWN.store(true, Ordering::Release);
}

// Single-thread tokio runtime for predictable latency
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();
    info!("loonaro-stream starting for domain: {}", args.domain);

    unsafe {
        let param = libc::sched_param { sched_priority: 80 };
        if libc::sched_setscheduler(0, libc::SCHED_RR, &param) != 0 {
            tracing::warn!("SCHED_RR 80 failed (run as root for realtime perf)");
        } else {
            tracing::info!("SCHED_RR priority 80 set");
        }

        if libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) != 0 {
            tracing::warn!("mlockall failed");
        }

        if libc::prctl(libc::PR_SET_TIMERSLACK, 1) != 0 {
            tracing::warn!("PR_SET_TIMERSLACK failed");
        }
    }

    let input_handler = Arc::new(input::InputHandler::new(&args.qmp_socket).await?);
    let (width, height) = match input_handler.detect_resolution().await {
        Ok(res) => res,
        Err(e) => {
            tracing::warn!("resolution detection failed (using 1920x1080): {}", e);
            (1920, 1080)
        }
    };

    let frame_latch = Arc::new(arc_swap::ArcSwapOption::empty());
    
    let (input_tx, input_rx): (Sender<input::InputEvent>, Receiver<input::InputEvent>) =
        bounded(512);

    let (qmp_tx, mut qmp_rx) = tokio::sync::mpsc::channel(128);
    
    let input_handler_clone = input_handler.clone();
    tokio::spawn(async move {
        while let Some(event) = qmp_rx.recv().await {
            if let Err(e) = input_handler_clone.handle_event(event).await {
                tracing::error!("input error: {}", e);
            }
        }
    });

    let input_thread = std::thread::spawn(move || {
        input_loop_rt(input_rx, qmp_tx);
    });

    let domain = args.domain.clone();
    let latch_writer = frame_latch.clone();
    
    let pid = find_qemu_pid(&domain).ok_or_else(|| anyhow::anyhow!("QEMU process not found for domain {}", domain))?;
    
    let vram_size = input_handler.query_vram_size().await?;
    let candidates = capture::find_vram_hva(pid, &domain, vram_size);
    if candidates.is_empty() {
        anyhow::bail!("No VRAM candidates found for size {} bytes", vram_size);
    }
    
    let mut selected_hva = 0;
    for hva in candidates {
        if let Ok(_) = capture::VramPipeline::new(pid, hva, width, height) { 
             tracing::info!("candidate 0x{:x} passed verification", hva);
             selected_hva = hva;
             break;
        } else {
             tracing::warn!("candidate 0x{:x} failed verification (zeroes or fault)", hva);
        }
    }
    
    if selected_hva == 0 {
        tracing::error!("All candidates failed verification (all zeros?). Defaulting to first candidate.");
        selected_hva = capture::find_vram_hva(pid, &domain, vram_size)[0];
    }
    
    tracing::info!("vram discovered: size={} bytes, hva=0x{:x}", vram_size, selected_hva);

    let capture_thread = std::thread::spawn(move || {
        unsafe {
            let param = libc::sched_param { sched_priority: 80 };
            libc::sched_setscheduler(0, libc::SCHED_RR, &param);
        }

        let shutdown_check = || crate::is_shutdown();
        
        if let Err(e) = capture::run_pipeline_loop(pid, selected_hva, width, height, latch_writer, shutdown_check) {
            tracing::error!("pipeline error: {}", e);
        }
    });

    let streamer = Arc::new(webrtc::Streamer::new(frame_latch, input_tx)?);
    
    let streamer_loop = streamer.clone();
    tokio::spawn(async move {
        webrtc::Streamer::run_loop(streamer_loop).await;
    });

    let signaling = signaling::SignalingServer::new(args.port);
    let streamer_clone = streamer.clone();

    tokio::select! {
        _ = signaling.run(streamer_clone) => {
            info!("signaling stopped, shutting down");
            signal_shutdown();
        }
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, shutting down");
            signal_shutdown();
        }
    }

    info!("waiting for threads to exit...");
    let _ = input_thread.join();
    let _ = capture_thread.join();
    info!("shutdown complete");

    Ok(())
}

// RT-safe input loop
fn input_loop_rt(
    rx: Receiver<input::InputEvent>, 
    qmp_tx: tokio::sync::mpsc::Sender<input::InputEvent>,
) {
    let mut pending_mouse: Option<(i32, i32)> = None;
    let mut last_send = Instant::now();
    let throttle = Duration::from_millis(8);
    let timeout = Duration::from_millis(1);

    loop {
        if is_shutdown() {
            break;
        }

        let maybe_event = rx.recv_timeout(timeout);

        match maybe_event {
            Ok(input::InputEvent::MouseMove { x, y }) => {
                if let Some((px, py)) = pending_mouse {
                    pending_mouse = Some((px + x, py + y));
                } else {
                    pending_mouse = Some((x, y));
                }
            }
            Ok(event) => {
                if let Some((mx, my)) = pending_mouse.take() {
                    if let Err(_) = qmp_tx.blocking_send(input::InputEvent::MouseMove { x: mx, y: my }) {
                        break; 
                    }
                }
                
                if let Err(_) = qmp_tx.blocking_send(event) {
                    break;
                }
                last_send = Instant::now();
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        if let Some((mx, my)) = pending_mouse {
            if last_send.elapsed() >= throttle {
                if let Err(_) = qmp_tx.blocking_send(input::InputEvent::MouseMove { x: mx, y: my }) {
                    break; 
                }
                pending_mouse = None;
                last_send = Instant::now();
            }
        }
    }
}
