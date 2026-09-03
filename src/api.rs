//! HTTP tier: feed UI + JSON API + live SSE stream.
//!
//! - `GET /` → the feed UI
//! - `GET /api/news?ticker=&category=&limit=` → stored items, newest first
//! - `GET /api/stream` → Server-Sent Events, one event per fresh headline
//! - `GET /api/poll-now?tickers=&limit=` → force one hot sweep, return items

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::{Html, Sse},
    routing::get,
    Json, Router,
};
use axum::response::sse::{Event, KeepAlive};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

use crate::feed::{HotSource, NewsItem, SweepStats};

pub struct App {
    pub db: String,
    pub tx: broadcast::Sender<String>,
    pub http: reqwest::Client,
    pub sec_http: reqwest::Client,
    pub watch: Vec<String>,
    pub aliases: Vec<(String, String)>,
    pub rss: Vec<HotSource>,
    pub finnhub_key: String,
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/news", get(news))
        .route("/api/stream", get(stream))
        .route("/api/poll-now", get(poll_now))
        .with_state(app)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../index.html"))
}

#[derive(Debug, Deserialize)]
struct NewsQ {
    ticker: Option<String>,
    category: Option<String>,
    limit: Option<i64>,
}

async fn news(State(st): State<Arc<App>>, Query(q): Query<NewsQ>) -> Json<serde_json::Value> {
    let items: Vec<NewsItem> = crate::store::open(&st.db)
        .map(|c| {
            crate::store::recent(
                &c,
                q.limit.unwrap_or(100).clamp(1, 500),
                &q.ticker.unwrap_or_default(),
                &q.category.unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    Json(serde_json::json!({ "items": items }))
}

async fn stream(State(st): State<Arc<App>>) -> impl axum::response::IntoResponse {
    let rx = st.tx.subscribe();
    // Drop lagged/ping errors: a slow browser just misses an event, the
    // next refresh replays everything from SQLite anyway.
    let s = BroadcastStream::new(rx)
        .filter_map(|r| r.ok().map(|msg| Ok::<Event, std::convert::Infallible>(Event::default().data(msg))));
    Sse::new(s).keep_alive(KeepAlive::default())
}

#[derive(Debug, Deserialize)]
struct PollQ {
    tickers: Option<String>,
    limit: Option<usize>,
}

async fn poll_now(
    State(st): State<Arc<App>>,
    Query(q): Query<PollQ>,
) -> Json<serde_json::Value> {
    let watch: Vec<String> = q
        .tickers
        .map(|t| t.split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_else(|| st.watch.clone());
    let (mut items, stats) = hot_sweep(&st, &watch).await;
    let _ = q.limit; // per-company depth is a wire property, not slicing here
    let n_new = crate::feed::commit(&st.db, &mut items, &st.tx);
    let stats = SweepStats { n_new, ..stats };
    Json(serde_json::json!({ "items": items, "stats": stats }))
}

/// One full hot sweep shared by the poll loop and /api/poll-now.
pub async fn hot_sweep(st: &App, watch: &[String]) -> (Vec<NewsItem>, SweepStats) {
    let (mut items, mut stats) =
        crate::feed::sweep_rss(&st.http, &st.sec_http, &st.rss, watch, &st.aliases).await;
    // No-key per-ticker wires run on every sweep — this is the early tier.
    if !watch.is_empty() {
        let (mut nk, s1) = crate::feed::sweep_no_key(&st.http, watch, &st.aliases).await;
        stats.n_raw += s1.n_raw;
        stats.elapsed_ms += s1.elapsed_ms;
        items.append(&mut nk);
    }
    if !st.finnhub_key.is_empty() && !watch.is_empty() {
        let to = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let from = (chrono::Utc::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let (mut fh, s2) =
            crate::feed::sweep_finnhub(&st.http, &st.finnhub_key, watch, &st.aliases, &from, &to).await;
        stats.n_raw += s2.n_raw;
        stats.elapsed_ms += s2.elapsed_ms;
        items.append(&mut fh);
    }
    // Newest first for the UI.
    items.sort_by_key(|i| std::cmp::Reverse((i.seen_ms, i.url.clone())));
    stats.n_items = items.len();
    (items, stats)
}
