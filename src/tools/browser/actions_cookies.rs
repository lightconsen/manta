//! Cookie read/write/clear actions.

use super::BrowserAction;
use serde_json::{json, Value};

pub(super) async fn execute_cookies_actions(
    action: BrowserAction,
    page: &chromiumoxide::Page,
    _browser: Option<&chromiumoxide::Browser>,
    _screenshot_data: &mut Option<String>,
) -> Result<serde_json::Value, String> {
    match action {
        BrowserAction::GetCookies => {
            let script = r#"() => {
                    return document.cookie.split(';').map(c => {
                        const [name, ...rest] = c.trim().split('=');
                        return { name: name.trim(), value: rest.join('=') };
                    }).filter(c => c.name);
                }"#;
            match page.evaluate(script).await {
                Ok(result) => {
                    let cookies = result.into_value::<Vec<Value>>().unwrap_or_default();
                    Ok(json!({
                        "success": true,
                        "cookies": cookies,
                        "count": cookies.len()
                    }))
                }
                Err(e) => Err(format!("Failed to get cookies: {}", e)),
            }
        }

        BrowserAction::SetCookie { name, value, domain, path } => {
            let domain_part = domain
                .as_ref()
                .map(|d| format!("domain={};", d))
                .unwrap_or_default();
            let path_part = path
                .as_ref()
                .map(|p| format!("path={};", p))
                .unwrap_or_default();
            let cookie_str = format!("{}={};{}{}", name, value, domain_part, path_part);
            let script = format!(r#"() => {{ document.cookie = "{}"; return true; }}"#, cookie_str);
            match page.evaluate(script.as_str()).await {
                Ok(_) => Ok(json!({
                    "success": true,
                    "name": name,
                    "value": value
                })),
                Err(e) => Err(format!("Failed to set cookie: {}", e)),
            }
        }

        BrowserAction::ClearCookies => {
            let script = r#"() => {
                    document.cookie.split(';').forEach(c => {
                        const [name] = c.split('=');
                        document.cookie = name.trim() + '=; expires=Thu, 01 Jan 1970 00:00:00 UTC; path=/';
                    });
                    return document.cookie === '';
                }"#;
            match page.evaluate(script).await {
                Ok(result) => {
                    let cleared = result.into_value::<bool>().unwrap_or(false);
                    Ok(json!({ "success": true, "cleared": cleared }))
                }
                Err(e) => Err(format!("Failed to clear cookies: {}", e)),
            }
        }
        _ => Err("browser: action not handled by this group".to_string()),
    }
}
