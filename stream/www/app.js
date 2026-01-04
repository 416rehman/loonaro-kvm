// zero-latency webrtc presenter
// uses MediaStreamTrackProcessor for direct frame access

const canvas = document.getElementById('stream-canvas');
const ctx = canvas.getContext('2d');
const status = document.getElementById('status');
const stats = document.getElementById('stats');

let pc = null;
let dc = null;
let frameCount = 0;
let lastStatsTime = performance.now();

// Render frame to canvas
function renderFrame(frame) {
    if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
    }
    ctx.drawImage(frame, 0, 0);
}

// Read frames from track
async function readLoop(reader) {
    console.log("Starting read loop");
    while (true) {
        try {
            const result = await reader.read();
            if (result.done) {
                console.log("Read loop done");
                break;
            }
            const frame = result.value;
            if (!frame) continue;

            // Log first frame and periodic frames
            if (frameCount === 0) console.log("Received FIRST Frame:", frame.displayWidth, "x", frame.displayHeight, frame.timestamp);
            if (frameCount % 60 === 0) console.log("Received Frame:", frameCount, frame.timestamp);

            renderFrame(frame);
            frame.close();
            frameCount++;
        } catch (e) {
            console.error("Read loop error:", e);
            break;
        }
    }
}

// Report WebRTC stats
async function reportStats() {
    if (!pc) return;
    try {
        const s = await pc.getStats();
        s.forEach(report => {
            if (report.type === 'inbound-rtp' && report.kind === 'video') {
                console.log("Stats:", {
                    bytes: report.bytesReceived,
                    decoded: report.framesDecoded,
                    dropped: report.framesDropped,
                    nack: report.nackCount,
                    pli: report.pliCount,
                    fps: report.framesPerSecond,
                    packetsLost: report.packetsLost
                });

                if (dc && dc.readyState === 'open') {
                    const msg = {
                        type: 'stats',
                        bytes: report.bytesReceived,
                        decoded: report.framesDecoded,
                        keyFrames: report.keyFramesDecoded,
                        fps: report.framesPerSecond
                    };
                    dc.send(JSON.stringify(msg));
                }
            }
        });
    } catch (e) { console.error("Stats error", e); }
}

// Local stats updates
function updateStats() {
    const now = performance.now();
    const elapsed = (now - lastStatsTime) / 1000;
    const fps = frameCount / elapsed;
    stats.textContent = `${fps.toFixed(1)} fps`;
    frameCount = 0;
    lastStatsTime = now;

    // Check connection stats
    reportStats().catch(console.error);
}
setInterval(updateStats, 1000);

// Input handling
function sendInput(event) {
    if (dc && dc.readyState === 'open') {
        dc.send(JSON.stringify(event));
    }
}

// Pointer lock & listeners
canvas.addEventListener('click', () => {
    if (document.pointerLockElement !== canvas) canvas.requestPointerLock();
});

canvas.addEventListener('mousemove', (e) => {
    if (document.pointerLockElement === canvas) sendInput({ type: 'MouseMove', x: e.movementX, y: e.movementY });
});

canvas.addEventListener('mousedown', (e) => {
    if (document.pointerLockElement === canvas) sendInput({ type: 'MouseButton', button: e.button, pressed: true });
});

canvas.addEventListener('mouseup', (e) => {
    if (document.pointerLockElement === canvas) sendInput({ type: 'MouseButton', button: e.button, pressed: false });
});

window.addEventListener('keydown', (e) => {
    if (document.pointerLockElement === canvas) {
        e.preventDefault();
        sendInput({ type: 'Keyboard', key: e.keyCode, pressed: true });
    }
});

window.addEventListener('keyup', (e) => {
    if (document.pointerLockElement === canvas) {
        e.preventDefault();
        sendInput({ type: 'Keyboard', key: e.keyCode, pressed: false });
    }
});

// Start connection
async function start() {
    try {
        console.log('starting webrtc connection');
        status.textContent = 'Connecting...';

        pc = new RTCPeerConnection({
            iceServers: [{ urls: 'stun:stun.l.google.com:19302' }]
        });

        // Logging state changes
        pc.onconnectionstatechange = () => console.log("pc connection state:", pc.connectionState);
        pc.oniceconnectionstatechange = () => console.log("pc ice connection state:", pc.iceConnectionState);
        pc.onicegatheringstatechange = () => console.log("pc ice gathering state:", pc.iceGatheringState);
        pc.onsignalingstatechange = () => console.log("pc signaling state:", pc.signalingState);

        // Data channel
        dc = pc.createDataChannel('input', { ordered: true });
        dc.onopen = () => {
            console.log('datachannel open');
            status.textContent = 'Connected - Click to capture input';
        };
        dc.onclose = () => console.log("datachannel closed");
        dc.onerror = (e) => console.log("datachannel error:", e);
        dc.onmessage = (e) => console.log("dc message:", e.data);

        // Track handling
        pc.ontrack = (event) => {
            console.log('pc ontrack:', event.track.kind, event.track.id);
            status.textContent = 'Video Track Received';

            if (event.track.kind === 'video') {
                event.track.onmute = () => console.log("track muted");
                event.track.onunmute = () => console.log("track unmuted");

                if (typeof MediaStreamTrackProcessor !== 'undefined') {
                    const processor = new MediaStreamTrackProcessor({ track: event.track });
                    const reader = processor.readable.getReader();
                    readLoop(reader);
                } else {
                    console.error("MediaStreamTrackProcessor not supported!");
                }
            }
        };

        pc.addTransceiver('video', { direction: 'recvonly' });

        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        const response = await fetch('/sdp', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ sdp: pc.localDescription.sdp, type: 'offer' })
        });

        if (!response.ok) throw new Error('Signaling failed');

        const answer = await response.json();
        console.log('received answer');

        await pc.setRemoteDescription(new RTCSessionDescription({
            type: 'answer',
            sdp: answer.sdp
        }));

    } catch (err) {
        console.error(err);
        status.textContent = 'Error: ' + err.message;
    }
}

start();
