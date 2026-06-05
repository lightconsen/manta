//! Example: test macOS desktop tools on the current host.

use syscity::capabilities::{CapabilityRegistry, CapabilitySet, ToolConflictStrategy};
use syscity::tools::{ToolContext, ToolRegistry};

#[tokio::main]
async fn main() {
    println!("=== macOS Capability Set Test ===\n");

    let macos_set = syscity::capabilities::MacosSet::new();
    println!("Set ID:       {}", macos_set.id());
    println!("Name:         {}", macos_set.name());
    println!("Description:  {}", macos_set.description());
    println!("Available:    {}", macos_set.is_available());
    println!("Tools count:  {}", macos_set.tools().len());
    println!();

    for tool in macos_set.tools() {
        println!("  - {}: {}", tool.name(), tool.description());
    }
    println!();

    let mut cap_reg = CapabilityRegistry::new();
    cap_reg.register(Box::new(macos_set));

    let mut tool_reg = ToolRegistry::new();
    cap_reg.export_to_tool_registry(&mut tool_reg, ToolConflictStrategy::Reject);

    let context = ToolContext::default();

    // Test 1: Screenshot
    println!("--- Test: macos_screenshot ---");
    match tool_reg
        .execute("macos_screenshot", serde_json::json!({}), &context)
        .await
    {
        Some(Ok(result)) => {
            if result.success {
                let preview: String = result.output.chars().take(120).collect();
                println!("OK: {}", preview);
            } else {
                println!("FAIL: {}", result.error.unwrap_or_default());
            }
        }
        Some(Err(e)) => println!("ERROR: {}", e),
        None => println!("NOT FOUND: tool not registered"),
    }
    println!();

    // Test 2: Accessibility (frontmost app)
    println!("--- Test: macos_accessibility (frontmost) ---");
    match tool_reg
        .execute("macos_accessibility", serde_json::json!({}), &context)
        .await
    {
        Some(Ok(result)) => {
            if result.success {
                let preview: String = result.output.chars().take(500).collect();
                println!("OK (first 500 chars):\n{}", preview);
            } else {
                println!("FAIL: {}", result.error.unwrap_or_default());
            }
        }
        Some(Err(e)) => println!("ERROR: {}", e),
        None => println!("NOT FOUND: tool not registered"),
    }
    println!();

    // Test 3: AppleScript (simple)
    println!("--- Test: applescript ---");
    match tool_reg
        .execute(
            "applescript",
            serde_json::json!({
                "script": "return \"Hello from Syscity macOS tools\""
            }),
            &context,
        )
        .await
    {
        Some(Ok(result)) => {
            if result.success {
                println!("OK: {}", result.output);
            } else {
                println!("FAIL: {}", result.error.unwrap_or_default());
            }
        }
        Some(Err(e)) => println!("ERROR: {}", e),
        None => println!("NOT FOUND: tool not registered"),
    }
    println!();

    // Test 4: Desktop control (inspect)
    println!("--- Test: macos_desktop_control (inspect) ---");
    match tool_reg
        .execute(
            "macos_desktop_control",
            serde_json::json!({
                "action": "inspect",
                "mode": "hybrid"
            }),
            &context,
        )
        .await
    {
        Some(Ok(result)) => {
            if result.success {
                let preview: String = result.output.chars().take(800).collect();
                println!("OK (first 800 chars):\n{}", preview);
            } else {
                println!("FAIL: {}", result.error.unwrap_or_default());
            }
        }
        Some(Err(e)) => println!("ERROR: {}", e),
        None => println!("NOT FOUND: tool not registered"),
    }

    println!("\n=== Done ===");
}
