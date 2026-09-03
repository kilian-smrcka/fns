//! fnsd — near-instant earnings + market-moving news monitor.
//!
//! Hot tier: RSS/Atom wires + Finnhub, polled every few seconds, matched
//! against your watchlist, classified, pushed over SSE in milliseconds.
//! Confirm tier: SEC EDGAR flips items to verified minutes later.
//!
//!   FINNHUB_KEY=... fnsd --tickers AAPL,MSFT,NVDA --serve --port 8000
//!   fnsd --tickers AAPL,MSFT --once        # one hot sweep, JSON lines

mod api;
mod classify;
mod feed;
mod store;
mod verify;
mod wire;

use std::sync::Arc;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "fnsd", about = "Near-instant earnings + market-moving news monitor")]
struct Args {
    /// Comma-separated tickers to watch.
    #[arg(long, default_value = "AAPL,MSFT,NVDA,AMZN,META,GOOGL,TSLA,AMD,JPM,XOM")]
    tickers: String,
    /// Hot-tier poll interval in seconds (wires + Finnhub).
    #[arg(long, default_value_t = 3)]
    hot_secs: u64,
    /// Confirm-tier interval in seconds (SEC EDGAR).
    #[arg(long, default_value_t = 90)]
    verify_secs: u64,
    /// SQLite path (dedup + history).
    #[arg(long, default_value = "data/news.db")]
    db: String,
    /// Wire source config (RSS list + company-name aliases).
    #[arg(long, default_value = "sources.json")]
    sources: String,
    /// Free key from finnhub.io (60 req/min). Empty = wires + SEC only.
    #[arg(long, env = "FINNHUB_KEY", default_value = "")]
    finnhub_key: String,
    /// Run the HTTP feed UI + SSE stream.
    #[arg(long, default_value_t = false)]
    serve: bool,
    #[arg(long, default_value_t = 8000)]
    port: u16,
    /// Single hot sweep to stdout, then exit.
    #[arg(long, default_value_t = false)]
    once: bool,
}

#[derive(Debug, Default)]
struct SourceCfg {
    rss: Vec<feed::HotSource>,
    aliases: Vec<(String, String)>,
}

/// sources.json: {"rss": [{"name":..,"url":..}], "aliases": {"APPLE INC": "AAPL"}}.
/// Missing file or bad JSON = empty config, never fatal.
fn load_sources(path: &str) -> SourceCfg {
    let mut cfg = SourceCfg::default();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return cfg,
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return cfg,
    };
    if let Some(arr) = v.get("rss").and_then(|x| x.as_array()) {
        for e in arr {
            let name = e.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let url = e.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let tickers: Vec<String> = e
                .get("tickers")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_uppercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let sec = e.get("sec").and_then(|x| x.as_bool()).unwrap_or(false);
            if !name.is_empty() && (url.starts_with("http://") || url.starts_with("https://")) {
                cfg.rss.push(feed::HotSource { name, url, tickers, sec });
            }
        }
    }
    if let Some(obj) = v.get("aliases").and_then(|x| x.as_object()) {
        for (alias, sym) in obj {
            if let Some(sym) = sym.as_str() {
                let sym = sym.trim().to_uppercase();
                let alias = alias.trim().to_uppercase();
                if !sym.is_empty() && !alias.is_empty() {
                    cfg.aliases.push((alias, sym));
                }
            }
        }
    }
    cfg
}

/// Browser-UA client for the hot wires. Several publishers (Nasdaq et al.)
/// drop non-browser clients at connection level, so identification happens
/// per SEC rules only where required (see `build_sec_client`).
fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36 fnsd/0.2")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .expect("tls backend must initialize")
}

/// Descriptive-UA client for SEC EDGAR (per https://www.sec.gov/os/accessing-edgar-data).
fn build_sec_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("fnsd/0.2 (hot-wire monitor; contact: hello@example.com)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("tls backend must initialize")
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let watch: Vec<String> = args
        .tickers
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if watch.is_empty() {
        eprintln!("no tickers given");
        std::process::exit(2);
    }
    let cfg = load_sources(&args.sources);
    if cfg.rss.is_empty() && args.finnhub_key.is_empty() {
        eprintln!(
            "warning: no hot-tier sources (sources.json has no rss entries and no --finnhub-key): \
             running on SEC confirm-tier only — fast headlines need a wire source"
        );
    }
    if let Err(e) = store::open(&args.db) {
        eprintln!("cannot open db {}: {e}", args.db);
        std::process::exit(2);
    }

    let (tx, _rx) = tokio::sync::broadcast::channel::<String>(1024);
    let app = Arc::new(api::App {
        db: args.db.clone(),
        tx: tx.clone(),
        http: build_client(),
        sec_http: build_sec_client(),
        watch: watch.clone(),
        aliases: cfg.aliases.clone(),
        rss: cfg.rss.clone(),
        finnhub_key: args.finnhub_key.clone(),
    });

    if args.once {
        let (mut items, stats) = api::hot_sweep(&app, &watch).await;
        let n_new = feed::commit(&args.db, &mut items, &tx);
        for it in &items {
            println!("{}", serde_json::to_string(it).unwrap_or_default());
        }
        eprintln!(
            "# {} items ({} new) in {}ms across {} tickers",
            stats.n_items, n_new, stats.elapsed_ms, watch.len()
        );
        return;
    }

    // Hot loop: wires every hot_secs.
    {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(args.hot_secs.max(1)));
            loop {
                tick.tick().await;
                let (mut items, stats) = api::hot_sweep(&app, &app.watch).await;
                let n_new = feed::commit(&app.db, &mut items, &app.tx);
                if n_new > 0 || stats.n_items > 0 {
                    eprintln!(
                        "hot sweep: {} items ({} new) in {}ms",
                        stats.n_items, n_new, stats.elapsed_ms
                    );
                }
            }
        });
    }

    // Confirm loop: SEC EDGAR every verify_secs.
    {
        let app = app.clone();
        tokio::spawn(async move {
            let cik_map = verify::load_cik_map(&app.sec_http).await;
            eprintln!("sec map: {} companies", cik_map.len());
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(args.verify_secs.max(30)));
            loop {
                tick.tick().await;
                for t in &app.watch {
                    if let Some((cik, _)) = cik_map.get(t) {
                        let n = verify::verify_ticker(&app.sec_http, &app.db, *cik, t).await;
                        if n > 0 {
                            eprintln!("sec confirmed {n} {t} items");
                        }
                    }
                }
            }
        });
    }

    if args.serve {
        let addr = format!("127.0.0.1:{}", args.port);
        let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind port");
        eprintln!("fnsd feed: http://{addr}  (hot every {}s, SEC verify every {}s)",
            args.hot_secs, args.verify_secs);
        axum::serve(listener, api::router(app)).await.expect("serve");
    } else {
        eprintln!("running headless (no --serve); Ctrl-C to stop");
        tokio::signal::ctrl_c().await.ok();
    }
}
