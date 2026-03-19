use http::HeaderMap;
use http::request::Parts;

/// Atlassian Cloud site + API token (HTTP Basic: email + token).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlassianCredentials {
    /// Jira + default Confluence wiki host, e.g. `https://your-company.atlassian.net`
    pub site_url: String,
    /// Base URL for Confluence Cloud (same as `site_url` unless overridden per request).
    pub confluence_site_url: String,
    pub email: String,
    pub api_token: String,
}

impl AtlassianCredentials {
    pub fn new(
        site_url: String,
        email: String,
        api_token: String,
        confluence_site_url: String,
    ) -> Self {
        Self {
            site_url: site_url.trim_end_matches('/').to_string(),
            confluence_site_url: confluence_site_url.trim_end_matches('/').to_string(),
            email,
            api_token,
        }
    }

    pub fn jira_rest_v3_root(&self) -> String {
        format!("{}/rest/api/3", self.site_url)
    }

    pub fn confluence_rest_root(&self) -> String {
        format!("{}/wiki/rest/api", self.confluence_site_url)
    }
}

/// Bitbucket Cloud REST API 2.0 (or compatible base URL) via HTTP Basic: username + app password.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitbucketCredentials {
    /// API root including `/2.0`, e.g. `https://api.bitbucket.org/2.0`
    pub base_api_url: String,
    pub workspace: String,
    pub username: String,
    pub app_password: String,
}

impl BitbucketCredentials {
    pub fn new(
        base_api_url: String,
        workspace: String,
        username: String,
        app_password: String,
    ) -> Self {
        Self {
            base_api_url: base_api_url.trim_end_matches('/').to_string(),
            workspace: workspace.trim().to_string(),
            username,
            app_password,
        }
    }

    pub fn api_root_trimmed(&self) -> String {
        self.base_api_url.trim_end_matches('/').to_string()
    }
}

const H_SITE: &str = "x-atlassian-site-url";
const H_EMAIL: &str = "x-atlassian-email";
const H_TOKEN: &str = "x-atlassian-api-token";
/// Optional. When set, Confluence API calls use `{this}/wiki/rest/api` instead of `{X-Atlassian-Site-Url}/wiki/rest/api`.
/// Same Atlassian Cloud site uses the same host for Jira and Confluence; only set this for atypical setups.
const H_CONFLUENCE_SITE: &str = "x-atlassian-confluence-site-url";

fn header_trimmed(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Requires all three credential headers on every MCP HTTP request. Optional Confluence site override.
pub fn resolve_credentials(parts: &Parts) -> Result<AtlassianCredentials, String> {
    let headers = &parts.headers;
    let site = header_trimmed(headers, H_SITE);
    let email = header_trimmed(headers, H_EMAIL);
    let token = header_trimmed(headers, H_TOKEN);
    let confluence_site = header_trimmed(headers, H_CONFLUENCE_SITE);

    match (&site, &email, &token) {
        (Some(s), Some(e), Some(t)) => {
            let conf = confluence_site.unwrap_or_else(|| s.clone());
            Ok(AtlassianCredentials::new(
                s.clone(),
                e.clone(),
                t.clone(),
                conf,
            ))
        }
        (None, None, None) => Err(
            "Missing required Atlassian credential HTTP headers. Send X-Atlassian-Site-Url, X-Atlassian-Email, and X-Atlassian-Api-Token on every MCP request (JSON-RPC error code -32600 Invalid Request)."
                .into(),
        ),
        _ => {
            let mut missing = Vec::new();
            if site.is_none() {
                missing.push("X-Atlassian-Site-Url");
            }
            if email.is_none() {
                missing.push("X-Atlassian-Email");
            }
            if token.is_none() {
                missing.push("X-Atlassian-Api-Token");
            }
            Err(format!(
                "Incomplete Atlassian credential headers (JSON-RPC -32600). Missing: {}. All three headers are required on every MCP HTTP request.",
                missing.join(", ")
            ))
        }
    }
}

const BB_BASE: &str = "x-bitbucket-base-url";
const BB_WORKSPACE: &str = "x-bitbucket-workspace";
const BB_USER: &str = "x-bitbucket-username";
const BB_APP_PW: &str = "x-bitbucket-app-password";

/// Default Bitbucket Cloud REST root when `X-Bitbucket-Base-Url` is omitted.
pub const BITBUCKET_CLOUD_API_2_0: &str = "https://api.bitbucket.org/2.0";

/// Requires Bitbucket credential headers on each MCP HTTP request that invokes Bitbucket tools.
pub fn resolve_bitbucket_credentials(parts: &Parts) -> Result<BitbucketCredentials, String> {
    let headers = &parts.headers;
    let base =
        header_trimmed(headers, BB_BASE).unwrap_or_else(|| BITBUCKET_CLOUD_API_2_0.to_string());
    let workspace = header_trimmed(headers, BB_WORKSPACE);
    let username = header_trimmed(headers, BB_USER);
    let app_password = header_trimmed(headers, BB_APP_PW);

    match (&workspace, &username, &app_password) {
        (Some(w), Some(u), Some(p)) => Ok(BitbucketCredentials::new(
            base,
            w.clone(),
            u.clone(),
            p.clone(),
        )),
        (None, None, None) => Err(
            "Missing required Bitbucket credential HTTP headers. Send X-Bitbucket-Workspace, X-Bitbucket-Username, and X-Bitbucket-App-Password on every MCP request that uses Bitbucket tools (JSON-RPC error code -32600 Invalid Request)."
                .into(),
        ),
        _ => {
            let mut missing = Vec::new();
            if workspace.is_none() {
                missing.push("X-Bitbucket-Workspace");
            }
            if username.is_none() {
                missing.push("X-Bitbucket-Username");
            }
            if app_password.is_none() {
                missing.push("X-Bitbucket-App-Password");
            }
            Err(format!(
                "Incomplete Bitbucket credential headers (JSON-RPC -32600). Missing: {}.",
                missing.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;

    fn parts_with_headers<I>(pairs: I) -> Parts
    where
        I: IntoIterator<Item = (&'static str, &'static str)>,
    {
        let mut builder = Request::builder();
        for (k, v) in pairs {
            builder = builder.header(k, v);
        }
        let req = builder.body(()).expect("request");
        let (parts, _) = req.into_parts();
        parts
    }

    #[test]
    fn resolve_rejects_when_no_headers() {
        let parts = parts_with_headers([]);
        let err = resolve_credentials(&parts).unwrap_err();
        assert!(err.contains("Missing required"));
    }

    #[test]
    fn resolve_rejects_partial_headers() {
        let parts = parts_with_headers([("X-Atlassian-Site-Url", "https://x.atlassian.net")]);
        let err = resolve_credentials(&parts).unwrap_err();
        assert!(err.contains("Incomplete"));
        assert!(err.contains("X-Atlassian-Email"));
    }

    #[test]
    fn resolve_accepts_three_headers() {
        let parts = parts_with_headers([
            ("X-Atlassian-Site-Url", "https://x.atlassian.net"),
            ("X-Atlassian-Email", "mcp-test-noreply@example.invalid"),
            ("X-Atlassian-Api-Token", "stub_atlassian_api_header_001"),
        ]);
        let c = resolve_credentials(&parts).unwrap();
        assert_eq!(c.site_url, "https://x.atlassian.net");
        assert_eq!(c.confluence_site_url, "https://x.atlassian.net");
        assert_eq!(c.jira_rest_v3_root(), "https://x.atlassian.net/rest/api/3");
        assert_eq!(
            c.confluence_rest_root(),
            "https://x.atlassian.net/wiki/rest/api"
        );
    }

    #[test]
    fn resolve_trims_trailing_slash_on_site_url() {
        let parts = parts_with_headers([
            ("X-Atlassian-Site-Url", "https://x.atlassian.net///"),
            ("X-Atlassian-Email", "mcp-test-noreply@example.invalid"),
            ("X-Atlassian-Api-Token", "stub_atlassian_api_header_001"),
        ]);
        let c = resolve_credentials(&parts).unwrap();
        assert_eq!(c.site_url, "https://x.atlassian.net");
    }

    #[test]
    fn resolve_confluence_site_override() {
        let parts = parts_with_headers([
            ("X-Atlassian-Site-Url", "https://jira-only.atlassian.net"),
            ("X-Atlassian-Email", "mcp-test-noreply@example.invalid"),
            ("X-Atlassian-Api-Token", "stub_atlassian_api_header_001"),
            (
                "X-Atlassian-Confluence-Site-Url",
                "https://wiki-other.atlassian.net",
            ),
        ]);
        let c = resolve_credentials(&parts).unwrap();
        assert_eq!(
            c.confluence_rest_root(),
            "https://wiki-other.atlassian.net/wiki/rest/api"
        );
    }

    #[test]
    fn bitbucket_resolve_accepts_three_headers_and_default_base() {
        let parts = parts_with_headers([
            ("X-Bitbucket-Workspace", "acme"),
            ("X-Bitbucket-Username", "stub_bb_user_001"),
            ("X-Bitbucket-App-Password", "stub_bb_header_second_001"),
        ]);
        let c = resolve_bitbucket_credentials(&parts).unwrap();
        assert_eq!(c.workspace, "acme");
        assert_eq!(c.username, "stub_bb_user_001");
        assert_eq!(c.base_api_url, BITBUCKET_CLOUD_API_2_0);
    }

    #[test]
    fn bitbucket_resolve_custom_base_url() {
        let parts = parts_with_headers([
            (
                "X-Bitbucket-Base-Url",
                "https://example.com/bitbucket/rest/api/1.0",
            ),
            ("X-Bitbucket-Workspace", "acme"),
            ("X-Bitbucket-Username", "stub_bb_user_001"),
            ("X-Bitbucket-App-Password", "stub_bb_header_second_001"),
        ]);
        let c = resolve_bitbucket_credentials(&parts).unwrap();
        assert_eq!(
            c.api_root_trimmed(),
            "https://example.com/bitbucket/rest/api/1.0"
        );
    }
}
