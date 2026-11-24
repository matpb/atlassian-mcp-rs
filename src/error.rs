use std::fmt;

/// Client failure, split so tools can map caller mistakes and upstream failures to different JSON-RPC codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// Input rejected before any request; JSON-RPC -32602.
    Invalid(String),
    /// Transport, HTTP status or response decoding failure; JSON-RPC -32603.
    Upstream(String),
}

impl ClientError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn upstream(msg: impl Into<String>) -> Self {
        Self::Upstream(msg.into())
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m) | Self::Upstream(m) => m,
        }
    }

    /// Prefixes the message, keeping the variant ("Jira search/jql failed: {msg}").
    pub fn context(self, prefix: &str) -> Self {
        match self {
            Self::Invalid(m) => Self::Invalid(format!("{prefix}{m}")),
            Self::Upstream(m) => Self::Upstream(format!("{prefix}{m}")),
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ClientError {}

impl From<ClientError> for rmcp::ErrorData {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Invalid(m) => rmcp::ErrorData::invalid_params(m, None),
            ClientError::Upstream(m) => rmcp::ErrorData::internal_error(m, None),
        }
    }
}

pub type ClientResult<T> = Result<T, ClientError>;

/// Rejects a caller value outside `lo..=hi` before any request; the message names the argument.
pub fn check_range(name: &str, value: u64, lo: u64, hi: u64) -> ClientResult<()> {
    if (lo..=hi).contains(&value) {
        return Ok(());
    }
    Err(ClientError::invalid(format!(
        "{name} must be between {lo} and {hi}, got {value}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;

    #[test]
    fn invalid_maps_to_32602() {
        let e = rmcp::ErrorData::from(ClientError::invalid("issue_key must not be empty"));
        assert_eq!(e.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(e.code.0, -32602);
        assert_eq!(e.message, "issue_key must not be empty");
        assert!(e.data.is_none());
    }

    #[test]
    fn upstream_maps_to_32603() {
        let e = rmcp::ErrorData::from(ClientError::upstream(
            "Jira returned 500 Internal Server Error: boom",
        ));
        assert_eq!(e.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(e.code.0, -32603);
        assert_eq!(e.message, "Jira returned 500 Internal Server Error: boom");
        assert!(e.data.is_none());
    }

    #[test]
    fn context_keeps_variant_and_prefixes() {
        assert_eq!(
            ClientError::invalid("jql must not be empty").context("Jira search/jql failed: "),
            ClientError::Invalid("Jira search/jql failed: jql must not be empty".to_string())
        );
        assert_eq!(
            ClientError::upstream("request failed").context("Jira search/jql failed: "),
            ClientError::Upstream("Jira search/jql failed: request failed".to_string())
        );
    }

    #[test]
    fn check_range_accepts_bounds() {
        assert_eq!(check_range("max_results", 1, 1, 100), Ok(()));
        assert_eq!(check_range("max_results", 100, 1, 100), Ok(()));
        assert_eq!(check_range("ttl_seconds", 3600, 1, 3600), Ok(()));
    }

    #[test]
    fn check_range_rejects_outside() {
        assert_eq!(
            check_range("max_results", 0, 1, 100),
            Err(ClientError::invalid(
                "max_results must be between 1 and 100, got 0"
            ))
        );
        assert_eq!(
            check_range("max_results", 101, 1, 100),
            Err(ClientError::invalid(
                "max_results must be between 1 and 100, got 101"
            ))
        );
        assert_eq!(
            check_range("ttl_seconds", u64::MAX, 1, 3600),
            Err(ClientError::invalid(format!(
                "ttl_seconds must be between 1 and 3600, got {}",
                u64::MAX
            )))
        );
    }

    #[test]
    fn display_is_the_message() {
        assert_eq!(ClientError::invalid("a b").to_string(), "a b");
        assert_eq!(ClientError::upstream("c: d").to_string(), "c: d");
        assert_eq!(ClientError::upstream("c").message(), "c");
    }
}
