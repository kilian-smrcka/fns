//! SQLite store: dedup by URL + fast recent-feed queries + SEC confirm.
//!
//! All functions are sync and open short-lived connections at the call
//! site — never hold a `Connection` across an `.await` (it is `!Sync`).

use rusqlite::{params, Connection};

use crate::feed::NewsItem;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS news (
  url            TEXT PRIMARY KEY,
  source         TEXT NOT NULL,
  ticker         TEXT NOT NULL,
  title          TEXT NOT NULL,
  published_ms   INTEGER,
  seen_ms        INTEGER NOT NULL,
  latency_ms     INTEGER,
  category       TEXT NOT NULL,
  importance     TEXT NOT NULL,
  detail         TEXT NOT NULL,
  verified       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_news_seen ON news(seen_ms DESC);
CREATE INDEX IF NOT EXISTS idx_news_ticker ON news(ticker, seen_ms DESC);
CREATE INDEX IF NOT EXISTS idx_news_cat ON news(category, seen_ms DESC);
";

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Insert, ignoring existing URLs. Returns true when the item is new.
pub fn insert(conn: &Connection, it: &NewsItem) -> bool {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO news
             (url, source, ticker, title, published_ms, seen_ms, latency_ms,
              category, importance, detail, verified)
             VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            params![
                it.url,
                it.source,
                it.ticker,
                it.title,
                it.published_ms,
                it.seen_ms,
                it.wire_latency_ms,
                it.category,
                it.importance,
                it.detail,
                if it.verified { 1 } else { 0 },
            ],
        )
        .unwrap_or(0);
    n == 1
}

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<NewsItem> {
    let url: String = row.get(0)?;
    let ticker: String = row.get(2)?;
    let title: String = row.get(3)?;
    Ok(NewsItem {
        url: url.clone(),
        source: row.get(1)?,
        ticker: ticker.clone(),
        title: title.clone(),
        headline: format!("{ticker} — {title}"),
        published_ms: row.get(4)?,
        seen_ms: row.get(5)?,
        wire_latency_ms: row.get(6)?,
        category: row.get(7)?,
        importance: row.get(8)?,
        detail: row.get(9)?,
        verified: row.get::<_, i64>(10)? == 1,
        fresh: false,
    })
}

pub fn recent(
    conn: &Connection,
    limit: i64,
    ticker: &str,
    category: &str,
) -> Vec<NewsItem> {
    let mut q = String::from(
        "SELECT url, source, ticker, title, published_ms, seen_ms, latency_ms,
                category, importance, detail, verified FROM news",
    );
    let mut clauses: Vec<&str> = Vec::new();
    if !ticker.is_empty() {
        clauses.push("ticker = ?");
    }
    if !category.is_empty() {
        clauses.push("category = ?");
    }
    if !clauses.is_empty() {
        q.push_str(" WHERE ");
        q.push_str(&clauses.join(" AND "));
    }
    q.push_str(" ORDER BY seen_ms DESC LIMIT ?");
    let mut stmt = match conn.prepare(&q) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let args: Vec<&dyn rusqlite::ToSql> = {
        let mut v: Vec<&dyn rusqlite::ToSql> = Vec::new();
        if !ticker.is_empty() {
            v.push(&ticker);
        }
        if !category.is_empty() {
            v.push(&category);
        }
        v.push(&limit);
        v
    };
    stmt.query_map(args.as_slice(), row_to_item)
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

/// Confirm-tier: mark items for `ticker` seen since `since_ms` as
/// SEC-verified. `earnings_form` narrows the confirmation to earnings
/// items when the filing is a 10-Q/10-K/Item-2.02 8-K; a plain 8-K
/// confirms any same-ticker item from the window. Returns # confirmed.
pub fn confirm(conn: &Connection, ticker: &str, earnings_form: bool, since_ms: i64) -> usize {
    let sql = if earnings_form {
        "UPDATE news SET verified = 1 WHERE ticker = ?1 AND verified = 0
         AND seen_ms >= ?2 AND category = 'earnings'"
    } else {
        "UPDATE news SET verified = 1 WHERE ticker = ?1 AND verified = 0
         AND seen_ms >= ?2 AND category != 'filing'"
    };
    conn.execute(sql, params![ticker, since_ms]).unwrap_or(0) as usize
}
