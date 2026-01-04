use crate::webrtc::Streamer;
use std::sync::Arc;
use warp::Filter;


pub struct SignalingServer {
    port: u16,
}

impl SignalingServer {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub async fn run(self, streamer: Arc<Streamer>) {
        let streamer_filter = warp::any().map(move || streamer.clone());
        
        // POST /sdp
        let sdp = warp::post()
            .and(warp::path("sdp"))
            .and(warp::body::json())
            .and(streamer_filter)
            .and_then(|body: serde_json::Value, streamer: Arc<Streamer>| async move {
                let sdp_str = body["sdp"].as_str().unwrap_or("");
                tracing::info!("received offer");
                
                match streamer.handle_offer(sdp_str) {
                    Ok(answer) => {
                        tracing::info!("generated answer");
                        // FORCE Level 4.2 (42e02a) over 3.1 (42e01f) to support 1080p
                        let patched_answer = answer.replace("42e01f", "42e02a");
                        
                        Ok::<_, warp::Rejection>(warp::reply::json(&serde_json::json!({
                            "sdp": patched_answer,
                            "type": "answer"
                        })))
                    },
                    Err(e) => {
                        tracing::error!("sdp error: {}", e);
                        // Return 500 error properly by rejecting with custom error
                        // For simplicity, using not_found but logging the error is key
                        // Better: warp::reply::with_status
                        // But can't return different types easily in and_then without Box
                        // Let's just log and return 400 for bad requests/failures
                        Err(warp::reject::custom(SdpError))
                    }
                }
            });

        // 405/404 handling: warp rejects if method doesn't match
        // We served static files via absolute path
        let www_path = "/home/ubuntu/projects/loonaro-kvm/stream/www";
        let static_files = warp::fs::dir(www_path);
        
        // recover errors for better UX
        let routes = sdp.or(static_files)
            .recover(handle_rejection);

        tracing::info!("signaling on :{}", self.port);
        warp::serve(routes).run(([0, 0, 0, 0], self.port)).await;
    }
}

#[derive(Debug)]
struct SdpError;
impl warp::reject::Reject for SdpError {}

async fn handle_rejection(err: warp::Rejection) -> Result<impl warp::Reply, std::convert::Infallible> {
    if err.is_not_found() {
        Ok(warp::reply::with_status("Not Found", warp::http::StatusCode::NOT_FOUND))
    } else if let Some(_) = err.find::<SdpError>() {
        Ok(warp::reply::with_status("SDP Negotiation Failed", warp::http::StatusCode::INTERNAL_SERVER_ERROR))
    } else {
        Ok(warp::reply::with_status("Method Not Allowed", warp::http::StatusCode::METHOD_NOT_ALLOWED))
    }
}
