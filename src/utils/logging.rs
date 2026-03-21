//! Logging setup for Manta
//!
//! This module initializes the tracing subscriber with the
//! configuration specified in the config file.

use crate::config::{Config, LogFormat};
use crate::error::Result;
use std::io;
use std::path::Path;
use tracing::Level;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialize logging based on configuration
pub fn init_logging(config: &Config) -> Result<()> {
    let level = parse_log_level(&config.logging.level);

    let filter = EnvFilter::new(level.to_string())
        .add_directive("hyper=warn".parse().unwrap_or_else(|_| level.into()))
        .add_directive("reqwest=warn".parse().unwrap_or_else(|_| level.into()));

    let registry = tracing_subscriber::registry().with(filter);

    // Configure the formatter based on the format setting
    match config.logging.format {
        LogFormat::Json => {
            init_json_logging(registry, config)?;
        }
        LogFormat::Pretty => {
            init_pretty_logging(registry, config)?;
        }
        LogFormat::Compact => {
            init_compact_logging(registry, config)?;
        }
    }

    tracing::info!("Logging initialized with level: {}", level);
    Ok(())
}

/// Initialize JSON format logging
fn init_json_logging<S>(registry: S, config: &Config) -> Result<()>
where
    S: tracing::Subscriber
        + Send
        + Sync
        + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    if config.logging.stdout {
        let stdout_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .with_thread_ids(true)
            .with_current_span(true)
            .with_span_list(true)
            .with_writer(io::stdout);

        if let Some(ref log_file) = config.logging.file {
            let file = create_log_file(log_file)?;
            let file_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_target(true)
                .with_thread_ids(true)
                .with_current_span(true)
                .with_span_list(true)
                .with_writer(file);
            registry.with(stdout_layer).with(file_layer).init();
        } else {
            registry.with(stdout_layer).init();
        }
    } else if let Some(ref log_file) = config.logging.file {
        let file = create_log_file(log_file)?;
        let file_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .with_thread_ids(true)
            .with_current_span(true)
            .with_span_list(true)
            .with_writer(file);
        registry.with(file_layer).init();
    }
    Ok(())
}

/// Initialize pretty format logging
fn init_pretty_logging<S>(registry: S, config: &Config) -> Result<()>
where
    S: tracing::Subscriber
        + Send
        + Sync
        + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    if config.logging.stdout {
        let stdout_layer = tracing_subscriber::fmt::layer()
            .pretty()
            .with_target(true)
            .with_thread_ids(true)
            .with_writer(io::stdout);

        if let Some(ref log_file) = config.logging.file {
            let file = create_log_file(log_file)?;
            let file_layer = tracing_subscriber::fmt::layer()
                .pretty()
                .with_target(true)
                .with_thread_ids(true)
                .with_writer(file);
            registry.with(stdout_layer).with(file_layer).init();
        } else {
            registry.with(stdout_layer).init();
        }
    } else if let Some(ref log_file) = config.logging.file {
        let file = create_log_file(log_file)?;
        let file_layer = tracing_subscriber::fmt::layer()
            .pretty()
            .with_target(true)
            .with_thread_ids(true)
            .with_writer(file);
        registry.with(file_layer).init();
    }
    Ok(())
}

/// Initialize compact format logging
fn init_compact_logging<S>(registry: S, config: &Config) -> Result<()>
where
    S: tracing::Subscriber
        + Send
        + Sync
        + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    if config.logging.stdout {
        let stdout_layer = tracing_subscriber::fmt::layer()
            .compact()
            .with_target(false)
            .with_thread_ids(false)
            .with_writer(io::stdout);

        if let Some(ref log_file) = config.logging.file {
            let file = create_log_file(log_file)?;
            let file_layer = tracing_subscriber::fmt::layer()
                .compact()
                .with_target(false)
                .with_thread_ids(false)
                .with_writer(file);
            registry.with(stdout_layer).with(file_layer).init();
        } else {
            registry.with(stdout_layer).init();
        }
    } else if let Some(ref log_file) = config.logging.file {
        let file = create_log_file(log_file)?;
        let file_layer = tracing_subscriber::fmt::layer()
            .compact()
            .with_target(false)
            .with_thread_ids(false)
            .with_writer(file);
        registry.with(file_layer).init();
    }
    Ok(())
}

/// Create a log file with parent directories
fn create_log_file<P: AsRef<Path>>(path: P) -> Result<std::fs::File> {
    let path = path.as_ref();

    // Ensure parent directories exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            crate::error::MantaError::Internal(format!(
                "Failed to open log file {}: {}",
                path.display(),
                e
            ))
        })?;

    Ok(file)
}

/// Parse a log level string into a tracing Level
fn parse_log_level(level: &str) -> Level {
    match level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    }
}

/// Setup panic hook for better error messages
pub fn setup_panic_handler() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown location".to_string());

        tracing::error!(
            target: "panic",
            location = %location,
            "Application panicked: {}", message
        );

        eprintln!("\n❌ Application panicked at {}: {}", location, message);
    }));
}

/// Initialize logging for tests
#[cfg(test)]
pub fn init_test_logging() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_log_level() {
        assert_eq!(parse_log_level("trace"), Level::TRACE);
        assert_eq!(parse_log_level("debug"), Level::DEBUG);
        assert_eq!(parse_log_level("info"), Level::INFO);
        assert_eq!(parse_log_level("warn"), Level::WARN);
        assert_eq!(parse_log_level("error"), Level::ERROR);
        assert_eq!(parse_log_level("unknown"), Level::INFO);
    }
}
