//! Deterministic classifier: wire text (or SEC fields) -> category.
//!
//! No LLM, no guessing: the same input always yields the same category.
//! Headlines are passed through verbatim from the wire — we tag them,
//! we never rewrite numbers into them.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Class {
    pub category: &'static str,
    pub importance: &'static str,
    pub detail: String,
}

fn cls(category: &'static str, importance: &'static str, detail: &str) -> Class {
    Class { category, importance, detail: detail.to_string() }
}

/// Word tokens of a text, uppercased. Used for short keywords where a
/// substring match would false-positive (e.g. "eps" in "steps").
pub fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_uppercase())
        .collect()
}

/// Match watchlist tickers against a headline + summary.
///
/// Short tickers (META, F, T, C...) also occur as plain English words, so a
/// bare token hit on a ticker of <= 4 chars additionally requires either a
/// company-name alias hit or one finance-context keyword. Longer tickers
/// match on the token alone.
pub fn match_tickers(
    text: &str,
    watch: &[String],
    aliases: &[(String, String)],
) -> Vec<String> {
    const FIN_CTX: &[&str] = &[
        "EARNINGS", "REVENUE", "SHARES", "STOCK", "NASDAQ", "NYSE", "EPS",
        "DIVIDEND", "MERGER", "GUIDANCE", "QUARTER", "PROFIT", "FILING",
        "RESULTS", "OUTLOOK", "BUYBACK", "SPLIT", "OFFERING", "CEO", "CFO",
    ];
    let up = text.to_uppercase();
    let toks = tokens(&up);
    let mut out: Vec<String> = Vec::new();
    for t in watch {
        if !toks.contains(t.as_str()) {
            continue;
        }
        if t.len() <= 4 {
            let alias_hit = aliases.iter().any(|(a, sym)| sym == t && up.contains(a.as_str()));
            let ctx_hit = FIN_CTX.iter().any(|w| toks.contains(*w));
            if !(alias_hit || ctx_hit) {
                continue;
            }
        }
        out.push(t.clone());
    }
    for (alias, sym) in aliases {
        if up.contains(alias.as_str()) && !out.contains(sym) {
            out.push(sym.clone());
        }
    }
    out
}

/// Classify a wire headline + summary.
///
/// Correctness rule: HIGH urgency needs the signal in the TITLE. Analysis
/// pieces bury "earnings" in paragraph 6 — those become medium "mention"s.
/// Press-release titles always carry the news, so nothing real is lost.
pub fn classify_wire(title: &str, summary: &str) -> Class {
    let lower = format!("{title} {summary}").to_lowercase();
    let t_lower = title.to_lowercase();
    let t_toks = tokens(title);
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    let title_has = |words: &[&str]| words.iter().any(|w| t_lower.contains(w));
    let title_tok = |words: &[&str]| words.iter().any(|w| t_toks.contains(*w));

    // Earnings first — highest priority, title-gated.
    if title_has(&["earnings", "earnings per share", "earnings release"])
        || (t_lower.contains("results")
            && (t_lower.contains("quarter")
                || t_lower.contains("full-year")
                || t_lower.contains("full year")
                || t_lower.contains("fiscal")
                || title_tok(&["Q1", "Q2", "Q3", "Q4"])))
        || title_tok(&["EPS"])
    {
        return cls("earnings", "high", "Earnings / quarterly results");
    }
    if has(&["earnings", "earnings per share"]) {
        return cls("earnings", "medium", "Earnings mention (analysis)");
    }
    if has(&["guidance", "outlook", "forecast"]) && has(&["raise", "lower", "cut", "reaffirm", "update", "issue", "provide", "expects"]) {
        return cls("guidance", "medium", "Guidance / outlook");
    }
    if has(&["merger", "acquisition", "tender offer", "takeover", "definitive agreement"])
        || (lower.contains("acquir") && has(&["announce", "agree", "complet", "propos"]))
    {
        return cls("mna", "high", "M&A");
    }
    // C-suite turnover is high urgency; routine board votes are not.
    if has(&["ceo", "cfo", "chief executive", "chief financial", "chairman"])
        && has(&["appoint", "name", "elect", "resign", "step down", "depart", "succeed", "successor", "retire", "terminate", "ceo", "cfo"])
    {
        return cls("leadership", "high", "Executive change");
    }
    if has(&["director", "board"]) && has(&["elect", "appoint", "resign", "depart", "vote"]) {
        return cls("leadership", "medium", "Board change");
    }
    if has(&["dividend", "buyback", "share repurchase", "stock split", "offering", "shelf registration"]) {
        return cls("capital", "medium", "Capital action");
    }
    if has(&["fda", "clinical", "phase 3", "phase iii", "approval", "clearance"]) {
        return cls("regulatory", "medium", "FDA / clinical");
    }
    cls("filing", "low", "Wire headline")
}

/// Classify from structured SEC fields (confirm tier). Same filing ->
/// same category, every time.
pub fn classify_sec(form: &str, items: &str, desc: &str) -> Class {
    let has_item = |code: &str| {
        items.replace(';', ",").split(',').any(|p| p.trim() == code)
    };
    let d = desc.to_lowercase();
    if has_item("2.02") {
        return cls("earnings", "high", "Results of Operations (Item 2.02)");
    }
    match form {
        "10-Q" | "10-Q/A" => return cls("earnings", "high", "Quarterly results (10-Q)"),
        "10-K" | "10-K/A" => return cls("earnings", "high", "Annual results (10-K)"),
        "20-F" | "20-F/A" => return cls("earnings", "high", "Foreign annual results (20-F)"),
        _ => {}
    }
    if d.contains("earn") || d.contains("results") || d.contains("press release") {
        if form.starts_with("8-K") {
            return cls("earnings", "high", "Earnings press release (EX-99.1)");
        }
        return cls("earnings", "medium", "Results-related filing");
    }
    if d.contains("guidance") || d.contains("outlook") || d.contains("forecast") {
        return cls("guidance", "medium", "Guidance / outlook");
    }
    if has_item("5.02") {
        return cls("leadership", "high", "Director/officer change (Item 5.02)");
    }
    if d.contains("merger") || d.contains("acquis") || d.contains("tender") {
        return cls("mna", "medium", "M&A-related");
    }
    if d.contains("dividend") || d.contains("buyback") || d.contains("repurchase") || d.contains("offering") {
        return cls("capital", "medium", "Capital action");
    }
    if form.starts_with("8-K") {
        if has_item("7.01") || has_item("8.01") {
            return cls("regulatory", "medium", "Reg FD / material event (7.01/8.01)");
        }
        return cls("regulatory", "low", "8-K current report");
    }
    cls("filing", "low", "SEC filing")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earnings_headline_is_earnings_high() {
        let c = classify_wire("Apple Reports Fourth Quarter Results", "Revenue of $89.5 billion, EPS of $1.64");
        assert_eq!((c.category, c.importance), ("earnings", "high"));
    }

    #[test]
    fn plain_english_meta_does_not_match_ticker_without_context() {
        let watch = vec!["META".to_string()];
        let hits = match_tickers("A meta-analysis of market structure", &watch, &[]);
        assert!(hits.is_empty());
    }

    #[test]
    fn ticker_with_finance_context_matches() {
        let watch = vec!["META".to_string()];
        let hits = match_tickers("META reports third-quarter earnings beat", &watch, &[]);
        assert_eq!(hits, vec!["META".to_string()]);
    }

    #[test]
    fn alias_matches_company_name() {
        let watch = vec!["AAPL".to_string()];
        let aliases = vec![("APPLE INC".to_string(), "AAPL".to_string())];
        let hits = match_tickers("Apple Inc. declares cash dividend", &watch, &aliases);
        assert_eq!(hits, vec!["AAPL".to_string()]);
    }

    #[test]
    fn sec_202_is_earnings() {
        let c = classify_sec("8-K", "2.02,9.01", "8-K");
        assert_eq!(c.category, "earnings");
    }

    #[test]
    fn board_vote_is_not_high_urgency() {
        let c = classify_wire("8-K - Current report", "election of directors at annual meeting");
        assert_eq!((c.category, c.importance), ("leadership", "medium"));
        let c = classify_wire("Acme names new CEO", "board appoints Jane Doe as chief executive");
        assert_eq!((c.category, c.importance), ("leadership", "high"));
    }

    #[test]
    fn analysis_piece_is_not_high_urgency() {
        let c = classify_wire(
            "The Real Question Behind CrowdStrike Stock's Premium Price",
            "earnings growth could justify the valuation over time",
        );
        assert_eq!((c.category, c.importance), ("earnings", "medium"));
        let c = classify_wire("Acme Q3 earnings beat estimates", "EPS $1.20 vs $1.05");
        assert_eq!((c.category, c.importance), ("earnings", "high"));
    }

    #[test]
    fn deterministic() {
        let a = classify_wire("Tesla Q3 earnings beat", "EPS $0.66 vs $0.60 est");
        let b = classify_wire("Tesla Q3 earnings beat", "EPS $0.66 vs $0.60 est");
        assert_eq!((a.category, a.importance), (b.category, b.importance));
    }
}
