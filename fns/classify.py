"""Deterministic classifier: structured SEC fields -> news category.

No LLM, no guessing: the same filing always yields the same category.
Full-text exhibit parsing (EX-99.1) is deliberately OUT of the hot path
for speed; we classify on form + 8-K items + doc description, which is
enough to catch earnings and other market-moving filings with ~zero
false positives.

Categories: earnings | guidance | mna | leadership | capital | regulatory | filing
Importance: high | medium | low
"""

from __future__ import annotations

EARNINGS_FORMS = {"10-Q", "10-K", "10-Q/A", "10-K/A", "20-F", "20-F/A"}
HIGH = "high"
MEDIUM = "medium"
LOW = "low"


def _has_item(items: str, code: str) -> bool:
    # items looks like "2.02,7.01,9.01" — substring match on "2" would
    # false-positive, so normalize to a padded set.
    parts = {p.strip() for p in (items or "").replace(";", ",").split(",")}
    return code in parts


def classify(filing: dict) -> dict:
    form = (filing.get("form") or "").strip()
    items = (filing.get("items") or "").strip()
    desc = ((filing.get("primaryDocDescription") or "") + " " +
            (filing.get("primaryDocument") or "")).lower()

    # 1. Earnings — highest priority.
    if _has_item(items, "2.02"):
        return _r("earnings", HIGH, "Results of Operations (Item 2.02)")
    if form in EARNINGS_FORMS:
        kind = "Annual results (10-K)" if "10-K" in form else \
               "Foreign annual results (20-F)" if "20-F" in form else \
               "Quarterly results (10-Q)"
        return _r("earnings", HIGH, kind)
    if "earn" in desc or "results" in desc or "99.1" in desc or "press release" in desc:
        if form.startswith("8-K"):
            return _r("earnings", HIGH, "Earnings press release (EX-99.1)")
        return _r("earnings", MEDIUM, "Results-related filing")

    # 2. Guidance (often 8-K 7.01/8.01 with outlook language).
    if any(w in desc for w in ("guidance", "outlook", "forecast", "reaffirm")):
        return _r("guidance", MEDIUM, "Guidance / outlook")

    # 3. M&A — Item 1.01/1.02/2.01 + deal language.
    if (_has_item(items, "1.01") or _has_item(items, "1.02") or _has_item(items, "2.01")) \
            and any(w in desc for w in ("merger", "acquis", "tender", "takeover", "definitive agreement")):
        return _r("mna", HIGH, "M&A agreement")
    if any(w in desc for w in ("merger", "acquisition", "tender offer")):
        return _r("mna", MEDIUM, "M&A-related")

    # 4. Leadership — Item 5.02.
    if _has_item(items, "5.02"):
        return _r("leadership", HIGH, "Director/officer change (Item 5.02)")

    # 5. Capital: dividends, buybacks, splits, offerings.
    if any(w in desc for w in ("dividend", "buyback", "repurchase", "stock split", "offering", "shelf")):
        return _r("capital", MEDIUM, "Capital action (dividend/buyback/offering)")
    if _has_item(items, "3.02") or _has_item(items, "3.03") or _has_item(items, "1.01"):
        return _r("capital", LOW, "Corporate action")

    # 6. Regulatory / disclosure catch-all for notable 8-Ks.
    if form.startswith("8-K"):
        if _has_item(items, "7.01") or _has_item(items, "8.01"):
            return _r("regulatory", MEDIUM, "Reg FD / material event (7.01/8.01)")
        return _r("regulatory", LOW, "8-K current report")

    return _r("filing", LOW, f"{form or 'SEC'} filing")


def _r(category: str, importance: str, detail: str) -> dict:
    return {"category": category, "importance": importance, "detail": detail}


def build_headline(ticker: str, company: str, filing: dict, cls: dict) -> str:
    form = filing.get("form", "?")
    date = filing.get("filingDate", "")
    if cls["category"] == "earnings":
        return f"{ticker} earnings — {cls['detail']} filed {date}"
    if cls["category"] == "guidance":
        return f"{ticker} guidance update — {cls['detail']} ({form}, {date})"
    if cls["category"] == "mna":
        return f"{ticker} M&A — {cls['detail']} ({form}, {date})"
    if cls["category"] == "leadership":
        return f"{ticker} leadership change — {cls['detail']} ({form}, {date})"
    if cls["category"] == "capital":
        return f"{ticker} capital action — {cls['detail']} ({form}, {date})"
    short = company[:40] if company else ticker
    return f"{ticker} ({short}) — {form} filed {date}"
