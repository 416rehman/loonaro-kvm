use str0m::media::MediaWriter;
use std::time::Instant;
use str0m::media::{Pt, MediaTime};

fn test_api(writer: &mut MediaWriter) {
    let pt = Pt::from(96);
    let time = Instant::now();
    let media_time = MediaTime::new(0, 90000);
    let data = vec![0u8; 10];
    
    // Attempt 1: Look for write_with_marker
    writer.write_with_marker(pt, time, media_time, &data, false);
}

fn main() {}
