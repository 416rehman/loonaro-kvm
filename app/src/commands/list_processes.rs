//! list-processes command implementation

use loonaro_vmi::cli::VmiArgs;
use loonaro_vmi::session::Session;
use loonaro_vmi::vmi::OsType;
use loonaro_vmi::os::windows::actions::list_processes::ListProcesses;

pub fn run(args: &VmiArgs) -> anyhow::Result<()> {
    let json_str = args.json.to_string_lossy();
    let socket_str = args.socket_path.to_string_lossy();
    
    // session owns the vmi handle
    let session = Session::new(&args.name, &json_str, &socket_str)
        .map_err(|e| anyhow::anyhow!("init failed: {}", e))?;

    let os_type = session.vmi().lock().unwrap().os_type();
    println!("OS: {:?}", os_type);
    
    let processes = match os_type {
        OsType::Windows => {
            session.execute(ListProcesses)
                .map_err(|e| anyhow::anyhow!("list failed: {}", e))?
        },
        _ => return Err(anyhow::anyhow!("unsupported OS")),
    };

    println!("\n{:<8} {:<30} {:<18}", "PID", "Name", "Address");
    println!("{:-<8} {:-<30} {:-<18}", "", "", "");
    
    for p in processes {
        println!("{:<8} {:<30} 0x{:016x}", p.pid, p.name, p.addr);
    }
    
    Ok(())
}
