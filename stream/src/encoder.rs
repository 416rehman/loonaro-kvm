//! x264 encoder - optimized for multi-tenant SaaS density
//! single-threaded per VM, MTU-compliant slice sizes

use anyhow::Result;
use bytes::Bytes;
use x264_sys::*;
use std::mem;

pub struct Encoder {
    pub width: i32,
    pub height: i32,
    encoder: *mut x264_t,
    pic_in: x264_picture_t,
    pts: i64,
}

// x264_t is thread-safe for sequential encoding
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        let mut param: x264_param_t = unsafe { mem::zeroed() };
        let mut pic_in: x264_picture_t = unsafe { mem::zeroed() };

        unsafe {
            x264_param_default_preset(
                &mut param,
                b"ultrafast\0".as_ptr() as *const i8,
                b"zerolatency\0".as_ptr() as *const i8,
            );

            param.i_width = width;
            param.i_height = height;
            param.i_fps_num = 60;
            param.i_fps_den = 1;
            param.i_keyint_max = 60;

            // SaaS density: limit threads to prevent CPU meltdown with 8+ VMs
            // each spawning 16 threads on a 64-core server
            param.i_threads = 1;
            param.i_lookahead_threads = 1;
            param.b_vfr_input = 0;

            // Disable Annex-B (start codes). Produces length-prefixed NALs
            // which we strip - cleaner than scanning for variable start codes
            param.b_annexb = 0;

            x264_param_apply_profile(
                &mut param,
                b"constrained_baseline\0".as_ptr() as *const i8,
            );

            param.rc.i_rc_method = X264_RC_CRF as i32;
            // slightly higher CRF for network stability under graphically intense scenes
            param.rc.f_rf_constant = 26.0;
            param.b_repeat_headers = 1;

            // MTU compliance - x264 ensures no NAL exceeds this size
            // this moves fragmentation work to encoder's optimized asm routines
            param.i_slice_max_size = 1200;
            param.i_slice_count = 0;

            x264_picture_init(&mut pic_in);

            let encoder = x264_encoder_open(&mut param);
            if encoder.is_null() {
                return Err(anyhow::anyhow!("failed to open x264 encoder"));
            }

            pic_in.img.i_csp = X264_CSP_I420 as i32;
            pic_in.img.i_plane = 3;

            Ok(Self {
                encoder,
                width,
                height,
                pic_in,
                pts: 0,
            })
        }
    }

    /// encode yuv frame, returns NAL units as Vec<Bytes> for zero-copy handoff
    pub fn encode(&mut self, yuv: &[u8], force_intra: bool) -> Result<Vec<Bytes>> {
        let mut nal_out = std::ptr::null_mut();
        let mut i_nal = 0;
        let mut pic_out: x264_picture_t = unsafe { mem::zeroed() };

        let y_size = (self.width * self.height) as usize;
        let uv_size = y_size / 4;

        unsafe {
            // pointer arithmetic for plane mapping - avoids bounds checks
            self.pic_in.img.plane[0] = yuv.as_ptr() as *mut u8;
            self.pic_in.img.plane[1] = yuv.as_ptr().add(y_size) as *mut u8;
            self.pic_in.img.plane[2] = yuv.as_ptr().add(y_size + uv_size) as *mut u8;

            self.pic_in.img.i_stride[0] = self.width;
            self.pic_in.img.i_stride[1] = self.width / 2;
            self.pic_in.img.i_stride[2] = self.width / 2;

            self.pic_in.i_pts = self.pts;
            self.pts += 1;

            self.pic_in.i_type = if force_intra {
                X264_TYPE_IDR as i32
            } else {
                X264_TYPE_AUTO as i32
            };

            let ret = x264_encoder_encode(
                self.encoder,
                &mut nal_out,
                &mut i_nal,
                &mut self.pic_in,
                &mut pic_out,
            );

            if ret < 0 {
                return Err(anyhow::anyhow!("x264 encode failed: {}", ret));
            }

            let mut nals = Vec::with_capacity(i_nal as usize);
            for i in 0..i_nal {
                let nal = *nal_out.offset(i as isize);

                // b_annexb=0 means first 4 bytes are length (big-endian)
                // WebRTC/str0m wants raw NAL payload without length prefix
                if nal.i_payload > 4 {
                    let payload =
                        std::slice::from_raw_parts(nal.p_payload.add(4), (nal.i_payload - 4) as usize);
                    nals.push(Bytes::copy_from_slice(payload));
                }
            }
            Ok(nals)
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            x264_encoder_close(self.encoder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_produces_mtu_compliant_nals() {
        let mut enc = Encoder::new(640, 480).unwrap();
        let yuv = vec![128u8; 640 * 480 * 3 / 2]; // gray frame
        let nals = enc.encode(&yuv, true).unwrap();

        assert!(!nals.is_empty(), "encoder should produce NALs");
        for nal in &nals {
            assert!(
                nal.len() <= 1200,
                "NAL exceeds MTU: {} bytes",
                nal.len()
            );
        }
    }
}
