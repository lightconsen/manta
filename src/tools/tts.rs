//! TTS Tool — Text-to-Speech
//!
//! OpenClaw-compatible tool for converting text to speech.
//! Supports OpenAI TTS API as the primary backend, with
//! fallback to macOS `say` command or festival/espeak.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, warn};

use super::{Tool, ToolContext, ToolExecutionResult};

/// Text-to-speech tool
pub struct TtsTool;

impl TtsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TtsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct TtsArgs {
    text: String,
    #[serde(default)]
    voice: Option<String>,
    #[serde(default)]
    speed: Option<f32>,
    #[serde(default)]
    output: Option<String>,
}

#[async_trait]
impl Tool for TtsTool {
    fn name(&self) -> &str {
        "tts"
    }

    fn description(&self) -> &str {
        "Convert text to speech audio. Supports OpenAI TTS API (requires OPENAI_API_KEY) \
         or local system TTS (macOS say, Linux espeak/festival)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to speak"
                },
                "voice": {
                    "type": "string",
                    "description": "Voice name (OpenAI: alloy/echo/fable/onyx/nova/shimmer; macOS: system voice names)",
                    "default": "alloy"
                },
                "speed": {
                    "type": "number",
                    "description": "Speech speed multiplier (0.25 - 4.0)",
                    "default": 1.0
                },
                "output": {
                    "type": "string",
                    "description": "Output file path (default: auto-generated .mp3 or .aiff)"
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: TtsArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid arguments: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        let output_path = if let Some(out) = args.output {
            std::path::PathBuf::from(out)
        } else {
            context
                .working_directory
                .join(format!("tts_{}.mp3", uuid::Uuid::new_v4()))
        };

        // Try OpenAI TTS API first
        if let Some(api_key) = context
            .environment
            .get("OPENAI_API_KEY")
        {
            let voice = args.voice.as_deref().unwrap_or("alloy");
            let speed = args.speed.unwrap_or(1.0).clamp(0.25, 4.0);

            let body = serde_json::json!({
                "model": "tts-1",
                "input": args.text,
                "voice": voice,
                "speed": speed,
            });

            let client = reqwest::Client::new();
            let response = client
                .post("https://api.openai.com/v1/audio/speech")
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(bytes) = resp.bytes().await {
                        if let Err(e) = tokio::fs::write(&output_path, &bytes).await {
                            return Ok(ToolExecutionResult {
                                success: false,
                                output: String::new(),
                                error: Some(format!("Failed to save audio: {}", e)),
                                data: None,
                                execution_time: start.elapsed(),
                            });
                        }

                        info!("TTS audio generated via OpenAI: {:?}", output_path);
                        return Ok(ToolExecutionResult {
                            success: true,
                            output: format!("TTS audio saved: {}", output_path.display()),
                            error: None,
                            data: Some(serde_json::json!({
                                "local_path": output_path.to_string_lossy(),
                                "voice": voice,
                                "speed": speed,
                                "provider": "openai",
                            })),
                            execution_time: start.elapsed(),
                        });
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    warn!("OpenAI TTS API error: {}", status);
                }
                Err(e) => {
                    warn!("OpenAI TTS request failed: {}", e);
                }
            }
        }

        // Fallback: macOS say command
        #[cfg(target_os = "macos")]
        {
            let output_aiff = output_path.with_extension("aiff");
            let voice = args.voice.as_deref().unwrap_or("");
            let mut cmd = tokio::process::Command::new("say");
            if !voice.is_empty() && voice != "alloy" {
                cmd.arg("-v").arg(voice);
            }
            cmd.arg("-o")
                .arg(&output_aiff)
                .arg(&args.text);

            match cmd.output().await {
                Ok(output) if output.status.success() => {
                    info!("TTS audio generated via macOS say: {:?}", output_aiff);
                    return Ok(ToolExecutionResult {
                        success: true,
                        output: format!("TTS audio saved: {}", output_aiff.display()),
                        error: None,
                        data: Some(serde_json::json!({
                            "local_path": output_aiff.to_string_lossy(),
                            "provider": "macos_say",
                        })),
                        execution_time: start.elapsed(),
                    });
                }
                _ => {}
            }
        }

        // Fallback: espeak (Linux)
        {
            let output_wav = output_path.with_extension("wav");
            let mut cmd = tokio::process::Command::new("espeak");
            cmd.arg("-w")
                .arg(&output_wav)
                .arg(&args.text);

            match cmd.output().await {
                Ok(output) if output.status.success() => {
                    info!("TTS audio generated via espeak: {:?}", output_wav);
                    return Ok(ToolExecutionResult {
                        success: true,
                        output: format!("TTS audio saved: {}", output_wav.display()),
                        error: None,
                        data: Some(serde_json::json!({
                            "local_path": output_wav.to_string_lossy(),
                            "provider": "espeak",
                        })),
                        execution_time: start.elapsed(),
                    });
                }
                _ => {}
            }
        }

        // Final fallback: just return the text (no audio generated)
        Ok(ToolExecutionResult {
            success: false,
            output: String::new(),
            error: Some(
                "No TTS provider available. Set OPENAI_API_KEY, or install espeak (Linux) / use macOS."
                    .to_string(),
            ),
            data: Some(serde_json::json!({
                "text": args.text,
                "requested_voice": args.voice,
            })),
            execution_time: start.elapsed(),
        })
    }
}
