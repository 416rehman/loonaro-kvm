//! webrtc streaming using str0m
//! receives pre-encoded NALs, no encoding here

use anyhow::Result;

use bytes::Bytes;
use crossbeam_channel::Sender;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use str0m::change::SdpOffer;
use str0m::media::{MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{ExtensionValues, SeqNo};
use str0m::{Event, Input, Output, Rtc, RtcConfig};

use crate::capture::EncodedFrame;
use crate::input::InputEvent;

const MTU: usize = 1200;

pub struct Streamer {
    rtc: Arc<Mutex<Rtc>>,
    socket: Arc<UdpSocket>,
    frame_latch: Arc<arc_swap::ArcSwapOption<EncodedFrame>>,
    video_mid: Arc<Mutex<Option<Mid>>>,
    video_seq_no: Arc<Mutex<SeqNo>>,
    input_tx: Sender<InputEvent>,
    local_addr: SocketAddr,
}

fn get_local_ip() -> Result<std::net::IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}

impl Streamer {
    pub fn new(
        frame_latch: Arc<arc_swap::ArcSwapOption<EncodedFrame>>,
        input_tx: Sender<InputEvent>,
    ) -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;

        let bound_port = socket.local_addr()?.port();
        let local_ip =
            get_local_ip().unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)));
        let local_addr = SocketAddr::new(local_ip, bound_port);

        tracing::info!("webrtc bound 0.0.0.0:{}, advertising {}", bound_port, local_addr);

        let rtc = RtcConfig::new().set_ice_lite(true).build();
        let mid = Some(Mid::from("0"));

        Ok(Self {
            rtc: Arc::new(Mutex::new(rtc)),
            socket: Arc::new(socket),
            frame_latch,
            video_mid: Arc::new(Mutex::new(mid)),
            video_seq_no: Arc::new(Mutex::new(SeqNo::from(1))),
            input_tx,
            local_addr,
        })
    }

    pub fn handle_offer(&self, offer_sdp: &str) -> Result<String> {
        tracing::info!("handling offer sdp");
        let mut rtc = self.rtc.lock().unwrap();

        let offer = SdpOffer::from_sdp_string(offer_sdp)?;
        rtc.add_local_candidate(str0m::Candidate::host(self.local_addr, "udp")?);
        let answer = rtc.sdp_api().accept_offer(offer)?;

        Ok(answer.to_sdp_string())
    }

    fn poll_rtc(&self) {
        let mut rtc = match self.rtc.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        loop {
            match rtc.poll_output() {
                Ok(output) => match output {
                    Output::Transmit(t) => {
                        let _ = self.socket.send_to(&t.contents, t.destination);
                    }
                    Output::Timeout(_) => break,
                    Output::Event(event) => match event {
                        Event::MediaAdded(media) => {
                            tracing::info!("media added: {:?} kind={:?}", media.mid, media.kind);
                            if media.kind == MediaKind::Video {
                                if let Ok(mut mid) = self.video_mid.lock() {
                                    *mid = Some(media.mid);
                                }
                            }
                        }
                        Event::ChannelOpen(ch, label) => {
                            tracing::info!("datachannel opened: id={:?} label={}", ch, label);
                        }
                        Event::ChannelData(cd) => {
                            if let Ok(text) = std::str::from_utf8(&cd.data) {
                                if text.contains("\"type\":\"stats\"") {
                                    continue;
                                }
                                if let Ok(event) = serde_json::from_str::<InputEvent>(text) {
                                    let _ = self.input_tx.try_send(event);
                                }
                            }
                        }
                        Event::IceConnectionStateChange(state) => {
                            tracing::info!("ice state: {:?}", state);
                        }
                        _ => {}
                    },
                },
                Err(_) => break,
            }
        }
    }

    fn receive_packets(&self) {
        let mut buf = [0u8; 2000];

        while let Ok((n, source)) = self.socket.recv_from(&mut buf) {
            let now = Instant::now();
            let mut rtc = match self.rtc.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };

            let receive = Receive {
                proto: Protocol::Udp,
                source,
                destination: self.local_addr,
                contents: (&buf[..n]).try_into().unwrap(),
            };

            let _ = rtc.handle_input(Input::Receive(now, receive));
        }
    }

    /// send pre-encoded NALs with FU-A fragmentation
    fn send_nals(&self, nals: &[Bytes], wallclock: Instant, media_time: u32) {
        let mid_opt = match self.video_mid.lock() {
            Ok(guard) => *guard,
            Err(_) => return,
        };
        let Some(mid) = mid_opt else { return };

        let mut rtc = match self.rtc.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let mut seq_no_guard = match self.video_seq_no.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        let pt = rtc
            .writer(mid)
            .and_then(|w| {
                w.payload_params()
                    .find(|p| format!("{:?}", p.spec().codec).to_uppercase().contains("H264"))
                    .map(|p| p.pt())
            });

        let Some(pt) = pt else { return };

        let mut direct = rtc.direct_api();
        let Some(stream) = direct.stream_tx_by_mid(mid, None) else {
            return;
        };

        for (nal_idx, nal) in nals.iter().enumerate() {
            let is_last_nal = nal_idx == nals.len() - 1;

            if nal.len() <= MTU {
                let seq = seq_no_guard.inc();
                let _ = stream.write_rtp(
                    pt,
                    seq,
                    media_time,
                    wallclock,
                    is_last_nal,
                    ExtensionValues::default(),
                    true,
                    nal.to_vec(),
                );
            } else {
                // FU-A fragmentation
                let nal_header = nal[0];
                let nal_type = nal_header & 0x1F;
                let nri = nal_header & 0x60;
                let payload = &nal[1..];

                let chunk_size = MTU - 2;
                let total_chunks = (payload.len() + chunk_size - 1) / chunk_size;

                for (chunk_idx, chunk) in payload.chunks(chunk_size).enumerate() {
                    let is_first = chunk_idx == 0;
                    let is_last_chunk = chunk_idx == total_chunks - 1;

                    let fu_indicator = 28 | nri;
                    let mut fu_header = nal_type;
                    if is_first {
                        fu_header |= 0x80;
                    }
                    if is_last_chunk {
                        fu_header |= 0x40;
                    }

                    let mut packet = Vec::with_capacity(chunk.len() + 2);
                    packet.push(fu_indicator);
                    packet.push(fu_header);
                    packet.extend_from_slice(chunk);

                    let seq = seq_no_guard.inc();
                    let marker = is_last_nal && is_last_chunk;

                    let _ = stream.write_rtp(
                        pt,
                        seq,
                        media_time,
                        wallclock,
                        marker,
                        ExtensionValues::default(),
                        true,
                        packet,
                    );
                }
            }
        }
    }

    pub async fn run_loop(self_arc: Arc<Self>) {
        // rtc driver for NACK and timeout
        let rtc_clone = self_arc.rtc.clone();
        let socket_clone = self_arc.socket.clone();
        tokio::spawn(async move {
            drive_rtc(rtc_clone, socket_clone).await;
        });

        // latch poller
        // busy wait on latest frame latch = 0 latency
        // polls network in same loop
        tokio::spawn(async move {
            tracing::info!("streamer loop started");
            let mut last_processed = 0u64;

            loop {
                if crate::is_shutdown() {
                    break;
                }
                
                // always process network
                self_arc.receive_packets();
                self_arc.poll_rtc();

                // atomic load latest frame (ArcSwapOption)
                let frame_guard = self_arc.frame_latch.load();
                
                if let Some(frame) = &*frame_guard {
                    // check if fresh
                    if frame.frame_num > last_processed {
                        let media_time = (frame.frame_num * 1500) as u32;
                        self_arc.send_nals(&frame.nals, frame.timestamp, media_time);
                        last_processed = frame.frame_num;

                        if frame.frame_num % 60 == 0 {
                            tracing::info!(
                                "streamer: sent frame {}, nals={}",
                                frame.frame_num,
                                frame.nals.len()
                            );
                        }
                    }
                }

                tokio::time::sleep(Duration::from_micros(500)).await;
            }
        });
    }
}

async fn drive_rtc(rtc: Arc<Mutex<Rtc>>, socket: Arc<UdpSocket>) {
    loop {
        if crate::is_shutdown() {
            break;
        }
        let maybe_timeout: Option<Instant> = {
            if let Ok(mut rtc_guard) = rtc.lock() {
                let t = loop {
                    match rtc_guard.poll_output() {
                        Ok(Output::Transmit(t)) => {
                            let _ = socket.send_to(&t.contents, t.destination);
                        }
                        Ok(Output::Timeout(t)) => break t,
                        Ok(Output::Event(_)) => {}
                        Err(_) => break Instant::now() + Duration::from_millis(100),
                    }
                };
                Some(t)
            } else {
                None
            }
        };

        match maybe_timeout {
            Some(timeout) => {
                let now = Instant::now();
                if timeout > now {
                    tokio::time::sleep(timeout - now).await;
                }
            }
            None => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        }

        if let Ok(mut rtc_guard) = rtc.lock() {
            let _ = rtc_guard.handle_input(Input::Timeout(Instant::now()));
        }
    }
}
