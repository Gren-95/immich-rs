//! The `server-info` command.

use std::thread;

use serde_json::Value;

use crate::api::ApiError;
use crate::auth::{authenticate, fatal_api_error, require_permissions};
use crate::cli::GlobalOptions;
use crate::error::Result;

type ApiResult<T> = std::result::Result<T, ApiError>;

/// Renders a JSON value the way a JavaScript template literal would.
fn js_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(js_text).collect::<Vec<_>>().join(","),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Media types without their first `.` and comma-joined, as `${array.map(...)}` prints them.
fn extension_list(extensions: &[String]) -> String {
    extensions
        .iter()
        .map(|extension| extension.replacen('.', "", 1))
        .collect::<Vec<_>>()
        .join(",")
}

fn panicked<T>() -> ApiResult<T> {
    Err(ApiError::Transport("request thread panicked".to_string()))
}

pub fn server_info(options: &GlobalOptions) -> Result<()> {
    let connection = authenticate(options)?;
    let client = &connection.client;
    require_permissions(client, &["server.about", "asset.statistics", "user.read"])?;

    // The original fires these four requests concurrently (`Promise.all`).
    let (version, media_types, stats, user) = thread::scope(|scope| {
        let version = scope.spawn(|| client.get_server_version());
        let media_types = scope.spawn(|| client.get_supported_media_types());
        let stats = scope.spawn(|| client.get_asset_statistics());
        let user = client.get_my_user();
        (
            version.join().unwrap_or_else(|_| panicked()),
            media_types.join().unwrap_or_else(|_| panicked()),
            stats.join().unwrap_or_else(|_| panicked()),
            user,
        )
    });
    let message = "Failed to load server info";
    let version = version.map_err(|error| fatal_api_error(&error, message))?;
    let media_types = media_types.map_err(|error| fatal_api_error(&error, message))?;
    let stats = stats.map_err(|error| fatal_api_error(&error, message))?;
    let user = user.map_err(|error| fatal_api_error(&error, message))?;

    println!("Server Info (via {})", user.email);
    println!("  Url: {}", connection.url);
    println!(
        "  Version: {}.{}.{}",
        js_text(&version.major),
        js_text(&version.minor),
        js_text(&version.patch)
    );
    println!("  Formats:");
    println!("    Images: {}", extension_list(&media_types.image));
    println!("    Videos: {}", extension_list(&media_types.video));
    println!("  Statistics:");
    println!("    Images: {}", js_text(&stats.images));
    println!("    Videos: {}", js_text(&stats.videos));
    println!("    Total: {}", js_text(&stats.total));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_extensions_like_the_original() {
        let extensions = vec![".jpg".to_string(), ".tar.gz".to_string(), "raw".to_string()];
        assert_eq!(extension_list(&extensions), "jpg,tar.gz,raw");
    }

    #[test]
    fn renders_values_like_template_literals() {
        assert_eq!(js_text(&serde_json::json!(42)), "42");
        assert_eq!(js_text(&serde_json::json!("x")), "x");
        assert_eq!(js_text(&serde_json::json!(["a", 1])), "a,1");
    }
}
