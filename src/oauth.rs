//! Signing an account in — our own copy of `claude setup-token`'s OAuth flow.
//!
//! `claude setup-token` mints the right long-lived token, but it *prints* it to
//! the terminal at the end. We never want the token on screen, so instead of
//! driving that command we run the very same authorization-code flow ourselves:
//! the same public client id, scope, redirect, and PKCE challenge that the CLI
//! uses (captured from its own request). The user authorizes in the browser and
//! pastes back the short-lived **code** shown on the callback page — not a
//! secret — and we exchange that code for the token over HTTPS. The token lands
//! in a `String` and goes straight to the keychain; it is never written to
//! stdout, a file, or a log.

use std::io::Write;

use crate::store::Store;

/// Endpoints and client identity, matching Claude Code's own public OAuth
/// client (the id and client are not secrets — the PKCE verifier is the proof).
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const SCOPE: &str = "user:inference";
const BETA_HEADER: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-cli/1.0";

/// A PKCE challenge: the secret `verifier`, its S256 `challenge`, and a `state`
/// nonce tying the browser round-trip to this process.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl Pkce {
    pub fn new() -> Pkce {
        let verifier = base64url(&random_bytes());
        let challenge = base64url(&sha256(verifier.as_bytes()));
        let state = base64url(&random_bytes());
        Pkce {
            verifier,
            challenge,
            state,
        }
    }
}

/// The browser URL that starts authorization for this challenge.
pub fn authorize_url(pkce: &Pkce) -> String {
    format!(
        "{AUTHORIZE_URL}?code=true&client_id={client}&response_type=code\
         &redirect_uri={redirect}&scope={scope}\
         &code_challenge={challenge}&code_challenge_method=S256&state={state}",
        client = percent_encode(CLIENT_ID),
        redirect = percent_encode(REDIRECT_URI),
        scope = percent_encode(SCOPE),
        challenge = pkce.challenge,
        state = pkce.state,
    )
}

/// Split what the user pastes from the callback page into `(code, state)`.
///
/// The page shows `code#state`; some flows omit the `#state` suffix, so the
/// state is optional here and verified by the caller when present.
pub fn split_pasted_code(pasted: &str) -> (String, Option<String>) {
    let pasted = pasted.trim();
    match pasted.split_once('#') {
        Some((code, state)) => (code.to_string(), Some(state.to_string())),
        None => (pasted.to_string(), None),
    }
}

/// Run the whole interactive login and return the long-lived token.
///
/// `interactive` must be true: the flow needs a terminal to show the URL and
/// read the pasted code. The token is returned, never printed.
pub fn login(label: &str, interactive: bool) -> Result<String, String> {
    if !interactive {
        return Err(
            "adding an account needs an interactive terminal: it opens a browser \
             and asks you to paste a code. Run `claude-proxy add` directly in your shell."
                .into(),
        );
    }

    let pkce = Pkce::new();
    let url = authorize_url(&pkce);

    eprintln!("\nSigning in {label:?}.");
    eprintln!("Your browser will open so you can authorize the account you want {label:?} to use.");
    open_browser(&url);
    eprintln!("\nIf it did not open, visit this URL:\n\n{url}\n");
    eprint!("After authorizing, paste the code shown on the page and press Enter:\n> ");
    std::io::stderr().flush().ok();

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("could not read the pasted code: {e}"))?;
    let (code, state) = split_pasted_code(&line);
    if code.is_empty() {
        return Err("no code was pasted".into());
    }
    if let Some(returned) = &state {
        if returned != &pkce.state {
            return Err("the pasted code's state does not match — start over".into());
        }
    }

    exchange(&code, &pkce.state, &pkce.verifier, TOKEN_URL)
}

/// Exchange an authorization code for the token. `endpoint` is a parameter so
/// tests can point it at a mock server; production passes [`TOKEN_URL`].
pub fn exchange(code: &str, state: &str, verifier: &str, endpoint: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "client_id": CLIENT_ID,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });

    let resp = ureq::post(endpoint)
        .set("Content-Type", "application/json")
        .set("anthropic-beta", BETA_HEADER)
        .set("User-Agent", USER_AGENT)
        .send_json(body);

    match resp {
        Ok(r) => {
            let v: serde_json::Value = r
                .into_json()
                .map_err(|e| format!("unreadable token response: {e}"))?;
            token_from_response(&v)
        }
        Err(ureq::Error::Status(code, r)) => {
            // Never echo the request; surface only the server's own message.
            let msg = r
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|v| parse_error_body(&v))
                .unwrap_or_else(|| format!("HTTP {code}"));
            Err(format!("sign-in failed: {msg}"))
        }
        Err(ureq::Error::Transport(t)) => Err(format!("sign-in failed: {t}")),
    }
}

/// Pull the access token out of a successful response, rejecting an empty one.
pub fn token_from_response(v: &serde_json::Value) -> Result<String, String> {
    // ponytail: setup-token's grant is long-lived (~1 year), so we keep only the
    // access token and skip refresh. Add refresh_token handling if these ever
    // start arriving short-lived.
    match v.get("access_token").and_then(|x| x.as_str()) {
        Some(t) if !t.trim().is_empty() => Ok(t.to_string()),
        _ => Err("sign-in succeeded but no token was returned".into()),
    }
}

/// Human-readable message from an OAuth error body, tolerating the shapes
/// Claude's endpoints use.
pub fn parse_error_body(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.get("error_description").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    if let Some(s) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|x| x.as_str())
    {
        return Some(s.to_string());
    }
    v.get("error").and_then(|x| x.as_str()).map(str::to_string)
}

/// Store the token for `label` without it ever touching stdout.
pub fn store_token(store: &dyn Store, label: &str, token: &str) -> Result<(), String> {
    store
        .set(label, token)
        .map_err(|e| format!("could not store the token: {e}"))
}

/// Open the system browser at `url`, best-effort — the URL is also printed, so a
/// failure here is not fatal.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");

    cmd.arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.status();
}

fn random_bytes() -> [u8; 32] {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).expect("the OS secure RNG must be available");
    buf
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// Base64url without padding (RFC 4648 §5), the encoding PKCE requires.
fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18 & 0x3f) as usize] as char);
        out.push(TABLE[(n >> 12 & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6 & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        }
    }
    out
}

/// Percent-encode a query-parameter value, leaving only the RFC 3986 unreserved
/// set. Our PKCE values are already URL-safe; this covers the scope and redirect.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_known_vectors() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        // No padding, URL-safe alphabet only.
        let encoded = base64url(&[0xfb, 0xff, 0xfe]);
        assert!(!encoded.contains('='));
        assert!(encoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let pkce = Pkce::new();
        assert_eq!(pkce.challenge, base64url(&sha256(pkce.verifier.as_bytes())));
        assert_ne!(pkce.verifier, pkce.state);
        // Verifier/challenge within the RFC 7636 length bounds (43..=128).
        assert!((43..=128).contains(&pkce.verifier.len()));
    }

    #[test]
    fn authorize_url_carries_the_flow_params_encoded() {
        let pkce = Pkce::new();
        let url = authorize_url(&pkce);
        assert!(url.starts_with(AUTHORIZE_URL));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
        assert!(url.contains(&format!("state={}", pkce.state)));
        // scope `user:inference` must be encoded (`:` → %3A).
        assert!(url.contains("scope=user%3Ainference"));
        assert!(url
            .contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"));
    }

    #[test]
    fn split_pasted_code_handles_both_forms() {
        assert_eq!(
            split_pasted_code("  abc123#state-xyz \n"),
            ("abc123".to_string(), Some("state-xyz".to_string()))
        );
        assert_eq!(
            split_pasted_code("onlycode"),
            ("onlycode".to_string(), None)
        );
    }

    #[test]
    fn token_from_response_requires_a_nonempty_token() {
        assert_eq!(
            token_from_response(&serde_json::json!({"access_token": "sk-ant-oat01-x"})).unwrap(),
            "sk-ant-oat01-x"
        );
        assert!(token_from_response(&serde_json::json!({"access_token": ""})).is_err());
        assert!(token_from_response(&serde_json::json!({"nope": 1})).is_err());
    }

    #[test]
    fn parse_error_body_tolerates_oauth_and_object_shapes() {
        assert_eq!(
            parse_error_body(&serde_json::json!({"error_description": "bad code"})).as_deref(),
            Some("bad code")
        );
        assert_eq!(
            parse_error_body(&serde_json::json!({"error": {"message": "nope"}})).as_deref(),
            Some("nope")
        );
        assert_eq!(
            parse_error_body(&serde_json::json!({"error": "invalid_grant"})).as_deref(),
            Some("invalid_grant")
        );
        assert!(parse_error_body(&serde_json::json!({"x": 1})).is_none());
    }
}
