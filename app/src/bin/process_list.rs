//! process list utility using loonaro-vmi

use clap::Parser;
use loonaro_vmi::vmi::{OsType, Vmi};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Domain name
    #[arg(short, long)]
    name: String,

    /// Path to JSON profile
    #[arg(short, long)]
    json: String,

    /// Path to KVMI socket
    #[arg(short, long, default_value = "/tmp/introspector")]
    socket: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("Initializing LibVMI for domain {}...", args.name);
    println!("  JSON Profile: {}", args.json);
    println!("  KVMI Socket:  {}", args.socket);

    let vmi = Vmi::new(&args.name, &args.json, &args.socket)
        .map_err(|e| anyhow::anyhow!("Failed to init LibVMI: {}", e))?;

    println!("Successfully initialized LibVMI!");
    
    match vmi.os_type() {
        OsType::Windows => println!("Detected OS: Windows"),
        OsType::Linux => println!("Detected OS: Linux"),
        OsType::FreeBSD => println!("Detected OS: FreeBSD"),
        OsType::Osx => println!("Detected OS: OSX"),
        OsType::Unknown => println!("Detected OS: Unknown"),
    }

    println!("Pausing VM for introspection...");
    vmi.pause().map_err(|e| anyhow::anyhow!("Failed to pause VM: {}", e))?;

    // clean up on exit
    // Vmi::drop() will call vmi_resume_vm() automatically, 
    // but we want to ensure we catch any panics before that point if we had logic here
    
    let result = (|| {
        let processes = match vmi.os_type() {
            OsType::Windows => {
                use loonaro_vmi::os::windows::WindowsOs;
                let windows = WindowsOs::new(&vmi);
                windows.list_processes()
            },
            OsType::Linux => {
                 return Err(anyhow::anyhow!("Linux support not implemented yet"));
            },
            _ => return Err(anyhow::anyhow!("Unsupported OS for process listing")),
        };

        let processes = processes.map_err(|e| anyhow::anyhow!("Failed to list processes: {}", e))?;
        
        println!("\nProcess Listing:");
        println!("{:<8} {:<30} {:<18}", "PID", "Name", "Address");
        println!("{:-<8} {:-<30} {:-<18}", "", "", "");
        
        for p in processes {
            println!("{:<8} {:<30} 0x{:016x}", p.pid, p.name, p.addr);
        }
        
        Ok(())
    })();

    println!("Resuming VM...");
    // drop(vmi) happens here automatically, which calls resume 
    
    result
}
