//! Screen recorder for desktop agents — captures video frames for temporal
//! analysis (animation detection, loading state recognition).
//!
//! Uses FFmpeg as the cross-platform capture backend.  Frames are stored in
//! a circular buffer so the agent can analyse recent history without
//! consuming unbounded memory.
//!
//! # Usage
//!
//! ```rust,no_run
//! use syscity::computer::screen_recorder::{ScreenRecorder, RecorderConfig};
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut recorder = ScreenRecorder::new(RecorderConfig::default())?;
//! recorder.start().await?;
//!
//! // Wait a bit, then check if the scene has stabilised
//! tokio::time::sleep(Duration::from_secs(2)).await;
//! if recorder.is_scene_stable(Duration::from_millis(500), 1000).await {
//!     println!("Loading finished — scene is stable");
//! }
//!
//! recorder.stop().await?;
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// A single captured video frame.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// Capture timestamp.
    pub timestamp: Instant,
    /// Raw RGBA pixel data (width × height × 4 bytes).
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
}

/// Configuration for the screen recorder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecorderConfig {
    /// Target capture frames per second.
    pub fps: u32,
    /// Maximum duration to keep in the circular buffer (seconds).
    pub max_buffer_secs: u64,
    /// Output resolution width (0 = native).
    pub output_width: u32,
    /// Output resolution height (0 = native).
    pub output_height: u32,
    /// Region to capture (`None` = full screen).
    pub region: Option<Rect>,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            fps: 5,
            max_buffer_secs: 10,
            output_width: 0,
            output_height: 0,
            region: None,
        }
    }
}

/// Capture region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Screen recorder backed by FFmpeg.
pub struct ScreenRecorder {
    config: RecorderConfig,
    frame_buffer: Arc<Mutex<VecDeque<VideoFrame>>>,
    running: Arc<AtomicBool>,
    capture_task: Option<tokio::task::JoinHandle<()>>,
    save_path: Option<PathBuf>,
}

impl ScreenRecorder {
    /// Create a new recorder with the given configuration.
    pub fn new(config: RecorderConfig) -> crate::computer::Result<Self> {
        Ok(Self {
            config,
            frame_buffer: Arc::new(Mutex::new(VecDeque::new())),
            running: Arc::new(AtomicBool::new(false)),
            capture_task: None,
            save_path: None,
        })
    }

    /// Start capturing frames in the background.
    pub async fn start(&mut self) -> crate::computer::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(crate::computer::ComputerError::Other(
                "Recorder already running".to_string(),
            ));
        }

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let buffer = self.frame_buffer.clone();
        let config = self.config;

        let handle = tokio::spawn(async move {
            if let Err(e) = run_ffmpeg_capture(config, buffer, running).await {
                warn!("Screen recorder capture loop exited: {}", e);
            }
        });

        self.capture_task = Some(handle);
        info!("Screen recorder started at {} fps", self.config.fps);
        Ok(())
    }

    /// Stop capturing and optionally save the buffer to a video file.
    pub async fn stop(&mut self) -> crate::computer::Result<()> {
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.capture_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }

        if let Some(path) = &self.save_path {
            self.save_buffer_to_video(path).await?;
        }

        info!("Screen recorder stopped");
        Ok(())
    }

    /// Set a path to save the recorded video when stopping.
    pub fn set_save_path<P: AsRef<Path>>(&mut self, path: P) {
        self.save_path = Some(path.as_ref().to_path_buf());
    }

    /// Get frames captured within the last `duration`.
    pub async fn recent_frames(&self, duration: Duration) -> Vec<VideoFrame> {
        let buffer = self.frame_buffer.lock().await;
        let cutoff = Instant::now() - duration;
        buffer
            .iter()
            .filter(|f| f.timestamp >= cutoff)
            .cloned()
            .collect()
    }

    /// Check whether the scene has been "stable" for the given window.
    ///
    /// Stability is defined as: the pixel-wise difference between every
    /// consecutive pair of frames in the window is below `pixel_diff_threshold`.
    pub async fn is_scene_stable(
        &self,
        window: Duration,
        pixel_diff_threshold: u32,
    ) -> bool {
        let frames = self.recent_frames(window).await;
        if frames.len() < 2 {
            return false; // Not enough history to decide.
        }

        for pair in frames.windows(2) {
            let diff = pixel_diff(&pair[0], &pair[1]);
            if diff > pixel_diff_threshold {
                return false;
            }
        }
        true
    }

    /// Get the most recent frame, if any.
    pub async fn latest_frame(&self) -> Option<VideoFrame> {
        let buffer = self.frame_buffer.lock().await;
        buffer.back().cloned()
    }

    /// Compute the pixel difference between the two most recent frames.
    pub async fn latest_frame_diff(&self) -> Option<u32> {
        let buffer = self.frame_buffer.lock().await;
        let frames: Vec<_> = buffer.iter().rev().take(2).collect();
        if frames.len() < 2 {
            return None;
        }
        Some(pixel_diff(frames[1], frames[0]))
    }

    /// Save the current frame buffer as an MP4 video using FFmpeg.
    async fn save_buffer_to_video(
        &self,
        path: &Path,
    ) -> crate::computer::Result<()> {
        // TODO: encode buffer frames into a video file.
        // For now, this is a placeholder.
        info!("Saving video to {} — not yet implemented", path.display());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FFmpeg capture backend
// ---------------------------------------------------------------------------

async fn run_ffmpeg_capture(
    config: RecorderConfig,
    buffer: Arc<Mutex<VecDeque<VideoFrame>>>,
    running: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = build_ffmpeg_command(config)?;
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or("ffmpeg has no stdout")?;

    // Read raw RGBA frames from FFmpeg stdout.
    let mut reader = tokio::io::BufReader::new(stdout);
    let _frame_size = (config.output_width * config.output_height * 4) as usize;

    // If output dimensions are not set, detect them from the platform.
    let (width, height) = if config.output_width == 0 || config.output_height == 0 {
        detect_screen_resolution().await.unwrap_or((1920, 1080))
    } else {
        (config.output_width, config.output_height)
    };

    let actual_frame_size = (width * height * 4) as usize;
    let max_frames = (config.fps as u64 * config.max_buffer_secs) as usize;

    let mut frame_data = vec![0u8; actual_frame_size];

    while running.load(Ordering::SeqCst) {
        match tokio::time::timeout(
            Duration::from_secs(2),
            reader.read_exact(&mut frame_data),
        )
        .await
        {
            Ok(Ok(_)) => {
                let frame = VideoFrame {
                    timestamp: Instant::now(),
                    data: frame_data.clone(),
                    width,
                    height,
                };

                let mut buf = buffer.lock().await;
                buf.push_back(frame);
                while buf.len() > max_frames {
                    buf.pop_front();
                }
            }
            Ok(Err(e)) => {
                warn!("FFmpeg stdout read error: {}", e);
                break;
            }
            Err(_) => {
                // Timeout — check if we should still be running.
                continue;
            }
        }
    }

    let _ = child.kill().await;
    Ok(())
}

fn build_ffmpeg_command(
    config: RecorderConfig,
) -> Result<tokio::process::Command, Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y"); // Overwrite output

    // Input format and device depend on platform.
    #[cfg(target_os = "linux")]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0.0".to_string());
        cmd.arg("-f").arg("x11grab");
        if let Some(r) = config.region {
            cmd.arg("-video_size")
                .arg(format!("{}x{}", r.width, r.height))
                .arg("-i")
                .arg(format!("{}+{}, {}", display, r.x, r.y));
        } else {
            let (w, h) = std::sync::Arc::new(std::sync::Mutex::new((1920u32, 1080u32)));
            cmd.arg("-i").arg(display);
        }
    }

    #[cfg(target_os = "macos")]
    {
        cmd.arg("-f").arg("avfoundation");
        if let Some(r) = config.region {
            cmd.arg("-video_size")
                .arg(format!("{}x{}", r.width, r.height))
                .arg("-i")
                .arg("1:"); // Capture display 1
            // TODO: apply offset crop
        } else {
            cmd.arg("-i").arg("1:");
        }
    }

    #[cfg(target_os = "windows")]
    {
        cmd.arg("-f").arg("gdigrab");
        if let Some(r) = config.region {
            cmd.arg("-video_size")
                .arg(format!("{}x{}", r.width, r.height))
                .arg("-offset_x")
                .arg(r.x.to_string())
                .arg("-offset_y")
                .arg(r.y.to_string())
                .arg("-i")
                .arg("desktop");
        } else {
            cmd.arg("-i").arg("desktop");
        }
    }

    // Framerate.
    cmd.arg("-framerate").arg(config.fps.to_string());

    // Output raw RGBA to stdout.
    cmd.arg("-pix_fmt").arg("rgba")
        .arg("-f").arg("rawvideo")
        .arg("-an") // No audio
        .arg("-");  // stdout

    Ok(cmd)
}

async fn detect_screen_resolution() -> Option<(u32, u32)> {
    #[cfg(target_os = "linux")]
    {
        // Try xrandr or xdpyinfo.
        if let Ok(output) = tokio::process::Command::new("xrandr")
            .arg("--current")
            .output()
            .await
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                // Look for "*+" marking the current mode.
                if line.contains("*+") {
                    let re = regex::Regex::new(r"(\d+)x(\d+)\+").ok()?;
                    if let Some(caps) = re.captures(line) {
                        let w = caps.get(1)?.as_str().parse().ok()?;
                        let h = caps.get(2)?.as_str().parse().ok()?;
                        return Some((w, h));
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = tokio::process::Command::new("system_profiler")
            .args(["SPDisplaysDataType", "-json"])
            .output()
            .await
        {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(displays) =
                    json.get("SPDisplaysDataType")?.as_array()
                {
                    if let Some(first) = displays.first() {
                        let w = first
                            .get("_spdisplays_pixels")?
                            .as_str()?
                            .split('x')
                            .next()?
                            .parse()
                            .ok()?;
                        let h = first
                            .get("_spdisplays_pixels")?
                            .as_str()?
                            .split('x')
                            .nth(1)?
                            .parse()
                            .ok()?;
                        return Some((w, h));
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = tokio::process::Command::new("powershell")
            .arg("-Command")
            .arg("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Size")
            .output()
            .await
        {
            let text = String::from_utf8_lossy(&output.stdout);
            let re = regex::Regex::new(r"Width\s*=\s*(\d+).*Height\s*=\s*(\d+)").ok()?;
            if let Some(caps) = re.captures(&text) {
                let w = caps.get(1)?.as_str().parse().ok()?;
                let h = caps.get(2)?.as_str().parse().ok()?;
                return Some((w, h));
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Frame analysis
// ---------------------------------------------------------------------------

/// Compute pixel-wise absolute difference between two frames.
/// Returns the number of pixels that differ by more than a small threshold.
fn pixel_diff(a: &VideoFrame, b: &VideoFrame) -> u32 {
    if a.width != b.width || a.height != b.height || a.data.len() != b.data.len() {
        // Different sizes → treat as completely different.
        return a.width * a.height;
    }

    // Sample every 16th pixel for performance (sparse diff).
    let stride = 16usize.saturating_mul(4);
    let mut diff_count = 0u32;

    for i in (0..a.data.len()).step_by(stride) {
        let end = (i + 4).min(a.data.len());
        let pa = &a.data[i..end];
        let pb = &b.data[i..end];
        let pixel_diff: u32 = pa
            .iter()
            .zip(pb.iter())
            .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs())
            .sum();
        if pixel_diff > 30 {
            diff_count += 1;
        }
    }

    // Scale back up from sparse sampling.
    diff_count * (stride as u32 / 4)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_frame(width: u32, height: u32, fill: u8) -> VideoFrame {
        let size = (width * height * 4) as usize;
        VideoFrame {
            timestamp: Instant::now(),
            data: vec![fill; size],
            width,
            height,
        }
    }

    #[test]
    fn test_pixel_diff_identical() {
        let a = make_test_frame(100, 100, 128);
        let b = make_test_frame(100, 100, 128);
        assert_eq!(pixel_diff(&a, &b), 0);
    }

    #[test]
    fn test_pixel_diff_different() {
        let a = make_test_frame(100, 100, 0);
        let b = make_test_frame(100, 100, 255);
        let diff = pixel_diff(&a, &b);
        assert!(diff > 0);
        // Sparse sampling means diff_count * sampling_factor.
        assert!(diff <= 100 * 100);
    }

    #[test]
    fn test_pixel_diff_size_mismatch() {
        let a = make_test_frame(100, 100, 0);
        let b = make_test_frame(200, 100, 0);
        assert_eq!(pixel_diff(&a, &b), 100 * 100);
    }

    #[tokio::test]
    async fn test_recorder_buffer_management() {
        let recorder = ScreenRecorder::new(RecorderConfig {
            fps: 10,
            max_buffer_secs: 1,
            output_width: 10,
            output_height: 10,
            region: None,
        })
        .unwrap();

        // Manually inject frames, simulating what the capture loop does
        // (push + cap at max_frames).
        {
            let mut buf = recorder.frame_buffer.lock().await;
            let max_frames = (recorder.config.fps as u64 * recorder.config.max_buffer_secs) as usize;
            for i in 0..20 {
                buf.push_back(make_test_frame(10, 10, i as u8));
                while buf.len() > max_frames {
                    buf.pop_front();
                }
            }
        }

        // Buffer should not exceed max_frames = fps * max_buffer_secs = 10.
        let buf = recorder.frame_buffer.lock().await;
        assert!(buf.len() <= 10, "buffer should be capped at max_frames");
    }
}
