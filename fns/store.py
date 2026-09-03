"""SQLite store: dedup by SEC accession number + fast recent-feed queries."""

from __future__ import annotations

import sqlite3
import time

SCHEMA = """
CREATE TABLE IF NOT EXISTS news (
  accession  TEXT PRIMARY KEY,
  ticker     TEXT NOT NULL,
  company    TEXT,
  form       TEXT,
  category   TEXT,
  importance TEXT,
  headline   TEXT,
  detail     TEXT,
  filing_date TEXT,
  url        TEXT,
  first_seen INTEGER
);
CREATE INDEX IF NOT EXISTS idx_news_date ON news(filing_date DESC, accession DESC);
CREATE INDEX IF NOT EXISTS idx_news_ticker ON news(ticker, filing_date DESC);
CREATE INDEX IF NOT EXISTS idx_news_cat ON news(category, filing_date DESC);
"""


def connect(path: str) -> sqlite3.Connection:
    conn = sqlite3.connect(path)
    conn.execute("PRAGMA journal_mode=WAL;")
    conn.executescript(SCHEMA)
    return conn


def insert_new(conn: sqlite3.Connection, items: list[dict]) -> int:
    """Insert items, ignoring existing accessions. Returns # newly added."""
    if not items:
        return 0
    now = int(time.time())
    rows = [(
        it["accession"], it.get("ticker", ""), it.get("company", ""),
        it.get("form", ""), it.get("category", ""), it.get("importance", ""),
        it.get("headline", ""), it.get("detail", ""),
        it.get("filingDate", ""), it.get("url", ""), now,
    ) for it in items]
    before = conn.total_changes
    conn.executemany("INSERT OR IGNORE INTO news VALUES (?,?,?,?,?,?,?,?,?,?,?)", rows)
    conn.commit()
    return conn.total_changes - before


def recent(conn: sqlite3.Connection, limit: int = 100,
           ticker: str = "", category: str = "") -> list[dict]:
    q = "SELECT accession,ticker,company,form,category,importance,headline,detail,filing_date,url FROM news"
    clauses, args = [], []
    if ticker:
        clauses.append("ticker = ?")
        args.append(ticker.upper())
    if category:
        clauses.append("category = ?")
        args.append(category)
    if clauses:
        q += " WHERE " + " AND ".join(clauses)
    q += " ORDER BY filing_date DESC, accession DESC LIMIT ?"
    args.append(limit)
    cur = conn.execute(q, args)
    cols = ("accession", "ticker", "company", "form", "category",
            "importance", "headline", "detail", "filingDate", "url")
    return [dict(zip(cols, row)) for row in cur.fetchall()]
