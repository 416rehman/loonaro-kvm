//! PS/2 Set 2 Scancode mapping
//! This module converts Javascript/WebRTC key codes to PS/2 Set 2 scancodes.

pub fn map_key(key: u32) -> Option<Vec<u8>> {
    match key {
        // Alpha (A-Z)
        65 => Some(vec![0x1C]), // A
        66 => Some(vec![0x32]), // B
        67 => Some(vec![0x21]), // C
        68 => Some(vec![0x23]), // D
        69 => Some(vec![0x24]), // E
        70 => Some(vec![0x2B]), // F
        71 => Some(vec![0x34]), // G
        72 => Some(vec![0x33]), // H
        73 => Some(vec![0x43]), // I
        74 => Some(vec![0x3B]), // J
        75 => Some(vec![0x42]), // K
        76 => Some(vec![0x4B]), // L
        77 => Some(vec![0x3A]), // M
        78 => Some(vec![0x31]), // N
        79 => Some(vec![0x44]), // O
        80 => Some(vec![0x4D]), // P
        81 => Some(vec![0x15]), // Q
        82 => Some(vec![0x2D]), // R
        83 => Some(vec![0x1B]), // S
        84 => Some(vec![0x2C]), // T
        85 => Some(vec![0x3C]), // U
        86 => Some(vec![0x2A]), // V
        87 => Some(vec![0x1D]), // W
        88 => Some(vec![0x22]), // X
        89 => Some(vec![0x35]), // Y
        90 => Some(vec![0x1A]), // Z

        // Numeric (0-9) top row
        48 => Some(vec![0x45]), // 0
        49 => Some(vec![0x16]), // 1
        50 => Some(vec![0x1E]), // 2
        51 => Some(vec![0x26]), // 3
        52 => Some(vec![0x25]), // 4
        53 => Some(vec![0x2E]), // 5
        54 => Some(vec![0x36]), // 6
        55 => Some(vec![0x3D]), // 7
        56 => Some(vec![0x3E]), // 8
        57 => Some(vec![0x46]), // 9

        // Control Keys
        13 => Some(vec![0x5A]), // Enter
        32 => Some(vec![0x29]), // Space
        8  => Some(vec![0x66]), // Backspace
        9  => Some(vec![0x0D]), // Tab
        27 => Some(vec![0x76]), // Esc
        16 => Some(vec![0x12]), // Shift (Left) - Simplified
        17 => Some(vec![0x14]), // Ctrl (Left)
        18 => Some(vec![0x11]), // Alt (Left)

        // Arrows (Extended: E0 xx)
        37 => Some(vec![0xE0, 0x6B]), // Left
        38 => Some(vec![0xE0, 0x75]), // Up
        39 => Some(vec![0xE0, 0x74]), // Right
        40 => Some(vec![0xE0, 0x72]), // Down

        _ => None,
    }
}

pub fn make_break(scancode: &[u8], pressed: bool) -> Vec<u8> {
    if pressed {
        scancode.to_vec()
    } else {
        // Break code is 0xF0 followed by Make code
        let mut v = vec![0xF0];
        v.extend_from_slice(scancode);
        v
    }
}
