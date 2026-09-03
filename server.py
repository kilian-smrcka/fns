"""Minimal browser feed: serves index.html + JSON API. Stdlib only.

Endpoints:
  GET /                      -> index.html (FNS-like live feed)
  GET /api/news?ticker=&category=&limit=  -> rows from SQLite
  GET /api/poll?tickers=AAPL,MSFT&limit=5 -> live SEC poll, stores + returns items

Run:  python3 server.py --port 8000
Then open http://localhost:8000
"""

from __future__ import annotations

import argparse
import json
import os
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from fns import feed, sec, store

TICKER_MAP: dict = {}
DB_PATH = "data/news.db"


class Handler(BaseHTTPRequestHandler):
    server_version = "FNS/0.1"

    def _send_json(self, obj, status: int = 200) -> None:
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        qs = urllib.parse.parse_qs(parsed.query)
        if parsed.path in ("/", "/index.html"):
            try:
                with open("index.html", "rb") as f:
                    body = f.read()
            except FileNotFoundError:
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if parsed.path == "/api/news":
            os.makedirs("data", exist_ok=True)
            conn = store.connect(DB_PATH)
            try:
                rows = store.recent(
                    conn,
                    limit=min(int(qs.get("limit", ["100"])[0]), 500),
                    ticker=qs.get("ticker", [""])[0],
                    category=qs.get("category", [""])[0],
                )
            finally:
                conn.close()
            self._send_json({"items": rows})
            return
        if parsed.path == "/api/poll":
            tickers = [t.strip().upper() for t in
                       qs.get("tickers", ["AAPL,MSFT,NVDA"])[0].split(",") if t.strip()]
            limit = min(int(qs.get("limit", ["5"])[0]), 10)
            tickers = [t for t in tickers if t in TICKER_MAP][:50]
            items, stats = feed.poll_once(tickers, TICKER_MAP, limit_per_company=limit)
            os.makedirs("data", exist_ok=True)
            conn = store.connect(DB_PATH)
            try:
                added = store.insert_new(conn, items)
            finally:
                conn.close()
            self._send_json({"items": items, "stats": stats, "added": added})
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, *a) -> None:  # quieter logs
        pass


def main() -> int:
    global TICKER_MAP, DB_PATH
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=8000)
    p.add_argument("--db", default="data/news.db")
    a = p.parse_args()
    DB_PATH = a.db
    print("loading SEC ticker map ...", flush=True)
    TICKER_MAP, ms = sec.load_ticker_map()
    print(f"ticker map: {len(TICKER_MAP)} companies in {ms}ms", flush=True)
    os.makedirs("data", exist_ok=True)
    store.connect(DB_PATH).close()
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"FNS feed: http://localhost:{a.port}", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
