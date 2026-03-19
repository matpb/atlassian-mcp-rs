use http::request::Parts;
use http::HeaderMap;

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
            ("X-Atlassian-Email", "a@b.com"),
            ("X-Atlassian-Api-Token", "tok"),
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
            ("X-Atlassian-Email", "a@b.com"),
            ("X-Atlassian-Api-Token", "tok"),
        ]);
        let c = resolve_credentials(&parts).unwrap();
        assert_eq!(c.site_url, "https://x.atlassian.net");
    }

    #[test]
    fn resolve_confluence_site_override() {
        let parts = parts_with_headers([
            ("X-Atlassian-Site-Url", "https://jira-only.atlassian.net"),
            ("X-Atlassian-Email", "a@b.com"),
            ("X-Atlassian-Api-Token", "tok"),
            (
                "X-Atlassian-Confluence-Site-Url",
                "https://wiki-other.atlassian.net",
            ),
        ]);
        let c = resolve_credentials(&parts).unwrap();
        assert_eq!(c.confluence_rest_root(), "https://wiki-other.atlassian.net/wiki/rest/api");
    }
}
