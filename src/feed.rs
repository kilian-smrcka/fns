//! Orchestrator: match wire items to the watchlist, classify, commit.
//!
//! `sweep_*` are async and touch only the network. [`commit`] is sync and
//! owns the SQLite write + broadcast fan-out. Splitting them keeps hot-loop
//! futures `Send` (rusqlite connections are `!Sync`) and the commit itself
//! to local microseconds.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

use crate::wire::RawItem;

/// The unit of the whole system. Headlines are verbatim wire text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub url: String,
    pub source: String,
    pub ticker: String,
    pub title: String,
    pub headline: String,
    pub published_ms: Option<i64>,
    pub seen_ms: i64,
    /// seen_ms - published_ms when the wire carried a timestamp.
    /// This is the honest wire-to-you latency of that item.
    pub wire_latency_ms: Option<i64>,
    pub category: String,
    pub importance: String,
    pub detail: String,
    pub verified: bool,
    /// True when this sweep saw the item for the first time.
    pub fresh: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HotSource {
    pub name: String,
    pub url: String,
    /// Tickers this feed is authoritative for (per-company IR feeds, SEC
    /// company Atom). Items skip text matching and attribute directly.
    /// Empty = firehose feed, match by headline text.
    #[serde(default)]
    pub tickers: Vec<String>,
    /// True for SEC EDGAR feeds: fetched with the descriptive SEC client
    /// (contact UA) instead of the browser-UA hot client.
    #[serde(default)]
    pub sec: bool,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct SweepStats {
    pub n_raw: usize,
    pub n_items: usize,
    pub n_new: usize,
    pub elapsed_ms: i64,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn build_items(
    raws: Vec<(RawItem, Vec<String>)>,
    watch: &[String],
    aliases: &[(String, String)],
    seen_ms: i64,
) -> Vec<NewsItem> {
    let mut items = Vec::new();
    for (r, forced) in raws {
        // Attribution: text-match against the watchlist first — a per-symbol
        // query often returns stories really about another ticker (e.g. an
        // "INTU stock" piece inside the MSFT query). Fall back to the feed's
        // own tickers (per-company feed, per-symbol query) when the text
        // names nothing watched. Either way, only watched tickers survive.
        let text = format!("{} {}", r.title, r.summary);
        let mut tickers = crate::classify::match_tickers(&text, watch, aliases);
        if tickers.is_empty() {
            tickers = forced.into_iter().filter(|t| watch.contains(t)).collect();
        }
        for ticker in tickers {
            let c = crate::classify::classify_wire(&r.title, &r.summary);
            items.push(NewsItem {
                url: r.url.clone(),
                source: r.source.clone(),
                ticker: ticker.clone(),
                title: r.title.clone(),
                headline: format!("{ticker} — {}", r.title),
                published_ms: r.published_ms,
                seen_ms,
                wire_latency_ms: r.published_ms.map(|p| seen_ms - p),
                category: c.category.to_string(),
                importance: c.importance.to_string(),
                detail: c.detail.clone(),
                verified: false,
                fresh: false,
            });
        }
    }
    items
}

/// One hot sweep over all configured RSS wires.
pub async fn sweep_rss(
    client: &reqwest::Client,
    sec_client: &reqwest::Client,
    sources: &[HotSource],
    watch: &[String],
    aliases: &[(String, String)],
) -> (Vec<NewsItem>, SweepStats) {
    let t0 = now_ms();
    // Fetch concurrently — one owned future per wire (Client::clone is an
    // Arc bump, so each task owns everything it touches: 'static + Send).
    let futs: Vec<_> = sources
        .iter()
        .map(|s| {
            // SEC feeds ride the descriptive-UA client; everything else
            // rides the browser-UA hot client (Nasdaq et al. drop bots).
            let c = if s.sec { sec_client.clone() } else { client.clone() };
            let name = s.name.clone();
            let url = s.url.clone();
            let tickers = s.tickers.clone();
            async move { (crate::wire::fetch_rss(&c, &name, &url).await, tickers) }
        })
        .collect();
    let raws: Vec<(RawItem, Vec<String>)> = futures_join(futs)
        .await
        .into_iter()
        .flat_map(|(items, tickers)| items.into_iter().map(move |r| (r, tickers.clone())))
        .collect();
    let seen = now_ms();
    let items = build_items(raws, watch, aliases, seen);
    let stats = SweepStats {
        n_raw: items.len(),
        n_items: items.len(),
        elapsed_ms: seen - t0,
        ..Default::default()
    };
    (items, stats)
}

/// One hot sweep over Finnhub company-news for every watched ticker.
pub async fn sweep_finnhub(
    client: &reqwest::Client,
    key: &str,
    watch: &[String],
    aliases: &[(String, String)],
    from: &str,
    to: &str,
) -> (Vec<NewsItem>, SweepStats) {
    let t0 = now_ms();
    let futs: Vec<_> = watch
        .iter()
        .map(|sym| {
            let c = client.clone();
            let k = key.to_string();
            let s = sym.clone();
            let f = from.to_string();
            let t = to.to_string();
            // Finnhub answers per symbol, so attribution is exact.
            async move { (crate::wire::fetch_finnhub(&c, &k, &s, &f, &t).await, vec![s]) }
        })
        .collect();
    let raws: Vec<(RawItem, Vec<String>)> = futures_join(futs)
        .await
        .into_iter()
        .flat_map(|(items, tickers)| items.into_iter().map(move |r| (r, tickers.clone())))
        .collect();
    let seen = now_ms();
    // Finnhub items arrive per-symbol; re-match anyway so aliases apply.
    let items = build_items(raws, watch, aliases, seen);
    let stats = SweepStats {
        n_raw: items.len(),
        n_items: items.len(),
        elapsed_ms: seen - t0,
        ..Default::default()
    };
    (items, stats)
}

/// One hot sweep over the no-key per-ticker wires (Google News + Yahoo),
/// for every watched ticker. Attribution is exact (one query per symbol).
pub async fn sweep_no_key(
    client: &reqwest::Client,
    watch: &[String],
    aliases: &[(String, String)],
) -> (Vec<NewsItem>, SweepStats) {
    let t0 = now_ms();
    let mut futs = Vec::new();
    for sym in watch {
        for kind in ["g", "y"] {
            let c = client.clone();
            let s = sym.clone();
            futs.push(async move {
                let raws = if kind == "g" {
                    crate::wire::fetch_google_news(&c, &s).await
                } else {
                    crate::wire::fetch_yahoo(&c, &s).await
                };
                (raws, vec![s])
            });
        }
    }
    let raws: Vec<(RawItem, Vec<String>)> = futures_join(futs)
        .await
        .into_iter()
        .flat_map(|(items, tickers)| items.into_iter().map(move |r| (r, tickers.clone())))
        .collect();
    let seen = now_ms();
    let items = build_items(raws, watch, aliases, seen);
    let stats = SweepStats {
        n_raw: items.len(),
        n_items: items.len(),
        elapsed_ms: seen - t0,
        ..Default::default()
    };
    (items, stats)
}

/// Minimal join-all over a Vec of futures (avoids a futures-crate dep).
async fn futures_join<F>(futs: Vec<F>) -> Vec<F::Output>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let mut handles = Vec::with_capacity(futs.len());
    for f in futs {
        handles.push(tokio::spawn(f));
    }
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(v) = h.await {
            out.push(v);
        }
    }
    out
}

/// Write items to SQLite, broadcast the new ones. Returns # newly added.
/// Sync: opens its own connection, holds nothing across awaits.
pub fn commit(db: &str, items: &mut [NewsItem], tx: &broadcast::Sender<String>) -> usize {
    let conn = match crate::store::open(db) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let mut n_new = 0;
    for it in items.iter_mut() {
        if crate::store::insert(&conn, it) {
            it.fresh = true;
            n_new += 1;
            if let Ok(json) = serde_json::to_string(&it) {
                let _ = tx.send(json);
            }
        }
    }
    n_new
}
