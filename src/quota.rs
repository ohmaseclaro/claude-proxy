//! Reading each account's remaining quota, ported from the shell prototype.
//!
//! A `setup-token` can call the OAuth usage endpoint — the same figures the
//! Claude app shows. The reliable signal is the HTTP status: a token without
//! the right scope answers 403, an expired one 401, a throttled one 429. Those
//! are classified exactly. The body's detailed shape is summarised
//! defensively: this renders what is present and never pretends a field it did
//! not see.

use std::time::Duration;

/// The usage endpoint and the beta header it requires (same as the Claude app).
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA: &str = "oauth-2025-04-20";

/// What a usage lookup for one account came back as.
pub enum Usage {
    /// A parsed summary line, best-effort from the body.
    Ok(String),
    /// The token is valid but lacks the usage scope (403). Expected for some
    /// setup-tokens; not a failure to fix.
    NeedsScope,
    /// The token is rejected (401) — sign the account in again.
    Expired,
    /// Rate limited (429).
    RateLimited,
    /// A transport or unexpected-status problem, described.
    Error(String),
}

impl Usage {
    pub fn summary(&self) -> String {
        match self {
            Usage::Ok(s) => s.clone(),
            Usage::NeedsScope => "token lacks usage scope".into(),
            Usage::Expired => "expired — run `claude-proxy add` again".into(),
            Usage::RateLimited => "rate limited".into(),
            Usage::Error(e) => format!("quota unavailable: {e}"),
        }
    }
}

/// Fetch usage for one token. Blocking: a CLI wants one call, not a runtime.
pub fn fetch(token: &str) -> Usage {
    let resp = ureq::get(USAGE_URL)
        .timeout(Duration::from_secs(15))
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", BETA)
        .set("anthropic-version", "2023-06-01")
        .call();

    match resp {
        Ok(r) => match r.into_json::<serde_json::Value>() {
            Ok(v) => Usage::Ok(summarize(&v)),
            Err(e) => Usage::Error(format!("unreadable response: {e}")),
        },
        Err(ureq::Error::Status(403, _)) => Usage::NeedsScope,
        Err(ureq::Error::Status(401, _)) => Usage::Expired,
        Err(ureq::Error::Status(429, _)) => Usage::RateLimited,
        Err(ureq::Error::Status(code, _)) => Usage::Error(format!("HTTP {code}")),
        Err(ureq::Error::Transport(t)) => Usage::Error(t.to_string()),
    }
}

/// Render a one-line summary from the usage body.
///
/// Defensive by design: the endpoint's exact schema is not pinned here, so this
/// looks for the fields the app is known to use and falls back to "ok" rather
/// than inventing numbers. A future change can enrich this once the shape is
/// confirmed against a live token.
pub fn summarize(v: &serde_json::Value) -> String {
    // Common shape: a `five_hour`/`seven_day` object with `utilization` (0..1)
    // and a reset time. Pull whatever is present.
    let pct = |node: &serde_json::Value, key: &str| -> Option<String> {
        node.get(key)
            .and_then(|w| w.get("utilization").or_else(|| w.get("used")))
            .and_then(serde_json::Value::as_f64)
            .map(|u| format!("{}%", (u * 100.0).round() as i64))
    };
    let mut parts = Vec::new();
    if let Some(s) = pct(v, "five_hour").or_else(|| pct(v, "session")) {
        parts.push(format!("session {s}"));
    }
    if let Some(s) = pct(v, "seven_day").or_else(|| pct(v, "weekly")) {
        parts.push(format!("weekly {s}"));
    }
    if parts.is_empty() {
        "ok".into()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_reads_utilization_when_present() {
        let v = serde_json::json!({
            "five_hour": {"utilization": 0.13},
            "seven_day": {"utilization": 0.04}
        });
        assert_eq!(summarize(&v), "session 13%, weekly 4%");
    }

    #[test]
    fn summarize_falls_back_to_ok_not_fiction() {
        let v = serde_json::json!({"something_unexpected": true});
        assert_eq!(summarize(&v), "ok");
    }

    #[test]
    fn status_summaries_are_actionable() {
        assert!(Usage::Expired.summary().contains("add"));
        assert_eq!(Usage::NeedsScope.summary(), "token lacks usage scope");
    }
}
