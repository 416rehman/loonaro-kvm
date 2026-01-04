// LOONARO STREAM //
// VOID TRANSMISSION UI //

const container = document.getElementById('container');
const canvas = document.getElementById('stream-canvas');
const ctx = canvas.getContext('2d');

// HUD
const statusTextEl = document.getElementById('status-text');
const statusDotEl = document.querySelector('.status-dot');
const fpsEl = document.getElementById('hud-fps');

// Overlay
const overlay = document.getElementById('overlay');
const overlayTitle = document.getElementById('overlay-title');
const overlayDesc = document.getElementById('overlay-desc');

// State
let pc = null;
let dc = null;
let frameCount = 0;
let lastStatsTime = performance.now();
let lastFrameTime = performance.now();
let isConnected = false;

// --- UI CONTROLLER ---

function setOverlay(show, title, desc, type = 'loading') {
    if (show) {
        // Activate depth effect
        container.classList.add('overlay-active');

        // Show Overlay
        overlay.classList.remove('hidden', 'loading', 'error', 'warning');
        overlay.classList.add(type);

        // Update Text
        if (title) overlayTitle.innerText = title;
        if (desc) overlayDesc.innerText = desc;

        isConnected = false;
        if (document.pointerLockElement) document.exitPointerLock();

    } else {
        // Deactivate depth effect
        container.classList.remove('overlay-active');

        // Hide Overlay
        overlay.classList.add('hidden');

        isConnected = true;
        lastFrameTime = performance.now();
    }
}

function updateHUDStatus(text, colorHex) {
    statusTextEl.innerText = text;
    statusDotEl.style.color = colorHex;
    statusDotEl.style.boxShadow = `0 0 10px ${colorHex}`;
}

// --- RENDER LOOP ---

function renderFrame(frame) {
    if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
    }
    ctx.drawImage(frame, 0, 0);
}

async function readLoop(reader) {
    while (true) {
        try {
            const { done, value } = await reader.read();
            if (done) break;
            if (!value) continue;

            if (frameCount === 0) {
                // First frame: Immediate ready state
                setOverlay(false);
                updateHUDStatus('UPLINK_ESTABLISHED', '#fff');
            }

            lastFrameTime = performance.now();
            renderFrame(value);
            value.close();
            frameCount++;
        } catch (e) {
            console.error(e);
            break;
        }
    }
}

// Watchdog (200ms)
setInterval(() => {
    if (!isConnected) return;
    const now = performance.now();
    // 2000ms threshold
    if (now - lastFrameTime > 2000) {
        setOverlay(true, 'SIGNAL LOSS', 'UPLINK SIGNAL INTERRUPTED', 'warning');
        updateHUDStatus('SIGNAL_LOST', '#ffbb33');
    }
}, 200);

// Stats (1s)
setInterval(() => {
    const now = performance.now();
    const fps = frameCount / ((now - lastStatsTime) / 1000);
    fpsEl.innerText = fps.toFixed(0).padStart(2, '0');
    frameCount = 0;
    lastStatsTime = now;
}, 1000);


// --- WEBRTC CORE ---

function checkState() {
    if (!pc) return;
    const ice = pc.iceConnectionState;
    const cs = pc.connectionState;

    if (ice === 'connected' || ice === 'completed') {
        updateHUDStatus('SECURE_LINK', '#10b981');
    } else if (ice === 'failed' || cs === 'failed') {
        setOverlay(true, 'SYSTEM FAILURE', 'SECURE HANDSHAKE FAILED', 'error');
        updateHUDStatus('FAILURE', '#ff4444');
    } else if (ice === 'disconnected' || cs === 'disconnected') {
        setOverlay(true, 'DISCONNECTED', 'REMOTE TERMINATED SESSION', 'warning');
        updateHUDStatus('OFFLINE', '#ffbb33');
    } else {
        if (!isConnected) {
            setOverlay(true, 'SYSTEM START', 'INITIALIZING SECURE UPLINK...', 'loading');
            updateHUDStatus('BOOT_SEQUENCE', '#fff');
        }
    }
}

async function start() {
    try {
        setOverlay(true, 'SYSTEM START', 'INITIALIZING SECURE UPLINK...', 'loading');
        updateHUDStatus('BOOT', '#fff');

        pc = new RTCPeerConnection({ iceServers: [{ urls: 'stun:stun.l.google.com:19302' }] });
        pc.oniceconnectionstatechange = checkState;
        pc.onconnectionstatechange = checkState;

        dc = pc.createDataChannel('input', { ordered: true });
        // No logs on open/close, just State check and UI update
        dc.onopen = () => checkState();
        dc.onclose = () => {
            setOverlay(true, 'CHANNEL LOST', 'INPUT SUBSYSTEM FAILED', 'warning');
        };

        pc.ontrack = evt => {
            if (evt.track.kind === 'video') {
                updateHUDStatus('VIDEO_SYNC', '#3b82f6');
                const processor = new MediaStreamTrackProcessor({ track: evt.track });
                readLoop(processor.readable.getReader());
            }
        };
        pc.addTransceiver('video', { direction: 'recvonly' });

        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        const res = await fetch('/sdp', {
            method: 'POST',
            body: JSON.stringify({ sdp: pc.localDescription.sdp, type: 'offer' }),
            headers: { 'Content-Type': 'application/json' }
        });

        if (!res.ok) throw new Error("SIGNAL_SERVER_UNREACHABLE");

        const ans = await res.json();
        await pc.setRemoteDescription(new RTCSessionDescription({ type: 'answer', sdp: ans.sdp }));

    } catch (e) {
        console.error(e); // Keep error logs for debugging
        setOverlay(true, 'CRITICAL ERROR', e.message.toUpperCase(), 'error');
    }
}

// Input Helpers
const sendInput = (e) => { if (dc && dc.readyState === 'open') dc.send(JSON.stringify(e)); };

canvas.addEventListener('click', () => {
    if (!isConnected) return;
    if (document.pointerLockElement !== canvas) canvas.requestPointerLock();
});

// Event Listeners
['mousemove', 'mousedown', 'mouseup'].forEach(evtType => {
    canvas.addEventListener(evtType, e => {
        if (!isConnected || document.pointerLockElement !== canvas) return;
        const msg = { type: (evtType === 'mousemove' ? 'MouseMove' : 'MouseButton') };
        if (evtType === 'mousemove') { msg.x = e.movementX; msg.y = e.movementY; }
        else { msg.button = e.button; msg.pressed = (evtType === 'mousedown'); }
        sendInput(msg);
    });
});

['keydown', 'keyup'].forEach(evtType => {
    window.addEventListener(evtType, e => {
        if (!isConnected || document.pointerLockElement !== canvas) return;
        e.preventDefault();
        sendInput({ type: 'Keyboard', key: e.keyCode, pressed: (evtType === 'keydown') });
    });
});

start();
