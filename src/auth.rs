//! Connecting to the server, permission checks, and the `login` / `logout` commands.

use std::fs;
use std::path::Path;

use crate::api::{self, ApiError, Client};
use crate::cli::GlobalOptions;
use crate::config::{self, AuthConfig};
use crate::error::{Error, Result, node_io_error};
use crate::output::s;
use crate::url;

/// A connection to the server plus the (possibly discovered) API URL.
pub struct Connection {
    pub client: Client,
    pub url: String,
}

/// Discovers the API URL through `.well-known/immich`, then verifies the key with `GET /users/me`.
pub fn connect(raw_url: &str, key: &str) -> Result<Connection> {
    let well_known = url::resolve(raw_url, ".well-known/immich")
        .map_err(|error| Error::Fatal(format!("Failed to connect to server {raw_url} - {error}")))?;
    let mut url = raw_url.to_string();
    if let Some(endpoint) = api::discover_endpoint(&well_known)
        && let Ok(endpoint) = url::resolve(&url, &endpoint)
    {
        if endpoint != url {
            println!("Discovered API at {endpoint}");
        }
        url = endpoint;
    }

    let client = Client::new(&url, key);
    if let Err(error) = client.get_my_user() {
        api::log_error(&error, &format!("Failed to connect to server {url}"));
        return Err(Error::Exit(1));
    }
    Ok(Connection { client, url })
}

/// Uses `--url`/`--key` when both are given, otherwise the auth file (rewriting it when the
/// API URL was discovered to be different).
pub fn authenticate(options: &GlobalOptions) -> Result<Connection> {
    if let (Some(url), Some(key)) = (&options.url, &options.key) {
        return connect(url, key);
    }
    let auth = config::read_auth_file(&options.config_directory)?;
    let connection = connect(&auth.url, &auth.key)?;
    if connection.url != auth.url {
        let updated = AuthConfig {
            url: connection.url.clone(),
            key: auth.key,
        };
        config::write_auth_file(&options.config_directory, &updated).map_err(|error| {
            Error::Fatal(node_io_error(
                &error,
                "open",
                &config::auth_file_path(&options.config_directory),
            ))
        })?;
    }
    Ok(connection)
}

/// Exits with the original message when the API key lacks any of `permissions`.
pub fn require_permissions(client: &Client, permissions: &[&str]) -> Result<()> {
    let api_key = client
        .get_my_api_key()
        .map_err(|error| fatal_api_error(&error, "Failed to load API key"))?;
    if api_key.permissions.iter().any(|permission| permission == "all") {
        return Ok(());
    }
    let missing: Vec<&str> = permissions
        .iter()
        .copied()
        .filter(|wanted| !api_key.permissions.iter().any(|have| have == wanted))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let combined: Vec<String> = missing.iter().map(|permission| format!("\"{permission}\"")).collect();
    println!(
        "Missing required permission{}: {}.\nPlease make sure your API key has the correct permissions.",
        s(missing.len()),
        combined.join(", ")
    );
    Err(Error::Exit(1))
}

/// Logs an API failure the original would have crashed on, and stops the command.
pub fn fatal_api_error(error: &ApiError, message: &str) -> Error {
    api::log_error(error, message);
    Error::Exit(1)
}

pub fn login(url: &str, key: &str, options: &GlobalOptions) -> Result<()> {
    println!("Logging in to {url}");
    let config_dir = &options.config_directory;
    let connection = connect(url, key)?;
    require_permissions(&connection.client, &["user.read"])?;
    let user = match connection.client.get_my_user() {
        Ok(user) => user,
        Err(error) => {
            api::log_error(&error, "Failed to load user info");
            return Err(Error::Exit(1));
        }
    };
    println!("Logged in as {}", user.email);

    if !Path::new(config_dir).exists() && fs::create_dir_all(config_dir).is_err() {
        println!("Failed to create config folder: {config_dir}");
        return Ok(());
    }
    let auth = AuthConfig {
        url: url.to_string(),
        key: key.to_string(),
    };
    let auth_file = config::auth_file_path(config_dir);
    config::write_auth_file(config_dir, &auth)
        .map_err(|error| Error::Fatal(node_io_error(&error, "open", &auth_file)))?;
    println!("Wrote auth info to {auth_file}");
    Ok(())
}

pub fn logout(options: &GlobalOptions) -> Result<()> {
    println!("Logging out...");
    let auth_file = config::auth_file_path(&options.config_directory);
    if Path::new(&auth_file).exists() {
        fs::remove_file(&auth_file).map_err(|error| Error::Fatal(node_io_error(&error, "unlink", &auth_file)))?;
        println!("Removed auth file: {auth_file}");
    }
    println!("Successfully logged out");
    Ok(())
}
