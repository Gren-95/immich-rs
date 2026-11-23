//! The Immich API client: the handful of endpoints the CLI uses, with the same request shapes
//! and error classification as the generated TypeScript SDK.

use std::fmt;
use std::fs::Metadata;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ureq::Agent;
use ureq::http::Response;

use crate::date;
use crate::multipart::{self, Multipart};
use crate::output;
use crate::pathutil;
use crate::url::join_api;

const USER_AGENT: &str = concat!("immich-rs/", env!("CARGO_PKG_VERSION"));
/// Response bodies (album lists can be large) are read with this cap.
const MAX_RESPONSE_BYTES: u64 = 1 << 30;

#[derive(Debug)]
pub enum ApiError {
    /// A non-2xx response with its raw body (`is_json` when the server said it was JSON).
    Http { status: u16, body: String, is_json: bool },
    /// The response was not JSON, e.g. a reverse proxy or SSO page answered instead.
    Malformed(String),
    /// The request never got a response.
    Transport(String),
    /// The response body could not be parsed into the expected shape.
    Parse(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Http { status, .. } => write!(f, "Error: {status}"),
            ApiError::Malformed(message) => write!(f, "MalformedResponseError: {message}"),
            ApiError::Transport(cause) => write!(f, "TypeError: fetch failed ({cause})"),
            ApiError::Parse(message) => write!(f, "SyntaxError: {message}"),
        }
    }
}

/// `JSON.stringify(error.data, undefined, 2)`: the parsed JSON body with its key order kept,
/// or the text body as a JSON string.
fn error_data_json(body: &str, is_json: bool) -> String {
    if !is_json {
        return serde_json::to_string(body).unwrap_or_default();
    }
    if body.trim().is_empty() {
        return "null".to_string();
    }
    output::pretty_json(body, "  ")
}

/// Prints an API error under `message`, exactly like the original `logError`.
pub fn log_error(error: &ApiError, message: &str) {
    match error {
        ApiError::Http { status, body, is_json } => {
            eprintln!("{message}: {status}");
            eprintln!("{}", error_data_json(body, *is_json));
        }
        ApiError::Malformed(details) => {
            eprintln!("{message}: {details}");
            eprintln!(
                "Check that the URL points at the Immich API, and that nothing in front of it \
                 (reverse proxy, SSO portal) is answering instead."
            );
        }
        other => eprintln!("{message} - {other}"),
    }
}

#[derive(Debug, Deserialize)]
pub struct UserResponse {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ApiKeyResponse {
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ServerVersion {
    pub major: Value,
    pub minor: Value,
    pub patch: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaTypes {
    #[serde(default)]
    pub image: Vec<String>,
    #[serde(default)]
    pub video: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct AssetStatistics {
    pub images: Value,
    pub videos: Value,
    pub total: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub album_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkCheckItem {
    pub id: String,
    pub checksum: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkCheckResult {
    pub id: String,
    pub action: String,
    pub asset_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BulkCheckResponse {
    results: Vec<BulkCheckResult>,
}

#[derive(Debug, Deserialize)]
pub struct UploadResponse {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct WellKnown {
    api: WellKnownApi,
}

#[derive(Debug, Deserialize)]
struct WellKnownApi {
    endpoint: String,
}

fn build_agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(300)))
        .timeout_recv_body(Some(Duration::from_secs(300)))
        .build()
        .new_agent()
}

fn header_text(response: &Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn read_body(response: &mut Response<ureq::Body>) -> Result<String, ApiError> {
    response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .lossy_utf8(true)
        .read_to_string()
        .map_err(|error| ApiError::Transport(error.to_string()))
}

/// Looks up `.well-known/immich` and returns the advertised API endpoint, if any.
pub fn discover_endpoint(well_known_url: &str) -> Option<String> {
    let agent = build_agent();
    let mut response = agent.get(well_known_url).call().ok()?;
    let body = read_body(&mut response).ok()?;
    let well_known: WellKnown = serde_json::from_str(&body).ok()?;
    Some(well_known.api.endpoint)
}

#[derive(Clone)]
pub struct Client {
    agent: Agent,
    base_url: String,
    key: String,
}

impl Client {
    pub fn new(base_url: &str, key: &str) -> Self {
        Client {
            agent: build_agent(),
            base_url: base_url.to_string(),
            key: key.to_string(),
        }
    }

    /// Sends a JSON request and returns the JSON response body, mirroring the SDK's
    /// `fetchJson` + `ok` + JSON-only checks.
    fn request(&self, method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Value, ApiError> {
        let url = join_api(&self.base_url, path);
        let result = match (method, body) {
            ("GET", _) => self
                .agent
                .get(&url)
                .header("x-api-key", &self.key)
                .header("accept", "application/json")
                .call(),
            (method, body) => {
                let builder = match method {
                    "PUT" => self.agent.put(&url),
                    _ => self.agent.post(&url),
                };
                builder
                    .header("x-api-key", &self.key)
                    .header("accept", "application/json")
                    .header("content-type", "application/json")
                    .send(body.unwrap_or_default())
            }
        };
        let mut response = result.map_err(|error| ApiError::Transport(error.to_string()))?;
        let status = response.status().as_u16();
        let content_type = header_text(&response, "content-type");
        let is_json = content_type.as_deref().is_some_and(|value| value.contains("json"));
        if status != 204 && !is_json {
            return Err(ApiError::Malformed(format!(
                "Expected a JSON response ({url}, HTTP {status}, content-type: {})",
                content_type.as_deref().unwrap_or("none")
            )));
        }
        let text = read_body(&mut response)?;
        if !matches!(status, 200 | 201 | 202 | 204) {
            return Err(ApiError::Http {
                status,
                body: text,
                is_json,
            });
        }
        if !is_json {
            return Ok(Value::String(text));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|error| ApiError::Parse(error.to_string()))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let value = self.request("GET", path, None)?;
        serde_json::from_value(value).map_err(|error| ApiError::Parse(error.to_string()))
    }

    fn send<B: Serialize, T: DeserializeOwned>(&self, method: &str, path: &str, body: &B) -> Result<T, ApiError> {
        let bytes = serde_json::to_vec(body).map_err(|error| ApiError::Parse(error.to_string()))?;
        let value = self.request(method, path, Some(bytes))?;
        serde_json::from_value(value).map_err(|error| ApiError::Parse(error.to_string()))
    }

    pub fn get_my_user(&self) -> Result<UserResponse, ApiError> {
        self.get("/users/me")
    }

    pub fn get_my_api_key(&self) -> Result<ApiKeyResponse, ApiError> {
        self.get("/api-keys/me")
    }

    pub fn get_server_version(&self) -> Result<ServerVersion, ApiError> {
        self.get("/server/version")
    }

    pub fn get_supported_media_types(&self) -> Result<MediaTypes, ApiError> {
        self.get("/server/media-types")
    }

    pub fn get_asset_statistics(&self) -> Result<AssetStatistics, ApiError> {
        self.get("/assets/statistics")
    }

    pub fn get_all_albums(&self) -> Result<Vec<Album>, ApiError> {
        self.get("/albums")
    }

    pub fn create_album(&self, album_name: &str) -> Result<Album, ApiError> {
        self.send("POST", "/albums", &serde_json::json!({ "albumName": album_name }))
    }

    pub fn add_assets_to_album(&self, album_id: &str, ids: &[String]) -> Result<(), ApiError> {
        let path = format!("/albums/{}/assets", encode_uri_component(album_id));
        self.send::<_, Value>("PUT", &path, &serde_json::json!({ "ids": ids }))
            .map(|_| ())
    }

    pub fn check_bulk_upload(&self, assets: &[BulkCheckItem]) -> Result<Vec<BulkCheckResult>, ApiError> {
        let response: BulkCheckResponse = self.send(
            "POST",
            "/assets/bulk-upload-check",
            &serde_json::json!({ "assets": assets }),
        )?;
        Ok(response.results)
    }

    /// Uploads one asset (`POST /assets`, multipart). Errors are returned as the task error
    /// text the original prints (`Error: <response body>`).
    pub fn upload_asset(
        &self,
        filepath: &str,
        metadata: &Metadata,
        visibility: Option<&str>,
    ) -> Result<UploadResponse, String> {
        let modified = date::mtime_iso(metadata);
        let mut form = Multipart::new(multipart::random_boundary());
        form.text("fileCreatedAt", &modified);
        form.text("fileModifiedAt", &modified);
        form.text("fileSize", &metadata.len().to_string());
        form.text("isFavorite", "false");
        form.file(
            "assetData",
            &pathutil::basename(filepath),
            filepath.into(),
            metadata.len(),
        );
        if let Some(visibility) = visibility {
            form.text("visibility", visibility);
        }
        if let Some(sidecar) = find_sidecar(filepath)
            && let Ok(sidecar_metadata) = std::fs::metadata(&sidecar)
        {
            form.file(
                "sidecarData",
                &pathutil::basename(&sidecar),
                sidecar.clone().into(),
                sidecar_metadata.len(),
            );
        }

        let content_type = form.content_type();
        let content_length = form.content_length();
        let reader = form.into_reader().map_err(|error| format!("Error: {error}"))?;
        let url = format!("{}/assets", self.base_url.trim_end_matches('/'));
        let mut response = self
            .agent
            .post(&url)
            .config()
            .max_redirects(0)
            .build()
            .header("x-api-key", &self.key)
            .header("content-type", content_type)
            .header("content-length", content_length.to_string())
            .send(ureq::SendBody::from_owned_reader(reader))
            .map_err(|error| format!("TypeError: fetch failed ({error})"))?;
        let status = response.status().as_u16();
        let text = read_body(&mut response).map_err(|error| error.to_string())?;
        if status != 200 && status != 201 {
            return Err(format!("Error: {text}"));
        }
        serde_json::from_str(&text).map_err(|error| format!("SyntaxError: {error}"))
    }
}

/// `encodeURIComponent` for path parameters.
fn encode_uri_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

/// The XMP sidecar for an asset: `photo.jpg.xmp`, then `photo.xmp`.
pub fn find_sidecar(filepath: &str) -> Option<String> {
    let without_extension = pathutil::join(&pathutil::dirname(filepath), &pathutil::stem(filepath));
    [format!("{filepath}.xmp"), format!("{without_extension}.xmp")]
        .into_iter()
        .find(|candidate| std::path::Path::new(candidate).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_path_parameters() {
        assert_eq!(encode_uri_component("a b/c"), "a%20b%2Fc");
        assert_eq!(encode_uri_component("0f8f-uuid"), "0f8f-uuid");
    }

    #[test]
    fn finds_sidecars_in_the_original_order() {
        let dir = std::env::temp_dir().join(format!("immich-rs-sidecar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let base = dir.to_string_lossy().into_owned();
        let photo = format!("{base}/IMG_1.jpg");
        std::fs::write(&photo, b"x").expect("write photo");
        assert_eq!(find_sidecar(&photo), None);
        std::fs::write(format!("{base}/IMG_1.xmp"), b"x").expect("write sidecar");
        assert_eq!(find_sidecar(&photo), Some(format!("{base}/IMG_1.xmp")));
        std::fs::write(format!("{base}/IMG_1.jpg.xmp"), b"x").expect("write sidecar");
        assert_eq!(find_sidecar(&photo), Some(format!("{base}/IMG_1.jpg.xmp")));
        std::fs::remove_dir_all(&dir).expect("remove temp dir");
    }

    #[test]
    fn formats_http_error_data_like_json_stringify() {
        let body = r#"{"message":"Invalid API key","error":"Unauthorized","statusCode":401}"#;
        let error = ApiError::Http {
            status: 401,
            body: body.to_string(),
            is_json: true,
        };
        assert_eq!(error.to_string(), "Error: 401");
        assert_eq!(
            error_data_json(body, true),
            "{\n  \"message\": \"Invalid API key\",\n  \"error\": \"Unauthorized\",\n  \"statusCode\": 401\n}"
        );
        assert_eq!(error_data_json("", true), "null");
        assert_eq!(error_data_json("Bad Gateway", false), "\"Bad Gateway\"");
    }
}
