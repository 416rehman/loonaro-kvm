//! webrtc streaming using str0m pure-rust stack
//! sans-io pattern: we drive the rtc instance manually

use anyhow::Result;
use bytes::Bytes;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use rayon::prelude::*; // Parallel iterators
use str0m::{Rtc, RtcConfig, Event, Input, Output};
use str0m::media::{MediaKind, Mid};
use str0m::change::SdpOffer;
use str0m::rtp::{SeqNo, ExtensionValues};
use str0m::net::{Protocol, Receive};
use tokio::sync::mpsc;
use crate::state::VmState;

use crate::encoder::Encoder;
use crate::input::InputEvent;

/// webrtc streamer using str0m
pub struct Streamer {
    rtc: Arc<Mutex<Rtc>>,
    socket: Arc<UdpSocket>,
    pub state: Arc<VmState>,
    video_mid: Arc<Mutex<Option<Mid>>>,
    // video_pt: Arc<Mutex<Option<Pt>>>,
    video_seq_no: Arc<Mutex<SeqNo>>,
    input_tx: mpsc::Sender<InputEvent>,
    local_addr: SocketAddr, // resolved local address with real IP
}

fn get_local_ip() -> Result<std::net::IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}

impl Streamer {
    pub fn new(state: Arc<VmState>, input_tx: mpsc::Sender<InputEvent>) -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        
        // resolve local ip or fallback
        let bound_port = socket.local_addr()?.port();
        let local_ip = get_local_ip().unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::new(0,0,0,0)));
        let local_addr = SocketAddr::new(local_ip, bound_port);
        
        tracing::info!("webrtc bound 0.0.0.0:{}, advertising {}", bound_port, local_addr);
        
        let rtc = RtcConfig::new()
            .set_ice_lite(true)
            .build();
        
        let mut mid = None;
        // Optimization: default mid to "0" for single video track case
        // This avoids missing the MediaAdded event race if it occurs
        if true {
             mid = Some(Mid::from("0"));
        }

        Ok(Self {
            rtc: Arc::new(Mutex::new(rtc)),
            socket: Arc::new(socket),
            state,
            video_mid: Arc::new(Mutex::new(mid)),

            video_seq_no: Arc::new(Mutex::new(SeqNo::from(1))), // Init with small ROC
            input_tx,
            local_addr,
        })
    }

    pub fn handle_offer(&self, offer_sdp: &str) -> Result<String> {
        tracing::info!("handling offer sdp");
        let mut rtc = self.rtc.lock().unwrap();
        
        let offer = SdpOffer::from_sdp_string(offer_sdp)?;
        
        // Add candidate BEFORE accepting offer so it's included in the Answer SDP
        tracing::info!("adding candidate: {}", self.local_addr);
        rtc.add_local_candidate(str0m::Candidate::host(self.local_addr, "udp")?);
        
        let answer = rtc.sdp_api().accept_offer(offer)?;
        
        Ok(answer.to_sdp_string())
    }

    fn poll_rtc(&self) {
        let mut rtc = self.rtc.lock().unwrap();
        
        loop {
            match rtc.poll_output() {
                Ok(output) => match output {
                    Output::Transmit(t) => {
                        let len = t.contents.len();
                        if len > 1400 {
                            tracing::warn!("sending large packet: {} bytes (likely MTU violation)", len);
                        } else if len > 0 && rand::random::<u8>() % 100 == 0 {
                             tracing::debug!("sending packet: {} bytes", len);
                        }
                        let _ = self.socket.send_to(&t.contents, t.destination);
                    }
                    Output::Timeout(_) => break,
                    Output::Event(event) => {
                        // tracing::debug!("rtc event: {:?}", event);
                        match event {
                            Event::MediaAdded(media) => {
                                tracing::info!("media added: {:?} kind={:?}", media.mid, media.kind);
                                if media.kind == MediaKind::Video {
                                    *self.video_mid.lock().unwrap() = Some(media.mid);
                                }
                            }
                            Event::ChannelData(cd) => {
                                if let Ok(text) = std::str::from_utf8(&cd.data) {
                                    tracing::info!("dc msg: {}", text);
                                    if let Ok(event) = serde_json::from_str::<InputEvent>(text) {
                                        let _ = self.input_tx.blocking_send(event);
                                    }
                                }
                            }
                            Event::IceConnectionStateChange(state) => {
                                tracing::info!("ice state: {:?}", state);
                            }
                            _ => {}
                        }
                    }
                },
                Err(_) => break,
            }
        }
    }

    fn receive_packets(&self) {
        let mut buf = [0u8; 2000];
        
        while let Ok((n, source)) = self.socket.recv_from(&mut buf) {
            let now = Instant::now();
            let mut rtc = self.rtc.lock().unwrap();
            
            let receive = Receive {
                proto: Protocol::Udp,
                source,
                destination: self.local_addr, 
                contents: (&buf[..n]).try_into().unwrap(),
            };
            
            let _ = rtc.handle_input(Input::Receive(now, receive));
        }
    }

    fn send_video(&self, nals: &[Bytes], wallclock: Instant, media_time: u32) {
        let mid_opt = *self.video_mid.lock().unwrap();
        if let Some(mid) = mid_opt {
             let mut rtc = self.rtc.lock().unwrap();
             let mut seq_no_guard = self.video_seq_no.lock().unwrap();

             // Resolve PT (Payload Type) for video
             // We use rtc.writer to look up the parameters, as it exposes payload_params
             // We MUST find the H264 payload type, otherwise the browser might try to decode as VP8
             let pt = rtc.writer(mid).and_then(|w| {
                 w.payload_params()
                     .find(|p| format!("{:?}", p.spec().codec).to_uppercase().contains("H264"))
                     .map(|p| p.pt())
             });

             if let Some(pt) = pt {
                 let mut direct = rtc.direct_api();
                 if let Some(stream) = direct.stream_tx_by_mid(mid, None) {
                     for (i, nal) in nals.iter().enumerate() {
                         let is_last = i == nals.len() - 1;
                         // Marker bit is ONLY set on the last NAL of the frame
                         let marker = is_last;
                         
                         // Debug logging for NAL types
                         // 5=IDR, 7=SPS, 8=PPS, 1=Slice
                         let nal_type = nal[0] & 0x1f;
                         if nal_type == 7 || nal_type == 8 {
                             tracing::info!("sending NAL type={} len={} marker={}", nal_type, nal.len(), marker);
                         }

                         // Get next sequence number
                         let seq = seq_no_guard.inc();
                         
                         // Write RTP packet directly
                         // Payload must be just the NAL data
                         if let Err(e) = stream.write_rtp(
                             pt,
                             seq,
                             media_time,
                             wallclock,
                             marker,
                             ExtensionValues::default(),
                             true, // nackable
                             nal.to_vec()
                         ) {
                             tracing::error!("rtp write error: {:?}", e);
                         }
                     }
                 }
             }
        }
    }

    pub async fn run_loop(self_arc: Arc<Self>) {
        tokio::spawn(async move {
            // let mut tile_grid = TileGrid::new(1920, 1080);
            let mut encoder = match Encoder::new(1920, 1080) {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("encoder init: {}", e);
                    return;
                }
            };
            
            let mut frames: u64 = 0;
            let frame_duration = tokio::time::Duration::from_micros(16666);
            let mut interval = tokio::time::interval(frame_duration);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            
            tracing::info!("encode loop started");
            
            let mut yuv_buffer = vec![0u8; 1920 * 1080 * 3 / 2];
            let mut frame_count: u64 = 0;

            loop {
                // limit to ~60fps - simplified
                interval.tick().await;
                frame_count += 1;
                
                self_arc.receive_packets();
                self_arc.poll_rtc();
                
                if !self_arc.state.check_and_clear_dirty() {
                    continue;
                }
                
                let maybe_encoded = {
                    let fb = self_arc.state.fb.lock().unwrap();
                    let width = fb.metadata.width as i32;
                    let height = fb.metadata.height as i32;
                    
                    if width != encoder.width || height != encoder.height {
                        tracing::info!("res change: {}x{} -> {}x{}", encoder.width, encoder.height, width, height);
                        
                        match Encoder::new(width, height) {
                            Ok(e) => {
                                encoder = e;
                                // resize buffer
                                let req_size = (width * height * 3 / 2) as usize;
                                if yuv_buffer.len() < req_size {
                                    yuv_buffer.resize(req_size, 0);
                                }
                            },
                            Err(e) => {
                                tracing::error!("encoder init failed: {}", e);
                                continue;
                            }
                        }
                    }

                    let y_size = (width * height) as usize;
                    let uv_size = y_size / 4;
                    let stride = fb.metadata.stride as usize;
                    
                    if !fb.capture_buffer.is_empty() {
                        let src = &fb.capture_buffer;
                        let (y_plane, uv_plane) = yuv_buffer.split_at_mut(y_size);
                        let (u_plane, v_plane) = uv_plane.split_at_mut(uv_size);
                        
                        let y_stride = width as usize;
                        let uv_stride = width as usize / 2;
                        
                        let y_chunks = y_plane.par_chunks_mut(y_stride * 2);
                        let u_chunks = u_plane.par_chunks_mut(uv_stride);
                        let v_chunks = v_plane.par_chunks_mut(uv_stride);
                        let src_chunks = src.par_chunks(stride * 2);

                        y_chunks.zip(u_chunks).zip(v_chunks).zip(src_chunks)
                            .for_each(|(((y_rows, u_row), v_row), src_rows)| {
                                for cx in 0..(y_stride / 2) { 
                                    let x = cx * 2;
                                    let s00 = x * 4;
                                    let s10 = stride + x * 4;
                                    
                                    if s10 + 8 <= src_rows.len() {
                                        let b00 = src_rows[s00] as i32;
                                        let g00 = src_rows[s00+1] as i32;
                                        let r00 = src_rows[s00+2] as i32;
                                        
                                        let b01 = src_rows[s00+4] as i32;
                                        let g01 = src_rows[s00+5] as i32;
                                        let r01 = src_rows[s00+6] as i32;

                                        let b10 = src_rows[s10] as i32;
                                        let g10 = src_rows[s10+1] as i32;
                                        let r10 = src_rows[s10+2] as i32;

                                        let b11 = src_rows[s10+4] as i32;
                                        let g11 = src_rows[s10+5] as i32;
                                        let r11 = src_rows[s10+6] as i32;

                                        let y00 = ((66 * r00 + 129 * g00 + 25 * b00 + 128) >> 8) + 16;
                                        let y01 = ((66 * r01 + 129 * g01 + 25 * b01 + 128) >> 8) + 16;
                                        let y10 = ((66 * r10 + 129 * g10 + 25 * b10 + 128) >> 8) + 16;
                                        let y11 = ((66 * r11 + 129 * g11 + 25 * b11 + 128) >> 8) + 16;
                                        
                                        y_rows[x] = y00 as u8;
                                        y_rows[x+1] = y01 as u8;
                                        y_rows[y_stride + x] = y10 as u8;
                                        y_rows[y_stride + x + 1] = y11 as u8;

                                        let r_avg = (r00 + r01 + r10 + r11) >> 2;
                                        let g_avg = (g00 + g01 + g10 + g11) >> 2;
                                        let b_avg = (b00 + b01 + b10 + b11) >> 2;

                                        let u = ((-38 * r_avg - 74 * g_avg + 112 * b_avg + 128) >> 8) + 128;
                                        let v = ((112 * r_avg - 94 * g_avg - 18 * b_avg + 128) >> 8) + 128;

                                        u_row[cx] = u as u8;
                                        v_row[cx] = v as u8;
                                    }
                                }
                            });
                            
                        encoder.encode(&yuv_buffer, frame_count % 60 == 0).ok()
                    } else {
                        None
                    }
                };
                
                if let Some(nal_units) = maybe_encoded {
                    let wallclock = Instant::now();
                    // RTP time: 90000Hz clock. 
                    let media_time = (frame_count * 90000 / 60) as u32;
                    
                    self_arc.send_video(&nal_units, wallclock, media_time);
                    
                    let count = nal_units.len();
                    tracing::info!("encoder: sent frame {}, nals={}", frame_count, count);
                } else if frames % 60 == 0 {
                    tracing::info!("encoder: frame {}, no change/skipped", frames);
                }
                
                frames = frames.wrapping_add(1);
            }
        });
    }
}
