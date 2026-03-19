use std::env;
use std::fmt;

#[derive(Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

impl ServerConfig {
    pub fn from_env() -> Self {
        from_host_port(env::var("MCP_HOST").ok(), env::var("MCP_PORT").ok())
    }
}

fn from_host_port(host: Option<String>, port: Option<String>) -> ServerConfig {
    ServerConfig {
        host: host.unwrap_or_else(|| "0.0.0.0".into()),
        port: port
            .as_deref()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8432),
    }
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
}
