use anyhow::Result;
use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum InputEvent {
    MouseMove { x: i32, y: i32 },
    MouseButton { button: u32, pressed: bool },
    Keyboard { key: u32, pressed: bool },
}

// qmp command types for type-safe serialization
#[derive(Serialize)]
struct QmpCommand<T: Serialize> {
    execute: &'static str,
    arguments: T,
}

#[derive(Serialize)]
struct InputSendEventArgs {
    events: Vec<QmpEvent>,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "data")]
enum QmpEvent {
    #[serde(rename = "key")]
    Key(KeyEvent),
    #[serde(rename = "rel")]
    Rel(RelEvent),
    #[serde(rename = "btn")]
    Btn(BtnEvent),
}

#[derive(Serialize)]
struct KeyEvent {
    down: bool,
    key: KeyValue,
}

#[derive(Serialize)]
struct KeyValue {
    #[serde(rename = "type")]
    key_type: &'static str,
    data: u8,
}

#[derive(Serialize)]
struct RelEvent {
    axis: &'static str,
    value: i32,
}

#[derive(Serialize)]
struct BtnEvent {
    button: &'static str,
    down: bool,
}

#[derive(Serialize)]
struct HumanMonitorArgs {
    #[serde(rename = "command-line")]
    command_line: &'static str,
}

pub struct QmpClient {
    stream: UnixStream,
}

impl QmpClient {
    pub async fn connect(path: &str) -> Result<Self> {
        tracing::info!("connecting to qmp: {}", path);
        let mut stream = UnixStream::connect(path).await?;
        
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await?;
        let greeting = String::from_utf8_lossy(&buf[0..n]);
        tracing::debug!("qmp greeting: {}", greeting);
        
        let cmd = r#"{"execute":"qmp_capabilities"}"#;
        stream.write_all(cmd.as_bytes()).await?;
        let n = stream.read(&mut buf).await?;
        tracing::debug!("qmp caps response: {}", String::from_utf8_lossy(&buf[0..n]));
        
        Ok(Self { stream })
    }

    async fn send_command<T: Serialize>(&mut self, cmd: &QmpCommand<T>) -> Result<()> {
        let json = serde_json::to_string(cmd)?;
        tracing::debug!("qmp: {}", json);
        self.stream.write_all(json.as_bytes()).await?;
        
        // Read response to prevent buffer buildup/ensure command processed
        // We expect {"return": {}} or error
        let mut buf = vec![0u8; 4096];
        // Short timeout for response
        let _ = tokio::time::timeout(tokio::time::Duration::from_secs(1), self.stream.read(&mut buf)).await;
        
        Ok(())
    }

    pub async fn query_framebuffer_address(&mut self) -> Result<(u64, u64)> {
        // info mtree gives GPA (Guest Physical), but we need HVA (Host Virtual) for /proc/mem
        // info ramblock gives us the HVA mapping!
        
        let cmd = QmpCommand {
            execute: "human-monitor-command",
            arguments: HumanMonitorArgs {
                command_line: "info ramblock",
            },
        };
        
        // Manual send to control reading
        let json_cmd = serde_json::to_string(&cmd)?;
        tracing::debug!("qmp: {}", json_cmd);
        self.stream.write_all(json_cmd.as_bytes()).await?;
        
        // Read response
        // We need to read enough to get the full JSON.
        // QMP responses are newline terminated.
        let mut buf = vec![0u8; 1024 * 1024]; // 1MB buffer to be safe
        let n = self.stream.read(&mut buf).await?;
        
        // We might need to handle partial reads if the output is huge, 
        // but for now let's hope 1MB catches it all in one go or sufficiently enough.
        
        // The response is a JSON object: {"return": "escaped string\n..."}
        // We need to decode likely multiple JSON objects if there were events, 
        // but typically the command response is the last one.
        // Let's try to parse the whole buffer as raw string first to debug
        // tracing::debug!("ramblock raw: {}", String::from_utf8_lossy(&buf[0..n]));

        // Attempt to parse as serde_json::Value
        // We search for the object containing "return"
        let response_str = String::from_utf8_lossy(&buf[0..n]);
        
        let mut ramblock_output = String::new();
        
        // Naive JSON splitter (since we might get events before the response)
        // We look for the line containing "return"
        for line in response_str.lines() {
             if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                 if let Some(ret) = val.get("return") {
                     if let Some(s) = ret.as_str() {
                         ramblock_output = s.to_string();
                         break;
                     }
                 }
             }
        }
        
        if ramblock_output.is_empty() {
             tracing::warn!("ramblock output empty or not found in: {}", response_str);
             return Err(anyhow::anyhow!("empty response for info ramblock"));
        }

        // Parse info ramblock output matches:
        // Block Name    P Size      Offset   Address
        // ...
        // 0000:00:02.0/vga.vram  16 MiB   0x0      0x7f... (HVA)
        
        for line in ramblock_output.lines() {
            if line.contains("vga.vram") || (line.contains("vga") && line.contains("ram")) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                
                // Heuristic Parsing:
                // Collect all hex values starting with 0x
                let mut hex_vals = Vec::new();
                for p in &parts {
                    if p.starts_with("0x") {
                         if let Ok(val) = u64::from_str_radix(p.trim_start_matches("0x"), 16) {
                             hex_vals.push(val);
                         }
                    }
                }
                
                // If we found hex values, try to deduce Address vs Size
                if !hex_vals.is_empty() {
                    // Check for common VRAM sizes (16MB, 32MB, 64MB)
                    let vram_sizes = [16*1024*1024, 32*1024*1024, 64*1024*1024];
                    let mut size = 0;
                    let mut addr = 0;
                    
                    // Look for size first
                    if let Some(&s) = hex_vals.iter().find(|&&v| vram_sizes.contains(&v)) {
                        size = s;
                    } 
                    // Fallback: literal "0x1000000" (16MB) appears often
                    
                    // The Address should be the specific HVA
                    // In the user's case: 0x0000000200580000 (8.5GB)
                    // HVA is likely the unique large number that isn't the size
                    // Or it's simply the FIRST hex number if the format is HVA SIZE ...
                    
                    // User output: 0x0000000200580000 0x0000000001000000 0x0000000001000000
                    // First one is HVA? 
                    // Let's assume the largest value that ISN'T a power-of-2 size is the address? 
                    // No, address can be anything.
                    
                    // Let's assume standard QEMU ramblock order often puts HVA first or last.
                    // But in the user output, the 16MB (size) comes AFTER the 0x200...
                    
                    // Strategy:
                    // 1. If we found a size > 0, pick a DIFFERENT val as address.
                    // 2. If present, pick the FIRST hex val as address?
                    
                    if size > 0 {
                         // Find a value != size (or if all equals, well...)
                         if let Some(&a) = hex_vals.iter().find(|&&v| v != size) {
                             addr = a;
                         }
                    } else {
                        // try to parse "N MiB" logic again if hex matching failed
                        // ... (omitted for brevity, relying on hex log)
                    }
                    
                    // If we found both (or at least HVA and we default size)
                    if addr > 0 {
                        if size == 0 { size = 16 * 1024 * 1024; } // fallback default
                        tracing::info!("heuristic HVA: 0x{:x} Size: {}MB", addr, size/1024/1024);
                        return Ok((addr, size));
                    }
                }
                
                // Fallback to strict parser if heuristic failed (or to aid debug)
                // (Previous logic removed as it was buggy for this output)
            }
        }
        
        tracing::warn!("vga ramblock not found in parser output:\n{}", ramblock_output);
        Err(anyhow::anyhow!("could not find vga ramblock in info ramblock output"))
    }

    pub async fn detect_resolution(&mut self) -> Result<(u32, u32)> {
        use std::fs;
        use tokio::time::Duration;
        
        // Use screendump to detect resolution (works on old QEMU)
        let dump_path = format!("/tmp/qemu_dump_{}.ppm", uuid::Uuid::new_v4());
        // manual raw command construction to avoid struct definition overhead for one-off
        let cmd_str = format!("{{\"execute\":\"screendump\",\"arguments\":{{\"filename\":\"{}\"}}}}", dump_path);
        
        tracing::debug!("probing resolution via {}", dump_path);

        self.stream.write_all(cmd_str.as_bytes()).await?;

        // Read QMP response
        let mut buf = vec![0u8; 4096];
        let _ = tokio::time::timeout(Duration::from_secs(1), self.stream.read(&mut buf)).await;
        
        // Brief delay to ensure file write completes
        tokio::time::sleep(Duration::from_millis(50)).await;

        let content = fs::read(&dump_path)
            .map_err(|e| anyhow::anyhow!("failed to read screendump {}: {}", dump_path, e))?;
            
        // Clean up
        let _ = fs::remove_file(&dump_path);

        // Parse PPM header: P6\nWIDTH HEIGHT\nMAXVAL\nDATA
        let header = String::from_utf8_lossy(&content[..std::cmp::min(100, content.len())]);
        let mut parts = header.split_whitespace();
        
        if parts.next() == Some("P6") {
            let w_str = parts.next();
            let h_str = parts.next();
            
            if let (Some(w), Some(h)) = (w_str, h_str) {
                 let width = w.parse::<u32>().unwrap_or(1920);
                 let height = h.parse::<u32>().unwrap_or(1080);
                 tracing::info!("detected resolution: {}x{}", width, height);
                 return Ok((width, height));
            }
        }
        
        Ok((1920, 1080))
    }

    pub async fn send_input_event(&mut self, event: InputEvent) -> Result<()> {
        use crate::scancodes::{map_key, make_break};
        
        match event {
            InputEvent::Keyboard { key, pressed } => {
                if let Some(scancodes) = map_key(key) {
                    let seq = make_break(&scancodes, pressed);
                    
                    let events: Vec<QmpEvent> = seq.iter().map(|&byte| {
                        QmpEvent::Key(KeyEvent {
                            down: true,
                            key: KeyValue { key_type: "number", data: byte },
                        })
                    }).collect();
                    
                    let cmd = QmpCommand {
                        execute: "input-send-event",
                        arguments: InputSendEventArgs { events },
                    };
                    self.send_command(&cmd).await?;
                }
            }
            InputEvent::MouseMove { x, y } => {
                let events = vec![
                    QmpEvent::Rel(RelEvent { axis: "x", value: x }),
                    QmpEvent::Rel(RelEvent { axis: "y", value: y }),
                ];
                let cmd = QmpCommand {
                    execute: "input-send-event",
                    arguments: InputSendEventArgs { events },
                };
                self.send_command(&cmd).await?;
            }
            InputEvent::MouseButton { button, pressed } => {
                let btn_name = match button {
                    0 => "left",
                    1 => "middle",
                    2 => "right",
                    _ => "left",
                };
                let events = vec![QmpEvent::Btn(BtnEvent { button: btn_name, down: pressed })];
                let cmd = QmpCommand {
                    execute: "input-send-event",
                    arguments: InputSendEventArgs { events },
                };
                self.send_command(&cmd).await?;
            }
        }
        Ok(())
    }
}

pub struct InputHandler {
    qmp: tokio::sync::Mutex<QmpClient>,
}

impl InputHandler {
    pub async fn new(qmp_path: &str) -> Result<Self> {
        let qmp = QmpClient::connect(qmp_path).await?;
        Ok(Self { qmp: tokio::sync::Mutex::new(qmp) })
    }

    pub async fn handle_event(&self, event: InputEvent) -> Result<()> {
        // stealth jitter: 0-3ms random delay
        let jitter = rand::random::<u64>() % 4;
        if jitter > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(jitter)).await;
        }
        
        let mut qmp = self.qmp.lock().await;
        qmp.send_input_event(event).await
    }

    pub async fn detect_resolution(&self) -> Result<(u32, u32)> {
        let mut qmp = self.qmp.lock().await;
        qmp.detect_resolution().await
    }

    pub async fn query_framebuffer_address(&self) -> Result<(u64, u64)> {
        let mut qmp = self.qmp.lock().await;
        qmp.query_framebuffer_address().await
    }
}
