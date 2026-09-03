"""Concurrent poller: one SEC request per company, all in parallel.

Speed design: ThreadPoolExecutor over blocking HTTPS (stdlib) keeps the
hot path dependency-free while saturating network I/O. A 25-ticker
watchlist typically completes in ~1-2s (one round-trip per company).
"""

from __future__ import annotations

import concurrent.futures
import time

from . import classify as C
from . import sec


def poll_once(tickers: list[str], ticker_map: dict[str, dict],
              limit_per_company: int = 8, timeout: float = 10.0,
              max_workers: int = 16) -> tuple[list[dict], dict]:
    """Poll all tickers concurrently. Returns (items_newest_first, stats)."""
    tickers = [t.upper() for t in tickers if t.strip()]
    stats = {"n_companies": len(tickers), "fetch_ms_max": 0,
             "fetch_ms_total": 0, "n_filings": 0, "elapsed_ms": 0}
    start = time.perf_counter()

    def _one(ticker: str) -> tuple[str, list[dict], int]:
        meta = ticker_map.get(ticker)
        if not meta:
            return ticker, [], 0
        subs, _status, elapsed = sec.fetch_submissions(int(meta["cik"]), timeout=timeout)
        if not subs:
            return ticker, [], elapsed
        out = []
        for f in sec.recent_filings(subs, limit=limit_per_company):
            cls = C.classify(f)
            out.append({
                **f,
                "ticker": ticker,
                "company": subs.get("name", meta.get("name", ticker)),
                "category": cls["category"],
                "importance": cls["importance"],
                "detail": cls["detail"],
                "headline": C.build_headline(ticker, subs.get("name", ticker), f, cls),
            })
        return ticker, out, elapsed

    items: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as ex:
        for _ticker, filings, elapsed in ex.map(_one, tickers):
            stats["fetch_ms_max"] = max(stats["fetch_ms_max"], elapsed)
            stats["fetch_ms_total"] += elapsed
            items.extend(filings)

    items.sort(key=lambda d: (d.get("filingDate", ""), d.get("accession", "")), reverse=True)
    stats["n_filings"] = len(items)
    stats["elapsed_ms"] = int((time.perf_counter() - start) * 1000)
    return items, stats
