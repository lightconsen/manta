//! Manta - Main entry point
//!
//! This is the main entry point for the Manta application.
//! It initializes the application and runs the CLI.

use manta::cli::Cli;

#[tokio::main]
async fn main() {
    // Initialize the application
    if let Err(e) = manta::init() {
        eprintln!("Failed to initialize: {}", e);
        std::process::exit(1);
    }

    // Run the CLI
    if let Err(e) = Cli::run().await {
        handle_error(e);
    }
}

/// Handle an error and exit the application
fn handle_error(error: manta::error::MantaError) {
    use manta::error::MantaError;

    let exit_code = match &error {
        MantaError::Config(_) => 2,
        MantaError::Validation(_) => 3,
        MantaError::NotFound { .. } => 4,
        MantaError::ExternalService { .. } => 5,
        _ => 1,
    };

    // Use different formatting based on the error type
    match &error {
        MantaError::Validation(msg) => {
            eprintln!("❌ Validation error: {}", msg);
        }
        MantaError::NotFound { resource } => {
            eprintln!("🔍 Not found: {}", resource);
        }
        MantaError::Config(e) => {
            eprintln!("⚙️  Configuration error: {}", e);
        }
        MantaError::Io(e) => {
            eprintln!("📁 I/O error: {}", e);
        }
        MantaError::Http(e) => {
            eprintln!("🌐 HTTP error: {}", e);
        }
        MantaError::ExternalService { source, .. } => {
            eprintln!("🔌 External service error: {}", source);
        }
        _ => {
            eprintln!("💥 Error: {}", error);
        }
    }

    // Add helpful hints for common errors
    if let MantaError::Config(_) = &error {
        eprintln!();
        eprintln!("Hint: Check your configuration file or environment variables.");
        eprintln!("      Run 'manta config' to see the current configuration.");
    }

    std::process::exit(exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_available() {
        assert!(!manta::VERSION.is_empty());
    }
}
