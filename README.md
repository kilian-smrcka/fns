# fnsd — near-instant earnings + market-moving news

FNS-style feed for public companies, rebuilt for speed: the hot path reads
**newswires first** (RSS/Atom + Finnhub, polled every few seconds) and uses
**SEC EDGAR only as the confirm tier** — wire headlines arrive in ~seconds,
each gets a ✓ SEC-verified stamp minutes later when the filing lands.

One static Rust binary. No GC pauses, no interpreter, no API keys required
(Finnhub free key optional for extra speed).

## Latency, honestly

- Nothing public beats the embargo: a 4:00pm ET release appears on the
  wire at ~4:00:00–4:00:30pm. What we control is **wire→you**, and that is
  ~1 poll interval + fetch ms (default 3s cadence; set `--hot-secs 1` for
  ~0.5–1.5s on a small watchlist).
- Every item carries `wire_latency_ms` (wire timestamp → first seen), shown
  in the UI next to each headline — so you can see the real number instead
  of trusting a claim.
- "300ms" is achievable wire→you with sub-second polling on a liquid
  afternoon; pre-release ("before 4pm") is non-public info and not something
  any legitimate feed can sell you.

## Run it

```bash
cargo run --release -- --tickers AAPL,MSFT,NVDA --serve --port 8000
# open http://localhost:8000 in a browser — headlines stream in live (SSE)

# extra speed: free key from finnhub.io (60 req/min), adds company-news wire
FINNHUB_KEY=... cargo run --release -- --serve

# one hot sweep to stdout (JSON lines + ms stats on stderr)
cargo run --release -- --tickers AAPL,MSFT --once

# tighter loop for earnings afternoon
cargo run --release -- --tickers AAPL,MSFT,NVDA --hot-secs 1 --serve
```

Data lands in `data/news.db` (SQLite, deduped by URL).

## How it works

- `src/wire.rs` — hot fetchers: Google News + Yahoo per-ticker queries,
  any RSS/Atom URL in `sources.json`, Finnhub `company-news` (optional key).
  Never errors: a dead wire yields zero items, the rest keep flowing.
  Default wires (all verified live, no key): Google News per ticker, Yahoo
  per ticker, PR Newswire firehose, 2× Nasdaq RSS, SEC company Atom.
  Notes: Nasdaq drops non-browser user-agents (we send browser UA on hot
  wires, descriptive contact UA only to SEC); PR Newswire throttles
  aggressively (fails silent, recovers); Business Wire needs valid channel
  IDs and GlobeNewswire blocks datacenter IPs — both skipped for now, their
  releases reach us via Google News anyway.
- `src/classify.rs` — deterministic tagger (earnings / guidance / mna /
  leadership / capital / regulatory). Same input → same tag, every time.
  Short tickers (META, F…) need finance context or a company-name hit, so
  plain-English words don't false-positive.
- `src/feed.rs` — watchlist matching, per-item `wire_latency_ms`, broadcast.
- `src/verify.rs` — SEC Atom check every `--verify-secs` (default 90s);
  fresh 10-Q/10-K/Item-2.02 filings confirm earnings items, other 8-Ks
  confirm the rest. Unconfirmed headlines stay badged "awaiting SEC".
- `src/api.rs` — `/api/news` (history), `/api/stream` (live SSE),
  `/api/poll-now` (force a sweep).

Headlines are verbatim wire text — we tag, we never rewrite numbers.
Beat/miss vs. consensus is NOT computed (needs an estimates feed).

## Configure

`sources.json`: add RSS/Atom wires (newswire per-company feeds, company IR
press-release feeds) + company-name aliases. `fns/` holds the earlier
Python SEC-only prototype (superseded as the live path, still runs
standalone via `python3 -m fns`).

## Next

- EX-99.1 number extraction (EPS/revenue) once SEC confirms
- Consensus estimates → beat/miss flags
- Per-ticker hot wires auto-derived from IR pages
- Paid-wire upgrade path (Benzinga/DJ) for sub-second SLA
