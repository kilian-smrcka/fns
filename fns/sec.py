"""SEC EDGAR client. Stdlib only. Optimized for speed + correctness.

Correctness rules:
- Every item keeps its SEC accession number (unique ID) and direct
  Archives URL so any headline is one click verifiable.
- We classify from structured fields (form, items, filingDate) — never
  from guessed article text.

Speed rules:
- One HTTPS request per company (submissions JSON, gzip).
- Callers fetch companies concurrently (see feed.py).
- Short timeouts, no retries on 404, minimal parsing.
"""

from __future__ import annotations

import gzip
import json
import time
import urllib.error
import urllib.request

SEC_TICKERS_URL = "https://www.sec.gov/files/company_tickers.json"
SEC_SUBMISSIONS_URL = "https://data.sec.gov/submissions/CIK{cik:010d}.json"
SEC_ARCHIVE_URL = "https://www.sec.gov/Archives/edgar/data/{cik}/{nodash}/{doc}"

# SEC blocks requests without a descriptive User-Agent. Replace the email
# with your own contact before heavy use.
USER_AGENT = "FNS-Scraper/0.1 (contact: hello@example.com)"
TIMEOUT_S = 10


def http_get_json(url: str, timeout: float = TIMEOUT_S) -> tuple[dict | list | None, int, int]:
    """GET url, return (parsed_json_or_None, http_status, elapsed_ms)."""
    req = urllib.request.Request(
        url,
        headers={
            "User-Agent": USER_AGENT,
            "Accept": "application/json",
            "Accept-Encoding": "gzip",
        },
    )
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            status = resp.status
            raw = resp.read()
            if resp.headers.get("Content-Encoding") == "gzip":
                raw = gzip.decompress(raw)
            # Some endpoints omit the header but still send gzip.
            elif raw[:2] == b"\x1f\x8b":
                raw = gzip.decompress(raw)
    except urllib.error.HTTPError as e:
        elapsed = int((time.perf_counter() - start) * 1000)
        return None, e.code, elapsed
    except Exception:
        elapsed = int((time.perf_counter() - start) * 1000)
        return None, 0, elapsed
    elapsed = int((time.perf_counter() - start) * 1000)
    try:
        return json.loads(raw.decode("utf-8")), status, elapsed
    except Exception:
        return None, status, elapsed


def load_ticker_map() -> tuple[dict[str, dict], int]:
    """Fetch SEC ticker->CIK map. Returns ({TICKER: {cik, name}}, elapsed_ms)."""
    data, _status, elapsed = http_get_json(SEC_TICKERS_URL)
    mapping: dict[str, dict] = {}
    if isinstance(data, dict):
        for _k, v in data.items():
            try:
                ticker = str(v["ticker"]).upper()
                mapping[ticker] = {"cik": int(v["cik_str"]), "name": str(v["title"])}
            except (KeyError, TypeError, ValueError):
                continue
    elif isinstance(data, list):
        for v in data:
            try:
                ticker = str(v["ticker"]).upper()
                mapping[ticker] = {"cik": int(v["cik_str"]), "name": str(v["title"])}
            except (KeyError, TypeError, ValueError):
                continue
    return mapping, elapsed


def fetch_submissions(cik: int, timeout: float = TIMEOUT_S) -> tuple[dict | None, int, int]:
    """Fetch data.sec.gov submissions JSON for one CIK."""
    url = SEC_SUBMISSIONS_URL.format(cik=cik)
    data, status, elapsed = http_get_json(url, timeout=timeout)
    if not isinstance(data, dict):
        return None, status, elapsed
    return data, status, elapsed


def recent_filings(submissions: dict, limit: int = 8) -> list[dict]:
    """Flatten submissions['filings']['recent'] column-arrays into row dicts."""
    try:
        recent = submissions["filings"]["recent"]
    except (KeyError, TypeError):
        return []
    keys = ("accessionNumber", "filingDate", "reportDate", "form",
            "primaryDocument", "primaryDocDescription", "items")
    cols: dict[str, list] = {}
    n = 0
    for k in keys:
        v = recent.get(k, [])
        cols[k] = v if isinstance(v, list) else []
        if k == "accessionNumber":
            n = len(cols[k])
    out: list[dict] = []
    cik = submissions.get("cik", "")
    name = submissions.get("name", "")
    for i in range(min(n, limit)):
        try:
            acc = str(cols["accessionNumber"][i])
        except IndexError:
            continue

        def _at(k: str) -> str:
            try:
                return str(cols[k][i])
            except IndexError:
                return ""

        doc = _at("primaryDocument")
        nodash = acc.replace("-", "")
        try:
            cik_int = int(str(cik))
        except ValueError:
            cik_int = 0
        url = SEC_ARCHIVE_URL.format(cik=cik_int, nodash=nodash, doc=doc) if doc else ""
        out.append({
            "accession": acc,
            "cik": str(cik),
            "company": str(name),
            "form": _at("form"),
            "filingDate": _at("filingDate"),
            "reportDate": _at("reportDate"),
            "primaryDocument": doc,
            "primaryDocDescription": _at("primaryDocDescription"),
            "items": _at("items"),
            "url": url,
        })
    return out
