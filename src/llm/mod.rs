//! OpenAI-compatible llama.cpp integration. No GTK objects or stored credentials.
mod client;
mod stream;
mod worker;
pub use worker::{Event, Job, start};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub host: String,
    pub port: u16,
    pub https: bool,
}
impl Default for Connection {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8080,
            https: false,
        }
    }
}
impl Connection {
    pub fn base_url(&self) -> Result<reqwest::Url> {
        ensure!(self.port != 0, "Port must be between 1 and 65535");
        let host = self.host.trim();
        ensure!(
            !host.is_empty()
                && !host.contains(['/', '@', '?', '#'])
                && !host.chars().any(char::is_whitespace),
            "Enter a host name or IP address, not a URL"
        );
        let host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        let url = reqwest::Url::parse(&format!(
            "{}://{}:{}",
            if self.https { "https" } else { "http" },
            host,
            self.port
        ))
        .context("Invalid server address")?;
        ensure!(
            url.host_str().is_some() && url.port_or_known_default() == Some(self.port),
            "Invalid server address"
        );
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addresses() {
        assert_eq!(
            Connection::default().base_url().unwrap().as_str(),
            "http://127.0.0.1:8080/"
        );
        for host in ["localhost", "192.168.1.2", "::1", "[::1]"] {
            assert!(
                Connection {
                    host: host.into(),
                    ..Default::default()
                }
                .base_url()
                .is_ok()
            );
        }
        for host in [
            "",
            "http://localhost",
            "user@localhost",
            "localhost/path",
            "localhost:90",
        ] {
            assert!(
                Connection {
                    host: host.into(),
                    ..Default::default()
                }
                .base_url()
                .is_err()
            );
        }
    }
}
