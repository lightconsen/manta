//! Screencast recording: collect JPEG frames from a browser page into an
//! artifacts directory until stopped.

use base64::Engine;
use serde_json::json;
use tracing::warn;

struct ScreencastSession {
    /// Directory receiving frame-*.jpg files
    dir: std::path::PathBuf,
    /// Number of frames written so far
    frames: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Collector task reading screencastFrame events
    task: tokio::task::JoinHandle<()>,
}

/// Active screencast sessions keyed by page target id.
#[cfg(feature = "browser")]
fn screencast_sessions(
) -> &'static tokio::sync::Mutex<std::collections::HashMap<String, ScreencastSession>> {
    static SESSIONS: std::sync::OnceLock<
        tokio::sync::Mutex<std::collections::HashMap<String, ScreencastSession>>,
    > = std::sync::OnceLock::new();
    SESSIONS.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Start a screencast: JPEG frames are written to
/// `~/.syscity/artifacts/screencast-<timestamp>/frame-*.jpg`.
#[cfg(feature = "browser")]
pub(super) async fn screencast_start(
    page: &chromiumoxide::Page,
    quality: Option<u32>,
    every_nth_frame: Option<u32>,
) -> Result<serde_json::Value, String> {
    use chromiumoxide::cdp::browser_protocol::page::{
        EventScreencastFrame, StartScreencastFormat, StartScreencastParams,
    };
    use futures::StreamExt;

    let key = page.target_id().as_ref().to_string();
    let mut sessions = screencast_sessions().lock().await;
    if sessions.contains_key(&key) {
        return Err("Screencast already active for this page".to_string());
    }

    let dir = crate::dirs::syscity_dir()
        .join("artifacts")
        .join(format!("screencast-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create screencast dir: {}", e))?;

    // Subscribe before starting so no frames are missed.
    let mut events = page
        .event_listener::<EventScreencastFrame>()
        .await
        .map_err(|e| format!("Failed to subscribe screencast events: {}", e))?;

    let mut params = StartScreencastParams {
        format: Some(StartScreencastFormat::Jpeg),
        quality: Some(i64::from(quality.unwrap_or(80).min(100))),
        ..Default::default()
    };
    params.every_nth_frame = every_nth_frame.map(|n| i64::from(n.max(1)));
    page.execute(params)
        .await
        .map_err(|e| format!("Failed to start screencast: {}", e))?;

    let frames = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let frames_task = frames.clone();
    let page_task = page.clone();
    let dir_task = dir.clone();
    let task = tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            // Ack each frame so Chrome keeps delivering.
            if let Err(e) = page_task
                .execute(chromiumoxide::cdp::browser_protocol::page::ScreencastFrameAckParams::new(
                    ev.session_id,
                ))
                .await
            {
                warn!("screencast ack failed, stopping collector: {}", e);
                break;
            }
            let idx = frames_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(AsRef::<str>::as_ref(&ev.data))
                .unwrap_or_default();
            let path = dir_task.join(format!("frame-{idx:05}.jpg"));
            if let Err(e) = tokio::fs::write(&path, decoded).await {
                warn!("Failed to write screencast frame: {}", e);
            }
        }
    });

    sessions.insert(key, ScreencastSession { dir: dir.clone(), frames, task });
    Ok(json!({
        "success": true,
        "frames_dir": dir,
        "note": "Frames are saved as frame-*.jpg. Call screencast_stop to finish."
    }))
}

/// Stop the active screencast for this page.
#[cfg(feature = "browser")]
pub(super) async fn screencast_stop(
    page: &chromiumoxide::Page,
) -> Result<serde_json::Value, String> {
    use chromiumoxide::cdp::browser_protocol::page::StopScreencastParams;

    let key = page.target_id().as_ref().to_string();
    let session = screencast_sessions().lock().await.remove(&key);
    let Some(session) = session else {
        return Err("No active screencast for this page".to_string());
    };
    if let Err(e) = page.execute(StopScreencastParams {}).await {
        warn!("stopScreencast command failed: {}", e);
    }
    session.task.abort();
    let frames = session.frames.load(std::sync::atomic::Ordering::SeqCst);
    Ok(json!({
        "success": true,
        "frames_dir": session.dir,
        "frames": frames
    }))
}
