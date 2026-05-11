use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::str::FromStr;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;
use turn_server::config::{Auth, Config, Interface, Log, LogLevel, Server};
use turn_server::service::session::ports::PortRange;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = env::var("ROOM_LOG_FILTER").unwrap_or_else(|_| "room_turn=info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .init();

    let config = build_config()?;
    turn_server::start_server(config).await
}

fn build_config() -> Result<Config> {
    let realm = env_string("ROOM_TURN_REALM", "sharedesk");
    let listen = env_socket("ROOM_TURN_LISTEN", "0.0.0.0:3478")?;
    let external = env_socket(
        "ROOM_TURN_EXTERNAL",
        &env_string("ROOM_TURN_LISTEN", "0.0.0.0:3478"),
    )?;
    let port_range = PortRange::from_str(&env_string("ROOM_TURN_PORTRANGE", "49152..49200"))
        .context("ROOM_TURN_PORTRANGE must look like 49152..49200")?;

    let mut static_credentials = HashMap::new();
    if let (Some(user), Some(password)) = (
        env::var("ROOM_TURN_USERNAME").ok().filter(|v| !v.is_empty()),
        env::var("ROOM_TURN_PASSWORD").ok().filter(|v| !v.is_empty()),
    ) {
        static_credentials.insert(user, password);
    }

    Ok(Config {
        server: Server {
            realm,
            port_range,
            max_threads: num_cpus::get(),
            interfaces: vec![
                Interface::Udp {
                    listen,
                    external,
                    idle_timeout: 20,
                    mtu: 1500,
                },
                Interface::Tcp {
                    listen,
                    external,
                    idle_timeout: 20,
                    ssl: None,
                },
            ],
        },
        api: None,
        prometheus: None,
        hooks: None,
        log: Log {
            level: LogLevel::Info,
            stdout: true,
            file_directory: None,
        },
        auth: Auth {
            static_credentials,
            static_auth_secret: env::var("ROOM_TURN_SECRET").ok().filter(|v| !v.is_empty()),
            enable_hooks_auth: false,
        },
    })
}

fn env_string(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_socket(name: &str, default: &str) -> Result<SocketAddr> {
    env_string(name, default)
        .parse()
        .with_context(|| format!("{name} must be a valid socket address"))
}
