use std::env;
use std::fmt;

#[derive(Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Origin agents reach this server at, used to build attachment upload URLs.
    pub public_url: Option<String>,
    pub max_upload_bytes: usize,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("public_url", &self.public_url)
            .field("max_upload_bytes", &self.max_upload_bytes)
            .finish()
    }
}

impl ServerConfig {
    pub fn from_env() -> Self {
        from_env_values(
            env::var("MCP_HOST").ok(),
            env::var("MCP_PORT").ok(),
            env::var("MCP_PUBLIC_URL").ok(),
            env::var("MCP_MAX_UPLOAD_BYTES").ok(),
        )
    }
}

fn from_env_values(
    host: Option<String>,
    port: Option<String>,
    public_url: Option<String>,
    max_upload: Option<String>,
) -> ServerConfig {
    ServerConfig {
        host: host.unwrap_or_else(|| "0.0.0.0".into()),
        port: port.as_deref().and_then(|p| p.parse().ok()).unwrap_or(8432),
        public_url: public_url
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty()),
        max_upload_bytes: max_upload
            .as_deref()
            .and_then(|p| p.parse().ok())
            .unwrap_or(crate::upload::DEFAULT_MAX_UPLOAD_BYTES),
    }
}

#[cfg(test)]
fn from_host_port(host: Option<String>, port: Option<String>) -> ServerConfig {
    from_env_values(host, port, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_vars_missing() {
        let c = from_host_port(None, None);
        assert_eq!(c.host, "0.0.0.0");
        assert_eq!(c.port, 8432);
    }

    #[test]
    fn parses_host_and_port() {
        let c = from_host_port(Some("127.0.0.1".into()), Some("9000".into()));
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 9000);
    }

    #[test]
    fn invalid_port_falls_back_to_default() {
        let c = from_host_port(None, Some("not-a-port".into()));
        assert_eq!(c.port, 8432);
    }

    #[test]
    fn upload_settings() {
        let c = from_env_values(
            None,
            None,
            Some(" https://mcp.example.com ".into()),
            Some("1024".into()),
        );
        assert_eq!(c.public_url.as_deref(), Some("https://mcp.example.com"));
        assert_eq!(c.max_upload_bytes, 1024);
        let d = from_env_values(None, None, Some("".into()), Some("nope".into()));
        assert!(d.public_url.is_none());
        assert_eq!(d.max_upload_bytes, crate::upload::DEFAULT_MAX_UPLOAD_BYTES);
    }
}
