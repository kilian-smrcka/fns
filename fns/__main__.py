"""CLI: python3 -m fns --tickers AAPL,MSFT,NVDA [--watch 60]"""

from __future__ import annotations

import argparse
import json
import sys
import time

from . import feed, sec, store

DEFAULT_TICKERS = "AAPL,MSFT,NVDA,AMZN,META,GOOGL,TSLA,AMD,JPM,XOM"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description="FNS-style earnings + market-moving SEC feed")
    p.add_argument("--tickers", default=DEFAULT_TICKERS,
                   help="Comma-separated tickers (default: %(default)s)")
    p.add_argument("--limit", type=int, default=5, help="Filings per company per poll")
    p.add_argument("--db", default="data/news.db", help="SQLite path (dedup + history)")
    p.add_argument("--watch", type=float, default=0,
                   help="Poll every N seconds (0 = run once and exit)")
    p.add_argument("--earnings-only", action="store_true", help="Print only earnings items")
    p.add_argument("--json", action="store_true", help="Emit JSON lines instead of text")
    p.add_argument("--no-store", action="store_true", help="Skip SQLite writes")
    return p.parse_args(argv)


def fmt(item: dict) -> str:
    tag = item["category"].upper()
    imp = item["importance"]
    return (f"[{item.get('filingDate', '?')} {item['form']:<8} {tag:<10} {imp:<6}] "
            f"{item['headline']}\n  {item['url']}")


def run_once(tickers: list[str], tmap: dict, args: argparse.Namespace,
             conn) -> int:
    items, stats = feed.poll_once(tickers, tmap, limit_per_company=args.limit)
    if args.earnings_only:
        items = [i for i in items if i["category"] == "earnings"]
    added = 0
    if conn is not None and not args.no_store:
        added = store.insert_new(conn, items)
    for it in items:
        if args.json:
            print(json.dumps(it))
        else:
            print(fmt(it))
    print(f"# {stats['n_filings']} filings from {stats['n_companies']} companies "
          f"in {stats['elapsed_ms']}ms "
          f"(slowest company {stats['fetch_ms_max']}ms)"
          + (f", +{added} new in db" if conn is not None and not args.no_store else ""),
          file=sys.stderr)
    return added


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    tickers = [t.strip().upper() for t in args.tickers.split(",") if t.strip()]
    if not tickers:
        print("No tickers given.", file=sys.stderr)
        return 2
    print(f"# resolving {len(tickers)} tickers via SEC company_tickers.json ...",
          file=sys.stderr)
    t0 = time.perf_counter()
    tmap, map_ms = sec.load_ticker_map()
    missing = [t for t in tickers if t not in tmap]
    print(f"# ticker map: {len(tmap)} companies in {map_ms}ms "
          f"({(time.perf_counter()-t0)*1000:.0f}ms incl. parse)", file=sys.stderr)
    if missing:
        print(f"# WARNING: not SEC-mapped, skipped: {', '.join(missing)}", file=sys.stderr)
        tickers = [t for t in tickers if t in tmap]
    if not tickers:
        return 2
    conn = None
    if not args.no_store:
        import os
        os.makedirs("data", exist_ok=True)
        conn = store.connect(args.db)
    run_once(tickers, tmap, args, conn)
    while args.watch and args.watch > 0:
        time.sleep(args.watch)
        run_once(tickers, tmap, args, conn)
    if conn is not None:
        conn.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
