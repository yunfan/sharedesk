use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

const DEFAULT_SERVER_LISTEN: &str = "0.0.0.0:38080";
const DEFAULT_SERVER_ORIGINANY: bool = false;
const DEFAULT_SERVER_MAXONLINE: usize = 200;
const DEFAULT_WEB_PUBLICBASE: &str = "";
const DEFAULT_WEB_STATICDIR: &str = "../client/dist";
const DEFAULT_WEB_BACKENDBASE: &str = "/backend";
const DEFAULT_ICE_STUNURLS: &str = "stun:stun.l.google.com:19302";
const DEFAULT_TURN_MODE: &str = "disabled";
const DEFAULT_TURN_URLS: &str = "";
const DEFAULT_TURN_USERNAME: &str = "";
const DEFAULT_TURN_PASSWORD: &str = "";
const DEFAULT_TURN_SECRET: &str = "";
const DEFAULT_TURN_TTLSECONDS: u64 = 3600;
const DEFAULT_LOG_FILTER: &str = "room_server=info,tower_http=info";

#[derive(Clone, Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub web: WebConfig,
    pub ice: IceConfig,
    pub turn: TurnConfig,
    pub log: LogConfig,
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub originany: bool,
    pub maxonline: usize,
}

#[derive(Clone, Debug)]
pub struct WebConfig {
    pub publicbase: Option<String>,
    pub staticdir: PathBuf,
    pub backendbase: String,
}

#[derive(Clone, Debug)]
pub struct IceConfig {
    pub stunurls: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum TurnConfig {
    Disabled,
    Static {
        urls: Vec<String>,
        username: String,
        credential: String,
    },
    Coturnrest {
        urls: Vec<String>,
        secret: String,
        ttlseconds: u64,
    },
}

#[derive(Clone, Debug)]
pub struct LogConfig {
    pub filter: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            server: ServerConfig {
                listen: env_string("ROOM_SERVER_LISTEN", DEFAULT_SERVER_LISTEN)
                    .parse()
                    .context("ROOM_SERVER_LISTEN must be a valid socket address")?,
                originany: env_bool("ROOM_SERVER_ORIGINANY", DEFAULT_SERVER_ORIGINANY),
                maxonline: env_usize("ROOM_SERVER_MAXONLINE", DEFAULT_SERVER_MAXONLINE)?,
            },
            web: WebConfig {
                publicbase: option_string("ROOM_WEB_PUBLICBASE", DEFAULT_WEB_PUBLICBASE),
                staticdir: PathBuf::from(env_string("ROOM_WEB_STATICDIR", DEFAULT_WEB_STATICDIR)),
                backendbase: normalize_path(env_string(
                    "ROOM_WEB_BACKENDBASE",
                    DEFAULT_WEB_BACKENDBASE,
                )),
            },
            ice: IceConfig {
                stunurls: csv_string("ROOM_ICE_STUNURLS", DEFAULT_ICE_STUNURLS),
            },
            turn: parse_turn()?,
            log: LogConfig {
                filter: env_string("ROOM_LOG_FILTER", DEFAULT_LOG_FILTER),
            },
        })
    }

    pub fn ice_servers(&self, room: &str, role: &str) -> Vec<IceServer> {
        let mut servers = Vec::new();

        if !self.ice.stunurls.is_empty() {
            servers.push(IceServer {
                urls: self.ice.stunurls.clone(),
                username: None,
                credential: None,
            });
        }

        match &self.turn {
            TurnConfig::Disabled => {}
            TurnConfig::Static {
                urls,
                username,
                credential,
            } => servers.push(IceServer {
                urls: urls.clone(),
                username: Some(username.clone()),
                credential: Some(credential.clone()),
            }),
            TurnConfig::Coturnrest {
                urls,
                secret,
                ttlseconds,
            } => {
                let expires_at = unix_now() + *ttlseconds;
                let username = format!("{expires_at}:{room}:{role}");

                let mut mac =
                    HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
                mac.update(username.as_bytes());
                let digest = mac.finalize().into_bytes();
                let credential = base64::engine::general_purpose::STANDARD.encode(digest);

                servers.push(IceServer {
                    urls: urls.clone(),
                    username: Some(username),
                    credential: Some(credential),
                });
            }
        }

        servers
    }
}

fn parse_turn() -> Result<TurnConfig> {
    let mode = env_string("ROOM_TURN_MODE", DEFAULT_TURN_MODE);

    match mode.as_str() {
        "" | "disabled" => Ok(TurnConfig::Disabled),
        "static" => Ok(TurnConfig::Static {
            urls: csv_required("ROOM_TURN_URLS", DEFAULT_TURN_URLS)?,
            username: env_string("ROOM_TURN_USERNAME", DEFAULT_TURN_USERNAME),
            credential: env_string("ROOM_TURN_PASSWORD", DEFAULT_TURN_PASSWORD),
        }),
        "coturnrest" => Ok(TurnConfig::Coturnrest {
            urls: csv_required("ROOM_TURN_URLS", DEFAULT_TURN_URLS)?,
            secret: env_string("ROOM_TURN_SECRET", DEFAULT_TURN_SECRET),
            ttlseconds: env_u64("ROOM_TURN_TTLSECONDS", DEFAULT_TURN_TTLSECONDS)?,
        }),
        other => anyhow::bail!("unsupported ROOM_TURN_MODE: {other}"),
    }
}

fn env_string(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn option_string(name: &str, default: &str) -> Option<String> {
    let value = env_string(name, default);
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn csv_string(name: &str, default: &str) -> Vec<String> {
    env_string(name, default)
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn csv_required(name: &str, default: &str) -> Result<Vec<String>> {
    let values = csv_string(name, default);
    if values.is_empty() {
        anyhow::bail!("{name} is required");
    }
    Ok(values)
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn env_usize(name: &str, default: usize) -> Result<usize> {
    match env::var(name) {
        Ok(value) => value
            .trim()
            .parse()
            .with_context(|| format!("{name} must be a valid positive integer")),
        Err(_) => Ok(default),
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64> {
    match env::var(name) {
        Ok(value) => value
            .trim()
            .parse()
            .with_context(|| format!("{name} must be a valid positive integer")),
        Err(_) => Ok(default),
    }
}

fn normalize_path(input: String) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        "/backend".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.trim_end_matches('/').to_string()
    } else {
        format!("/{}", trimmed.trim_end_matches('/'))
    }
}

fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after epoch")
        .as_secs()
}
