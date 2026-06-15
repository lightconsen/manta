//! Capability check command — show available OS capability sets and tools.

use crate::computer::capabilities::all_known_sets;
use crate::error::Result;

/// Print a formatted table of all capability sets and their availability.
pub async fn run_capability_check() -> Result<()> {
    println!("Syscity Capability Report");
    println!("=========================\n");

    // Host environment
    println!("Host Environment:");
    println!("  OS:       {}", std::env::consts::OS);
    println!("  Arch:     {}", std::env::consts::ARCH);
    println!();

    println!("Display Environment:");
    println!(
        "  DISPLAY:          {}",
        std::env::var("DISPLAY")
            .map(|v| format!("\"{}\"", v))
            .unwrap_or_else(|_| "not set".to_string())
    );
    println!(
        "  WAYLAND_DISPLAY:  {}",
        std::env::var("WAYLAND_DISPLAY")
            .map(|v| format!("\"{}\"", v))
            .unwrap_or_else(|_| "not set".to_string())
    );
    println!();

    // Capability sets
    let sets = all_known_sets();
    if sets.is_empty() {
        println!("No OS-specific capability sets compiled into this binary.");
        return Ok(());
    }

    println!("Capability Sets ({} total):", sets.len());
    println!();

    let mut available_count = 0;
    let mut unavailable_count = 0;

    for set in sets {
        let available = set.is_available();
        let status = if available {
            available_count += 1;
            "✓ available"
        } else {
            unavailable_count += 1;
            "✗ unavailable"
        };

        let scope_label = match set.scope() {
            crate::computer::capabilities::OsControlScope::ReadOnly => "read-only",
            crate::computer::capabilities::OsControlScope::UserSpace => "user-space",
            crate::computer::capabilities::OsControlScope::System => "system",
            crate::computer::capabilities::OsControlScope::Root => "root",
        };

        println!("  {} [{}] — {} (scope: {})", set.name(), set.id(), status, scope_label);
        println!("    {}", set.description());

        let tools = set.tools();
        if !tools.is_empty() {
            print!("    Tools: ");
            let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
            println!("{}", names.join(", "));
        }
        println!();
    }

    println!("Summary: {} available, {} unavailable", available_count, unavailable_count);
    println!();

    if unavailable_count > 0 {
        println!(
            "Note: Unavailable sets require a different OS or environment (e.g. GUI session)."
        );
    }

    Ok(())
}
