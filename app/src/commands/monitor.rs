//! monitor command implementation

use loonaro_vmi::cli::VmiArgs;
use loonaro_vmi::os::windows::events::process_create::ProcessCreateMonitor;
use loonaro_vmi::session::Session;
use loonaro_vmi::vmi::OsType;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn run(args: &VmiArgs) -> anyhow::Result<()> {
    let json_str = args.json.to_string_lossy();
    let socket_str = args.socket_path.to_string_lossy();

    eprintln!("Init monitor for {}", args.name);

    let mut session = Session::new(&args.name, &json_str, &socket_str)
        .map_err(|e| anyhow::anyhow!("init failed: {}", e))?;

    if session.vmi().lock().unwrap().os_type() != OsType::Windows {
        anyhow::bail!("only Windows supported");
    }

    eprintln!("Enabling Process Monitor...");
    session
        .add_event(ProcessCreateMonitor::new())
        .map_err(|e| anyhow::anyhow!("enable failed: {}", e))?;

    eprintln!("Monitor running. Press Ctrl+C to stop.");

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // handle SIGINT for graceful cleanup (restores hooks to avoid BSOD)
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
        eprintln!("\nExiting...");
    })?;

    session.run(running)?;

    Ok(())
}
