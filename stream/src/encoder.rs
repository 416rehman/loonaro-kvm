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

unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        let mut param: x264_param_t = unsafe { mem::zeroed() };
        let mut pic_in: x264_sys::x264_picture_t = unsafe { std::mem::zeroed() };

        unsafe {
            // ultrafast preset, zerolatency tune
            x264_sys::x264_param_default_preset(&mut param, b"ultrafast\0".as_ptr() as *const i8, b"zerolatency\0".as_ptr() as *const i8);
            
            param.i_width = width as i32;
            param.i_height = height as i32;
            param.i_fps_num = 60;
            param.i_fps_den = 1;
            param.i_keyint_max = 60;
            
            // Constrained Baseline Profile
            x264_sys::x264_param_apply_profile(&mut param, b"constrained_baseline\0".as_ptr() as *const i8);
            
            param.rc.i_rc_method = x264_sys::X264_RC_CRF as i32;
            param.rc.f_rf_constant = 25.0;
            param.b_repeat_headers = 1;

            // Limit slice size for RTP MTU compliance (very important)
            param.i_slice_max_size = 1200; 
            param.i_slice_count = 0; // let max_size determine count
            
            x264_picture_init(&mut pic_in);
        }
        
        // Set dimensions explicitly since we didn't alloc
        pic_in.img.i_csp = x264_sys::X264_CSP_I420 as i32;
        pic_in.img.i_plane = 3;

        let encoder = unsafe { x264_sys::x264_encoder_open(&mut param) };
        if encoder.is_null() {
            return Err(anyhow::anyhow!("failed to open encoder"));
        }

        Ok(Self {
            encoder,
            width: width as i32,
            height: height as i32,
            pic_in,
            pts: 0,
        })
    }

    /// encode yuv frame, returns nal units as Vec<Bytes> for zero-copy
    pub fn encode(&mut self, yuv: &[u8], force_intra: bool) -> Result<Vec<Bytes>> {
        let mut nal_out = std::ptr::null_mut();
        let mut i_nal = 0;
        
        let mut pic_out: x264_sys::x264_picture_t = unsafe { std::mem::zeroed() };
        
        self.pic_in.img.plane[0] = yuv.as_ptr() as *mut u8;
        self.pic_in.img.plane[1] = yuv[self.width as usize * self.height as usize..].as_ptr() as *mut u8;
        self.pic_in.img.plane[2] = yuv[self.width as usize * self.height as usize + (self.width as usize * self.height as usize)/4..].as_ptr() as *mut u8;
        
        // stride needs to be correct (width for Y, width/2 for UV)
        self.pic_in.img.i_stride[0] = self.width as i32;
        self.pic_in.img.i_stride[1] = self.width as i32 / 2;
        self.pic_in.img.i_stride[2] = self.width as i32 / 2;
        
        self.pic_in.i_pts = self.pts;
        self.pts += 1;
        
        if force_intra {
            self.pic_in.i_type = x264_sys::X264_TYPE_IDR as i32;
        } else {
            self.pic_in.i_type = x264_sys::X264_TYPE_AUTO as i32;
        }

        let ret = unsafe {
            x264_sys::x264_encoder_encode(
                self.encoder,
                &mut nal_out,
                &mut i_nal,
                &mut self.pic_in,
                &mut pic_out
            )
        };

        if ret < 0 {
            return Err(anyhow::anyhow!("x264 encode failed: {}", ret));
        }

        let mut nals = Vec::new();
        if i_nal > 0 {
             for i in 0..i_nal {
                 let nal = unsafe { *nal_out.offset(i as isize) };
                 let payload = unsafe {
                     std::slice::from_raw_parts(nal.p_payload, nal.i_payload as usize)
                 };
                 
                 // Strip start code (00 00 01 or 00 00 00 01)
                 let mut start = 0;
                 if payload.len() > 4 && payload[0] == 0 && payload[1] == 0 {
                     if payload[2] == 1 {
                         start = 3;
                     } else if payload[2] == 0 && payload[3] == 1 {
                         start = 4;
                     }
                 }
                 
                 // Only add if we have payload left
                 if start < payload.len() {
                     nals.push(Bytes::copy_from_slice(&payload[start..]));
                 }
             }
        }
        Ok(nals)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            // x264_picture_clean(&mut self.pic_in); // Do NOT clean, we didn't alloc
            x264_encoder_close(self.encoder);
        }
    }
}
