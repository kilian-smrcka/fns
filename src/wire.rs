//! Hot-tier fetchers: RSS/Atom wires + Finnhub company news.
//!
//! Everything here is fetch-and-normalize only: no DB, no broadcast, so no
//! borrowed state is held across `.await` points. Callers commit results
//! via [`crate::feed::commit`] after the fetch completes.

/// One normalized headline from any wire source.
#[derive(Debug, Clone)]
pub struct RawItem {
    pub source: String,
    pub title: String,
    pub url: String,
    pub published_ms: Option<i64>,
    pub summary: String,
}

/// Fetch one RSS/Atom feed URL, return its entries newest-first when the
/// feed carries dates. Never errors — a dead wire yields zero items.
pub async fn fetch_rss(client: &reqwest::Client, name: &str, url: &str) -> Vec<RawItem> {
    let mut out = Vec::new();
    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            if std::env::var("FNSD_DEBUG").is_ok() {
                eprintln!("wire {name}: request failed: {e}");
            }
            return out;
        }
    };
    if !resp.status().is_success() {
        if std::env::var("FNSD_DEBUG").is_ok() {
            eprintln!("wire {name}: http {}", resp.status());
        }
        return out;
    }
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            if std::env::var("FNSD_DEBUG").is_ok() {
                eprintln!("wire {name}: body failed: {e}");
            }
            return out;
        }
    };
    // Some publishers (notably SEC EDGAR Atom) tag text elements with
    // type="text/xml", which feed-rs rejects as an unknown text type and
    // fails the whole feed. The payload is escaped text we treat as plain
    // text anyway, so normalize the attribute pre-parse.
    let bytes = sanitize_text_xml(&bytes);
    let feed = match feed_rs::parser::parse(std::io::Cursor::new(bytes)) {
        Ok(f) => f,
        Err(e) => {
            if std::env::var("FNSD_DEBUG").is_ok() {
                eprintln!("wire {name}: parse failed: {e}");
            }
            return out;
        }
    };
    for e in feed.entries {
        let title = e.title.map(|t| t.content.trim().to_string()).unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let link = e.links.into_iter().next().map(|l| l.href).unwrap_or_default();
        if link.is_empty() {
            continue;
        }
        let published_ms = e.published.or(e.updated).map(|d| d.timestamp_millis());
        let summary = e.summary.map(|s| s.content.trim().to_string()).unwrap_or_default();
        out.push(RawItem {
            source: name.to_string(),
            title,
            url: link,
            published_ms,
            summary,
        });
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.published_ms.unwrap_or(0)));
    if std::env::var("FNSD_DEBUG").is_ok() && !out.is_empty() {
        eprintln!("wire {name}: {} entries, first: {:?}", out.len(), out[0].title);
    }
    out
}

/// Replace type="text/xml" (double or single quoted, any case) with
/// type="text". Pure byte scan, no allocation when absent.
fn sanitize_text_xml(input: &[u8]) -> Vec<u8> {
    // Match `type=` (5) + quote (1) + `text/xml` (8) + same quote (1) = 15.
    const MATCH_LEN: usize = 15;
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];
        // Quote at 5, `text/xml` at 6..14, closing quote at 14.
        let hit = rest.len() >= MATCH_LEN
            && rest[..5].eq_ignore_ascii_case(b"type=")
            && (rest[5] == b'"' || rest[5] == b'\'')
            && rest[6..14].eq_ignore_ascii_case(b"text/xml")
            && rest[14] == rest[5];
        if hit {
            out.extend_from_slice(&input[i..i + 6]);
            out.extend_from_slice(b"text");
            out.push(input[i + 5]);
            i += MATCH_LEN;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_double_quoted() {
        let out = sanitize_text_xml(br#"<summary type="text/xml">x</summary>"#);
        assert_eq!(out, br#"<summary type="text">x</summary>"#.to_vec());
    }

    #[test]
    fn sanitizes_single_quoted_and_case() {
        let out = sanitize_text_xml(b"<title TYPE='TEXT/XML'>x</title>");
        assert_eq!(out, b"<title TYPE='text'>x</title>".to_vec());
    }

    #[test]
    fn leaves_other_types_alone() {
        let src = br#"<title type="html">x</title><link type="text/html"/>"#;
        assert_eq!(sanitize_text_xml(src), src.to_vec());
    }

    #[test]
    fn short_tail_does_not_panic() {
        assert_eq!(sanitize_text_xml(b"type=\"text/xm"), b"type=\"text/xm".to_vec());
        assert_eq!(sanitize_text_xml(b""), b"".to_vec());
    }
}

/// Google News per-ticker search. No key, ~100 recent items, timestamps
/// usually minutes fresh. Attribution is exact (one query per symbol).
pub async fn fetch_google_news(client: &reqwest::Client, symbol: &str) -> Vec<RawItem> {
    // `symbol stock` keeps the query on the company, not the fruit.
    let url = format!(
        "https://news.google.com/rss/search?q={symbol}%20stock&hl=en-US&gl=US&ceid=US:en"
    );
    fetch_rss(client, "google-news", &url).await
}

/// Yahoo Finance per-ticker headline feed. No key, per-symbol attribution.
pub async fn fetch_yahoo(client: &reqwest::Client, symbol: &str) -> Vec<RawItem> {
    let url = format!(
        "https://feeds.finance.yahoo.com/rss/2.0/headline?s={symbol}&region=US&lang=en-US"
    );
    fetch_rss(client, "yahoo", &url).await
}

#[derive(Debug, serde::Deserialize)]
struct FhNews {
    #[serde(default)]
    headline: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    summary: String,
    /// Unix seconds.
    #[serde(default)]
    datetime: i64,
}

/// Finnhub company-news for one symbol over [from, to] (YYYY-MM-DD).
/// Free tier: 60 calls/min — the caller staggers symbols across ticks.
pub async fn fetch_finnhub(
    client: &reqwest::Client,
    key: &str,
    symbol: &str,
    from: &str,
    to: &str,
) -> Vec<RawItem> {
    let url = format!(
        "https://finnhub.io/api/v1/company-news?symbol={symbol}&from={from}&to={to}&token={key}"
    );
    let items: Vec<FhNews> = match client.get(url).send().await {
        Ok(r) => match r.json().await {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        },
        Err(_) => return Vec::new(),
    };
    items
        .into_iter()
        .filter(|n| !n.headline.is_empty() && !n.url.is_empty())
        .map(|n| RawItem {
            source: if n.source.is_empty() { "finnhub".to_string() } else { format!("finnhub/{}", n.source) },
            title: n.headline,
            url: n.url,
            published_ms: if n.datetime > 0 { Some(n.datetime * 1000) } else { None },
            summary: n.summary,
        })
        .collect()
}
