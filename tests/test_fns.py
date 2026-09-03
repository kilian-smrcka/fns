"""Offline tests: classifier + SEC row parsing. No network. Stdlib unittest."""

import unittest

from fns import classify as C
from fns import sec


class TestClassify(unittest.TestCase):
    def test_8k_item_202_is_earnings_high(self):
        f = {"form": "8-K", "items": "2.02,9.01",
             "primaryDocument": "aapl-20260903.htm", "primaryDocDescription": "8-K"}
        c = C.classify(f)
        self.assertEqual(c["category"], "earnings")
        self.assertEqual(c["importance"], "high")

    def test_10q_is_earnings(self):
        c = C.classify({"form": "10-Q", "items": "",
                        "primaryDocument": "msft-10q.htm", "primaryDocDescription": "10-Q"})
        self.assertEqual(c["category"], "earnings")

    def test_item_502_is_leadership(self):
        c = C.classify({"form": "8-K", "items": "5.02",
                        "primaryDocument": "8k.htm", "primaryDocDescription": "8-K"})
        self.assertEqual((c["category"], c["importance"]), ("leadership", "high"))

    def test_item_code_prefix_no_false_positive(self):
        # "2.02" must not match a bare "2" and vice versa.
        self.assertTrue(C._has_item("2.02,9.01", "2.02"))
        self.assertFalse(C._has_item("2.02,9.01", "2"))
        self.assertFalse(C._has_item("7.01", "7"))

    def test_deterministic(self):
        f = {"form": "8-K", "items": "2.02", "primaryDocument": "x.htm",
             "primaryDocDescription": "8-K"}
        self.assertEqual(C.classify(f), C.classify(dict(f)))

    def test_headline_mentions_ticker_and_verifiable(self):
        f = {"form": "8-K", "filingDate": "2026-09-03"}
        cls = {"category": "earnings", "importance": "high", "detail": "Results of Operations (Item 2.02)"}
        hl = C.build_headline("AAPL", "Apple Inc.", f, cls)
        self.assertIn("AAPL", hl)
        self.assertIn("2026-09-03", hl)


class TestSecParse(unittest.TestCase):
    def test_recent_filings_flattens_columns(self):
        subs = {"cik": "320193", "name": "Apple Inc.",
                "filings": {"recent": {
                    "accessionNumber": ["0000320193-26-000001"],
                    "filingDate": ["2026-09-03"],
                    "reportDate": ["2026-09-03"],
                    "form": ["8-K"],
                    "primaryDocument": ["aapl-8k.htm"],
                    "primaryDocDescription": ["8-K"],
                    "items": ["2.02"]}}}
        rows = sec.recent_filings(subs, limit=5)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["accession"], "0000320193-26-000001")
        self.assertIn("Archives/edgar/data/320193/000032019326000001/aapl-8k.htm",
                      rows[0]["url"])

    def test_recent_filings_missing_key_is_empty(self):
        self.assertEqual(sec.recent_filings({}, limit=5), [])


if __name__ == "__main__":
    unittest.main()
