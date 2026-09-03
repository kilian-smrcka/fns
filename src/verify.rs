//! Confirm tier: SEC EDGAR as the verifier, never the source.
//!
//! Every ~90s we check each watched ticker's EDGAR Atom feed for fresh
//! earnings filings (8-K Item 2.02, 10-Q/10-K). When one lands, wire items
//! for that ticker inside the window flip to `verified = true` in the UI.
//! A wire headline you act on at 4pm gets its SEC stamp minutes later.

use std::collections::HashMap;

use crate::feed::now_ms;

const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const DAY_MS: i64 = 86_400_000;

/// ticker -> (cik, company name), uppercased tickers.
pub async fn load_cik_map(client: &reqwest::Client) -> HashMap<String, (u32, String)> {
    let mut map = HashMap::new();
    let resp = match client.get(TICKERS_URL).send().await {
        Ok(r) => r,
        Err(_) => return map,
    };
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => return map,
    };
    let v: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return map,
    };
    // Shape: {"0": {"cik_str": 320193, "ticker": "AAPL", "title": "Apple Inc."}, ...}
    if let Some(obj) = v.as_object() {
        for (_, e) in obj {
            let t = e.get("ticker").and_then(|x| x.as_str()).unwrap_or("").to_uppercase();
            let cik = e
                .get("cik_str")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            let name = e.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if !t.is_empty() && cik > 0 {
                map.insert(t, (cik, name));
            }
        }
    }
    map
}

/// Check one ticker's EDGAR Atom feed; confirm matching wire items.
/// Returns # items newly verified.
pub async fn verify_ticker(
    client: &reqwest::Client,
    db: &str,
    cik: u32,
    ticker: &str,
) -> usize {
    let url = format!(
        "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={cik:010}&type=&count=20&output=atom"
    );
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if !resp.status().is_success() {
        return 0;
    }
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let feed = match feed_rs::parser::parse(std::io::Cursor::new(bytes)) {
        Ok(f) => f,
        Err(_) => return 0,
    };
    // Only filings from the last 4 days can confirm anything.
    let cutoff = now_ms() - 4 * DAY_MS;
    let mut earnings_hit = false;
    let mut any_hit = false;
    for e in &feed.entries {
        let title = e.title.as_ref().map(|t| t.content.clone()).unwrap_or_default();
        let form = title.split_whitespace().next().unwrap_or("");
        let date_ms = e.published.or(e.updated).map(|d| d.timestamp_millis()).unwrap_or(0);
        if date_ms < cutoff {
            continue;
        }
        // Item 2.02 8-Ks usually say so in the Atom summary.
        let sum = e.summary.as_ref().map(|s| s.content.clone()).unwrap_or_default();
        let items = if sum.contains("2.02") || title.contains("2.02") { "2.02" } else { "" };
        match crate::classify::classify_sec(form, items, &title).category {
            "earnings" => earnings_hit = true,
            // Any 8-K confirms same-ticker wire items; other forms confirm nothing.
            _ if form == "8-K" || form == "8-K/A" => any_hit = true,
            _ => {}
        }
    }
    if !earnings_hit && !any_hit {
        return 0;
    }
    let conn = match crate::store::open(db) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    // Earnings filings confirm earnings items; any 8-K confirms the rest
    // (but never bare unclassified 'filing' rows — too weak a link).
    let mut n = 0;
    if earnings_hit {
        n += crate::store::confirm(&conn, ticker, true, cutoff);
    }
    if any_hit {
        n += crate::store::confirm(&conn, ticker, false, cutoff);
    }
    n
}
