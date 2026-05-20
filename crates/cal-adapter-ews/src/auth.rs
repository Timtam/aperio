//! Basic-auth header builder for EWS.
//!
//! EWS supports a handful of auth schemes — Basic, NTLM, OAuth.
//! Aperio's 6f.1 ships only Basic, which is what most on-premise
//! Exchange installs and self-hosted alternatives (Kerio, etc.)
//! still default to. NTLM needs a CGSS-API-style handshake that's
//! out of scope here; OAuth-against-EWS exists for Exchange Online
//! but Microsoft has been pushing customers to Graph for that
//! workload for years.

use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

use crate::error::{EwsError, EwsResult};

#[derive(Debug, Clone)]
pub struct BasicCredentials {
    pub username: String,
    pub password: String,
}

pub fn basic_auth_header(creds: &BasicCredentials) -> EwsResult<HeaderMap> {
    let token = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", creds.username, creds.password));
    let value = format!("Basic {token}");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&value)
            .map_err(|e| EwsError::Config(format!("auth header: {e}")))?,
    );
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_header_encodes_userpass() {
        let creds = BasicCredentials {
            username: "alice@example.com".into(),
            password: "hunter2".into(),
        };
        let headers = basic_auth_header(&creds).unwrap();
        let value = headers
            .get(AUTHORIZATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(value.starts_with("Basic "));
        // alice@example.com:hunter2 -> YWxpY2VAZXhhbXBsZS5jb206aHVudGVyMg==
        assert_eq!(value, "Basic YWxpY2VAZXhhbXBsZS5jb206aHVudGVyMg==");
    }
}
