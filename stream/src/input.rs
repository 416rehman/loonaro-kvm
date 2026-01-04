use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum InputEvent {
    MouseMove { x: i32, y: i32 },
    MouseButton { button: u32, pressed: bool },
    Keyboard { key: u32, pressed: bool },
}

pub struct QmpClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl QmpClient {
    pub async fn connect(path: &str) -> Result<Self> {
        tracing::info!("connecting to qmp: {}", path);
        let stream = UnixStream::connect(path).await?;
        let (read_half, mut writer) = stream.into_split();
        let mut reader = BufReader::new(read_half);

        let mut line = String::new();
        reader.read_line(&mut line).await?;
        tracing::debug!("qmp greeting: {}", line.trim());

        let cmd = r#"{"execute":"qmp_capabilities"}"#;
        writer.write_all(cmd.as_bytes()).await?;
        writer.write_all(b"\n").await?;

        line.clear();
        reader.read_line(&mut line).await?;
        tracing::debug!("qmp caps response: {}", line.trim());

        let cmd = r#"{"execute":"human-monitor-command","arguments":{"command-line":"info ramblock"}}"#;
        writer.write_all(cmd.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        
        line.clear();
        reader.read_line(&mut line).await?;
        tracing::info!("QMP RAMBLOCKS: {}", line.trim());

        Ok(Self { reader, writer })
    }

    async fn execute_command(&mut self, cmd: &str) -> Result<String> {
        self.writer.write_all(cmd.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;

        let mut line = String::new();
        loop {
            line.clear();
            self.reader.read_line(&mut line).await?;

            if line.contains(r#""return""#) || line.contains(r#""error""#) {
                return Ok(line);
            }
            if line.contains(r#""event""#) {
                tracing::trace!("qmp event (skipped): {}", line.trim());
            }
        }
    }

    pub async fn send_input_event(&mut self, event: InputEvent) -> Result<()> {
        let cmd = match event {
            InputEvent::MouseMove { x, y } => {
                let events = serde_json::json!([
                    {"type": "rel", "data": {"axis": "x", "value": x}},
                    {"type": "rel", "data": {"axis": "y", "value": y}}
                ]);
                serde_json::json!({
                    "execute": "input-send-event",
                    "arguments": {"events": events}
                })
            }
            InputEvent::MouseButton { button, pressed } => {
                let btn_name = match button {
                    0 => "left",
                    1 => "middle", 
                    2 => "right",
                    _ => "left",
                };
                let events = serde_json::json!([
                    {"type": "btn", "data": {"button": btn_name, "down": pressed}}
                ]);
                serde_json::json!({
                    "execute": "input-send-event",
                    "arguments": {"events": events}
                })
            }
            InputEvent::Keyboard { key, pressed } => {
                if let Some(scancodes) = crate::scancodes::map_key(key) {
                    let seq = crate::scancodes::make_break(&scancodes, pressed);
                    let events: Vec<serde_json::Value> = seq
                        .iter()
                        .map(|&byte| {
                            serde_json::json!({
                                "type": "key",
                                "data": {
                                    "down": true,
                                    "key": {"type": "number", "data": byte as i32}
                                }
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "execute": "input-send-event", 
                        "arguments": {"events": events}
                    })
                } else {
                    tracing::warn!("unmapped key: {}", key);
                    return Ok(());
                }
            }
        };

        let json = serde_json::to_string(&cmd)?;
        tracing::info!("qmp input: {}", json);
        
        let response = self.execute_command(&json).await?;
        if response.contains(r#""error""#) {
            tracing::error!("qmp error: {}", response.trim());
        }
        
        Ok(())
    }


    pub async fn query_vram_size(&mut self) -> Result<usize> {
        let cmd = r#"{"execute":"human-monitor-command","arguments":{"command-line":"info ramblock"}}"#;
        let response = self.execute_command(&cmd).await?;
        
        let json: serde_json::Value = serde_json::from_str(&response)?;
        let output = json.get("return").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("bad qmp response"))?;

        for line in output.lines() {
            if line.contains("vga.vram") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                     let size_str = parts[4].trim_start_matches("0x");
                     let size = usize::from_str_radix(size_str, 16).map_err(|_| anyhow::anyhow!("failed to parse size from '{}'", parts[4]))?;
                     
                     tracing::info!("found vga.vram size: {} bytes", size);
                     return Ok(size);
                }
            }
        }
        Err(anyhow::anyhow!("vga.vram block not found in info ramblock"))
    }


    pub async fn detect_resolution(&mut self) -> Result<(u32, u32)> {
        let dump_path = format!("/tmp/qemu_dump_{}.ppm", uuid::Uuid::new_v4());
        let cmd_str = format!(
            r#"{{"execute":"screendump","arguments":{{"filename":"{}"}}}}"#,
            dump_path
        );

        tracing::debug!("probing resolution via {}", dump_path);

        let _ = self.execute_command(&cmd_str).await;

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let content = fs::read(&dump_path)
            .map_err(|e| anyhow::anyhow!("failed to read screendump {}: {}", dump_path, e))?;

        let _ = fs::remove_file(&dump_path);

        let header = String::from_utf8_lossy(&content[..std::cmp::min(100, content.len())]);
        let mut parts = header.split_whitespace();

        if parts.next() == Some("P6") {
            if let (Some(w), Some(h)) = (parts.next(), parts.next()) {
                let width = w.parse::<u32>().unwrap_or(1920);
                let height = h.parse::<u32>().unwrap_or(1080);
                tracing::info!("detected resolution: {}x{}", width, height);
                return Ok((width, height));
            }
        }

        Ok((1920, 1080))
    }


}



pub struct InputHandler {
    qmp: tokio::sync::Mutex<QmpClient>,
}

impl InputHandler {
    pub async fn new(qmp_path: &str) -> Result<Self> {
        let qmp = QmpClient::connect(qmp_path).await?;
        Ok(Self {
            qmp: tokio::sync::Mutex::new(qmp),
        })
    }

    pub async fn handle_event(&self, event: InputEvent) -> Result<()> {
        tracing::info!("handling input: {:?}", event);
        
        let mut qmp = self.qmp.lock().await;
        qmp.send_input_event(event).await
    }

    pub async fn detect_resolution(&self) -> Result<(u32, u32)> {
        let mut qmp = self.qmp.lock().await;
        qmp.detect_resolution().await
    }

    pub async fn query_vram_size(&self) -> Result<usize> {
        let mut qmp = self.qmp.lock().await;
        qmp.query_vram_size().await
    }


}
