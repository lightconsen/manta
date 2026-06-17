//! End-to-end screen recorder tests.
//!
//! Tests the ffmpeg auto-download and video encoding pipeline with actual
//! ffmpeg binaries.  Skips gracefully when ffmpeg cannot be resolved.
//!
//! Run with:
//! ```bash
//! cargo test --test e2e_test -- screen_recorder -- --include-ignored
//! ```

use std::path::Path;
use syscity::computer::screen_recorder::{RecorderConfig, ScreenRecorder, VideoFrame};
use syscity::computer::resolve_or_download_ffmpeg;

// ── Helpers ────────────────────────────────────────────────────────────

fn make_test_frame(width: u32, height: u32, fill: u8) -> VideoFrame {
    let size = (width * height * 4) as usize;
    VideoFrame {
        timestamp: std::time::Instant::now(),
        data: vec![fill; size],
        width,
        height,
    }
}

// ── FFmpeg resolution tests ──────────────────────────────────────────

#[tokio::test]
#[ignore = "Downloads ffmpeg (~30 MB) if not on PATH"]
async fn test_resolve_ffmpeg_path() {
    let result = resolve_or_download_ffmpeg().await;
    assert!(
        result.is_ok(),
        "resolve_or_download_ffmpeg should succeed: {:?}",
        result.err()
    );
    let path = result.unwrap();
    assert!(
        path.exists() || path == Path::new("ffmpeg"),
        "Resolved path should exist or be 'ffmpeg' (on PATH): {}",
        path.display()
    );
}

#[tokio::test]
#[ignore = "Downloads ffmpeg (~30 MB) if not on PATH"]
async fn test_resolve_ffmpeg_is_executable() {
    let ffmpeg = resolve_or_download_ffmpeg()
        .await
        .expect("ffmpeg resolution");

    let output = tokio::process::Command::new(&ffmpeg)
        .arg("-version")
        .output()
        .await
        .expect("ffmpeg -version should run");

    assert!(
        output.status.success(),
        "ffmpeg -version exited with status: {}",
        output.status
    );

    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        !version.is_empty(),
        "ffmpeg -version should produce output"
    );
    assert!(
        version.to_lowercase().contains("ffmpeg"),
        "Output should mention ffmpeg"
    );
}

// ── Video encoding tests ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "Requires ffmpeg binary"]
async fn test_save_buffer_to_video_creates_file() {
    let tmp = std::env::temp_dir().join(format!(
        "syscity_e2e_recorder_{}.mp4",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    // Create recorder, inject frames manually, then save
    let mut recorder = ScreenRecorder::new(RecorderConfig {
        fps: 10,
        max_buffer_secs: 1,
        output_width: 16,
        output_height: 16,
        region: None,
    })
    .unwrap();

    // Inject 5 frames of 16x16 RGBA using the public API
    for i in 0..5 {
        recorder.inject_frame(make_test_frame(16, 16, (i * 50) as u8)).await;
    }

    // Save via the private method — we can't call it directly,
    // so we set a save path and call stop()
    recorder.set_save_path(&tmp);
    recorder.stop().await.expect("stop() should save video");

    // Verify the file was created and has content
    assert!(
        tmp.exists(),
        "Output file should exist: {}",
        tmp.display()
    );
    let metadata = std::fs::metadata(&tmp).expect("metadata");
    assert!(
        metadata.len() > 0,
        "Output file should not be empty (got {} bytes)",
        metadata.len()
    );

    // Cleanup
    std::fs::remove_file(&tmp).ok();
}

#[tokio::test]
#[ignore = "Requires ffmpeg binary"]
async fn test_save_buffer_to_video_empty() {
    let tmp = std::env::temp_dir().join(format!(
        "syscity_e2e_recorder_empty_{}.mp4",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let mut recorder = ScreenRecorder::new(RecorderConfig::default()).unwrap();
    // No frames injected — empty buffer
    recorder.set_save_path(&tmp);
    recorder.stop().await.expect("stop() with empty buffer");

    // File should NOT be created for empty buffer
    assert!(!tmp.exists(), "No file should be created for empty buffer");
}
