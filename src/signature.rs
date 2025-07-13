use super::config;
use warp::http::HeaderMap;
use hmac::Mac;

pub fn verify(config: &config::Config, headers: &HeaderMap, body: &[u8]) -> Result<(), &'static str> {
    let secret = config.credential.github_webhook_secret.as_bytes();
    if secret.is_empty() {
        return Err("empty secret");
    }

    let sig_header =  headers.get("X-Hub-Signature-256").or_else(|| headers.get("X-Hub-Signature")).and_then(|v| v.to_str().ok());
    let sig_header = match sig_header {
        Some(v) => v,
        None => return Err("missing signature"),
    };

    let sig = sig_header.strip_prefix("sha256=").unwrap_or("");
    let sig = match hex::decode(sig) {
        Ok(v) => v,
        Err(_) => return Err("invalid signature"),
    };
    
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(v) => v,
        Err(_) => return Err("failed to initialize hmac"),
    };
    
    mac.update(body);
    if !mac.verify_slice(&sig).is_ok() {
        return Err("signature mismatch");
    }

    Ok(())
}