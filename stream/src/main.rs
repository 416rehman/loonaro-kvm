mod capture;
mod webrtc;
mod input;
mod state;
mod encoder;
mod scancodes;
mod signaling;


use clap::Parser;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into())
        )
        .init();
    
    let args = Args::parse();

    info!("loonaro-stream starting for domain: {}", args.domain);
    
    // try realtime priority
    unsafe {
        let param = libc::sched_param { sched_priority: 1 };
        if libc::sched_setscheduler(0, libc::SCHED_RR, &param) != 0 {
            tracing::warn!("failed to set SCHED_RR, run as root?");
        } else {
            tracing::info!("SCHED_RR priority set");
        }
    }
    
    let state = std::sync::Arc::new(state::VmState::new());
    
    // qmp input handler
    let input_handler = match input::InputHandler::new(&args.qmp_socket).await {
        Ok(h) => std::sync::Arc::new(h),
        Err(e) => {
            tracing::warn!("qmp connect failed: {}", e);
            return Err(e);
        }
    };
    
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(100);

    // input processing task
    let input_handler_clone = input_handler.clone();
    tokio::spawn(async move {
        while let Some(event) = input_rx.recv().await {
            if let Err(e) = input_handler_clone.handle_event(event).await {
                tracing::error!("input error: {}", e);
            }
        }
    });

    // webrtc streamer
    let streamer = match webrtc::Streamer::new(state.clone(), input_tx) {
        Ok(s) => std::sync::Arc::new(s),
        Err(e) => {
            tracing::error!("streamer init failed: {}", e);
            return Err(e);
        }
    };
    // start capture from qemu process
    let (width, height) = match input_handler.detect_resolution().await {
        Ok(res) => res,
        Err(e) => {
            tracing::warn!("resolution detection failed (using 1920x1080): {}", e);
            (1920, 1080)
        }
    };
    
    // try to get explicit framebuffer address from QMP
    let explicit_vram = match input_handler.query_framebuffer_address().await {
        Ok(info) => Some(info),
        Err(e) => {
            tracing::warn!("failed to query framebuffer address via QMP: {}", e);
            None
        }
    };
    
    capture::start_capture(&args.domain, state.clone(), width, height, explicit_vram).await?;
    
    // start encoding/streaming loop
    webrtc::Streamer::run_loop(streamer.clone()).await;

    // signaling server
    let signaling = signaling::SignalingServer::new(args.port);
    let streamer_clone = streamer.clone();
    tokio::spawn(async move {
        signaling.run(streamer_clone).await;
    });
    
    info!("streaming on port {}", args.port);
    
    tokio::signal::ctrl_c().await?;
    info!("shutting down");

    Ok(())
}
