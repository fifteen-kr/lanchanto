use hmac::{KeyInit, Mac};
use warp::http::HeaderMap;

use crate::config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    EmptySecret,
    MissingSignature,
    MalformedSignature,
    SignatureMismatch,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::EmptySecret => "no webhook secret is configured",
            Self::MissingSignature => "missing X-Hub-Signature-256 header",
            Self::MalformedSignature => "malformed X-Hub-Signature-256 header",
            Self::SignatureMismatch => "signature mismatch",
        })
    }
}

impl std::error::Error for VerifyError {}

pub fn verify(config: &config::Config, headers: &HeaderMap, body: &[u8]) -> Result<(), VerifyError> {
    let secret = config.credential.github_webhook_secret.as_bytes();
    if secret.is_empty() {
        return Err(VerifyError::EmptySecret);
    }

    let Some(sig_header) = headers.get("X-Hub-Signature-256").and_then(|v| v.to_str().ok()) else {
        return Err(VerifyError::MissingSignature);
    };

    let Some(sig_hex) = sig_header.strip_prefix("sha256=") else {
        return Err(VerifyError::MalformedSignature);
    };
    let sig = hex::decode(sig_hex).map_err(|_| VerifyError::MalformedSignature)?;

    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(body);

    // `verify_slice` is a constant-time comparison.
    mac.verify_slice(&sig).map_err(|_| VerifyError::SignatureMismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use warp::http::header::HeaderValue;

    const SECRET: &str = "test-webhook-secret";
    const BODY: &[u8] = br#"{"action":"completed"}"#;

    fn make_config(secret: &str) -> config::Config {
        config::Config {
            credential: config::Credential {
                github_webhook_secret: secret.to_string(),
                github_token: String::new(),
            },
            deploy: Vec::new(),
        }
    }

    /// Hex-encoded HMAC-SHA256 of `body` keyed by `secret`, without the `sha256=` prefix.
    fn sign(secret: &[u8], body: &[u8]) -> String {
        type HmacSha256 = hmac::Hmac<sha2::Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
        mac.update(body);
        hex::encode(mac.finalize().into_bytes())
    }

    fn headers_with_signature(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Hub-Signature-256",
            HeaderValue::from_str(value).expect("header value is visible ascii"),
        );
        headers
    }

    #[test]
    fn accepts_correct_signature() {
        let config = make_config(SECRET);
        let headers =
            headers_with_signature(&format!("sha256={}", sign(SECRET.as_bytes(), BODY)));
        assert_eq!(verify(&config, &headers, BODY), Ok(()));
    }

    #[test]
    fn rejects_tampered_body() {
        let config = make_config(SECRET);
        let headers =
            headers_with_signature(&format!("sha256={}", sign(SECRET.as_bytes(), BODY)));
        let tampered = br#"{"action":"completed","evil":true}"#;
        assert_eq!(verify(&config, &headers, tampered), Err(VerifyError::SignatureMismatch));
    }

    #[test]
    fn rejects_missing_signature_header() {
        let config = make_config(SECRET);
        assert_eq!(verify(&config, &HeaderMap::new(), BODY), Err(VerifyError::MissingSignature));
    }

    #[test]
    fn rejects_non_hex_signature() {
        let config = make_config(SECRET);
        let headers = headers_with_signature("sha256=not-hexadecimal!!");
        assert_eq!(verify(&config, &headers, BODY), Err(VerifyError::MalformedSignature));
    }

    #[test]
    fn rejects_signature_without_sha256_prefix() {
        let config = make_config(SECRET);
        // Correct HMAC hex, but missing the mandatory `sha256=` scheme prefix.
        let headers = headers_with_signature(&sign(SECRET.as_bytes(), BODY));
        assert_eq!(verify(&config, &headers, BODY), Err(VerifyError::MalformedSignature));
    }

    #[test]
    fn rejects_empty_secret_even_with_matching_signature() {
        let config = make_config("");
        // HMAC keyed by the (empty) configured secret would match if the
        // empty-secret gate were absent; it must still be rejected.
        let headers = headers_with_signature(&format!("sha256={}", sign(b"", BODY)));
        assert_eq!(verify(&config, &headers, BODY), Err(VerifyError::EmptySecret));
    }
}
