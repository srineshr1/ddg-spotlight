//! DuckDuckGo clients.
//!
//! Two data sources are combined for a query:
//!   * the official, ToS-friendly Instant Answer JSON endpoint at
//!     `api.duckduckgo.com` — abstracts, definitions and related topics; and
//!   * the no-JS HTML endpoint at `html.duckduckgo.com/html` — a ranked list of
//!     web result links (title + URL + snippet), Google-style.
//!
//! [`fetch_all`] runs both concurrently and merges them into one
//! [`SearchResult`]: an answer block on top, ranked links below.

use std::time::Duration;

use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde::Deserialize;

/// A browser-like User-Agent. The HTML endpoint returns an empty/blocked body
/// to clients that don't look like a real browser.
const BROWSER_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0";

/// Maximum number of ranked web links to keep from one HTML page.
const MAX_WEB_LINKS: usize = 10;

/// A single related topic / link extracted from the Instant Answer API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub label: String,
    pub url: String,
}

/// A ranked web result link (Google-style) scraped from the DuckDuckGo HTML
/// endpoint: title, destination URL, snippet, and a display domain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebLink {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub domain: String,
}

/// Normalized, UI-friendly view of a search: an optional instant answer plus a
/// ranked list of web links.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResult {
    /// Title of the instant answer, if any.
    pub heading: String,
    /// The main answer/abstract/definition text, if any.
    pub answer_text: String,
    /// Source label (e.g. "Wikipedia"), if any.
    pub source: String,
    /// Canonical URL for the abstract/answer, if any.
    pub answer_url: String,
    /// Related topics from the Instant Answer API.
    pub related: Vec<Topic>,
    /// Ranked web result links (Google-style) from the HTML endpoint.
    pub links: Vec<WebLink>,
    /// True when `answer_text` is DuckDuckGo's direct `Answer` (e.g. a
    /// calculation/conversion) rather than an abstract/definition. Direct
    /// answers are never replaced by the featured-snippet fallback.
    pub answer_is_direct: bool,
}

impl SearchResult {
    /// True when there is no usable instant answer, related topic, or web link.
    pub fn is_empty(&self) -> bool {
        self.heading.is_empty()
            && self.answer_text.is_empty()
            && self.related.is_empty()
            && self.links.is_empty()
    }
}

/// Raw API response shape (only the fields we use).
#[allow(non_snake_case)]
#[derive(Debug, Default, Deserialize)]
struct RawResponse {
    #[serde(default)]
    Heading: String,
    #[serde(default)]
    AbstractText: String,
    #[serde(default)]
    AbstractSource: String,
    #[serde(default)]
    AbstractURL: String,
    #[serde(default)]
    Answer: String,
    #[serde(default)]
    Definition: String,
    #[serde(default)]
    DefinitionSource: String,
    #[serde(default)]
    DefinitionURL: String,
    #[serde(default)]
    RelatedTopics: Vec<RawRelated>,
}

/// RelatedTopics entries are either a topic or a named group of topics.
#[allow(non_snake_case)]
#[derive(Debug, Default, Deserialize)]
struct RawRelated {
    #[serde(default)]
    Text: String,
    #[serde(default)]
    FirstURL: String,
    /// Present when this entry is a group ("Topics") rather than a leaf topic.
    #[serde(default)]
    Topics: Vec<RawRelated>,
}

/// Parse a raw JSON body into a normalized [`SearchResult`].
pub fn parse_response(body: &str) -> Result<SearchResult, serde_json::Error> {
    let raw: RawResponse = serde_json::from_str(body)?;
    Ok(normalize(raw))
}

fn normalize(raw: RawResponse) -> SearchResult {
    // Prefer an explicit Answer, then Abstract, then Definition.
    let answer_is_direct = !raw.Answer.is_empty();
    let (answer_text, source, answer_url) = if answer_is_direct {
        (raw.Answer, raw.AbstractSource, raw.AbstractURL)
    } else if !raw.AbstractText.is_empty() {
        (raw.AbstractText, raw.AbstractSource, raw.AbstractURL)
    } else if !raw.Definition.is_empty() {
        (raw.Definition, raw.DefinitionSource, raw.DefinitionURL)
    } else {
        (String::new(), String::new(), String::new())
    };

    let mut related = Vec::new();
    flatten_related(&raw.RelatedTopics, &mut related);

    SearchResult {
        heading: raw.Heading,
        answer_text,
        source,
        answer_url,
        related,
        links: Vec::new(),
        answer_is_direct,
    }
}

/// Recursively flatten the (possibly nested) RelatedTopics into leaf topics.
fn flatten_related(items: &[RawRelated], out: &mut Vec<Topic>) {
    for item in items {
        if !item.Topics.is_empty() {
            flatten_related(&item.Topics, out);
        } else if !item.FirstURL.is_empty() && !item.Text.is_empty() {
            out.push(Topic {
                label: item.Text.clone(),
                url: item.FirstURL.clone(),
            });
        }
    }
}

/// Build the Instant Answer API URL for a query.
pub fn api_url(query: &str) -> String {
    format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        percent_encode(query)
    )
}

/// Build the human-facing DuckDuckGo results URL (opened in the browser).
pub fn results_url(query: &str) -> String {
    format!("https://duckduckgo.com/?q={}", percent_encode(query))
}

/// Build the DuckDuckGo HTML (no-JS) search URL used to scrape ranked links.
pub fn web_url(query: &str) -> String {
    format!("https://html.duckduckgo.com/html/?q={}", percent_encode(query))
}

/// Minimal percent-encoding for query strings (RFC 3986 unreserved set kept).
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Decode percent-encoding (`%XX`) and `+`-as-space. Invalid escapes are kept
/// literally; the result is lossily decoded as UTF-8.
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h << 4) | l);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Resolve a DuckDuckGo result `href` into a real destination URL.
///
/// DDG wraps results in a redirect like `//duckduckgo.com/l/?uddg=<encoded>`;
/// we extract and percent-decode the `uddg` parameter. Direct `http(s)` links
/// are returned as-is. Anything else (relative/internal) yields `None`.
pub fn decode_ddg_href(href: &str) -> Option<String> {
    if href.is_empty() {
        return None;
    }
    // Normalize protocol-relative URLs (`//host/...`).
    let normalized = match href.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => href.to_string(),
    };

    // DDG redirect wrapper: pull out the `uddg` target.
    if let Some(idx) = normalized.find("uddg=") {
        let rest = &normalized[idx + "uddg=".len()..];
        let encoded = rest.split('&').next().unwrap_or("");
        let decoded = percent_decode(encoded);
        return if decoded.is_empty() { None } else { Some(decoded) };
    }

    if normalized.starts_with("http://") || normalized.starts_with("https://") {
        Some(normalized)
    } else {
        None
    }
}

/// Extract a display domain (no scheme, no `www.`, no port) from a URL.
pub fn extract_domain(url: &str) -> String {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    let host = no_scheme.split('/').next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host); // strip any userinfo
    let host = host.split(':').next().unwrap_or(host); // strip port
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Parse a DuckDuckGo HTML results page into ranked [`WebLink`]s. Ads and
/// internal DuckDuckGo links are skipped.
pub fn parse_web_results(html: &str) -> Vec<WebLink> {
    let doc = Html::parse_document(html);
    // Selectors are static and always valid.
    let result_sel = Selector::parse("div.result").unwrap();
    let title_sel = Selector::parse("a.result__a").unwrap();
    let snippet_sel = Selector::parse(".result__snippet").unwrap();

    let mut out: Vec<WebLink> = Vec::new();
    for el in doc.select(&result_sel) {
        let classes = el.value().attr("class").unwrap_or("");
        // Skip sponsored results and "more results" rows.
        if classes.contains("result--ad") || classes.contains("result--more") {
            continue;
        }

        let anchor = match el.select(&title_sel).next() {
            Some(a) => a,
            None => continue,
        };
        let title = collapse_ws(&anchor.text().collect::<String>());
        let url = match anchor.value().attr("href").and_then(decode_ddg_href) {
            Some(u) => u,
            None => continue,
        };
        if title.is_empty() || url.is_empty() {
            continue;
        }
        let domain = extract_domain(&url);
        // Drop internal DDG links (ads/redirately things that slipped through).
        if domain == "duckduckgo.com" || domain.ends_with(".duckduckgo.com") {
            continue;
        }

        let snippet = el
            .select(&snippet_sel)
            .next()
            .map(|s| collapse_ws(&s.text().collect::<String>()))
            .unwrap_or_default();

        out.push(WebLink {
            title,
            url,
            snippet,
            domain,
        });
        if out.len() >= MAX_WEB_LINKS {
            break;
        }
    }
    out
}

/// Collapse runs of whitespace into single spaces and trim the ends.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build one reusable blocking HTTP client. Built once per worker and shared
/// across every request, so we don't pay TLS/root-cert/connection setup on each
/// keystroke (that was the main cause of slow suggestions). Per-request timeouts
/// are set on each call instead of a single client-wide timeout.
pub fn build_client() -> Client {
    Client::builder()
        .user_agent(BROWSER_UA)
        .pool_idle_timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// Perform a blocking Instant Answer fetch + parse for a query.
pub fn fetch(client: &Client, query: &str) -> Result<SearchResult, String> {
    let body = client
        .get(api_url(query))
        .timeout(Duration::from_secs(8))
        .send()
        .map_err(|e| format!("network error: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http error: {e}"))?
        .text()
        .map_err(|e| format!("read error: {e}"))?;

    parse_response(&body).map_err(|e| format!("parse error: {e}"))
}

/// Fetch a ranked list of web links from the DuckDuckGo HTML endpoint.
pub fn fetch_web(client: &Client, query: &str) -> Result<Vec<WebLink>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let body = client
        .get(web_url(query))
        .timeout(Duration::from_secs(8))
        .send()
        .map_err(|e| format!("network error: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http error: {e}"))?
        .text()
        .map_err(|e| format!("read error: {e}"))?;

    Ok(parse_web_results(&body))
}

/// Heuristic: does this query ask for a *specific* fact — who/where/when/which
/// or a "how many/much/…" quantity — where DuckDuckGo's topic abstract tends to
/// be the wrong granularity (it describes the office/topic) or is simply empty?
/// Such queries are answered far better by the top web result's snippet.
///
/// Definitional "what is …" and open "why/how …" queries are intentionally
/// excluded: the Instant Answer abstract/definition is good for those.
fn is_factoid_question(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    const LEADERS: &[&str] = &[
        "who ", "whom ", "whose ", "where ", "when ", "which ",
    ];
    if LEADERS.iter().any(|p| q.starts_with(p)) {
        return true;
    }
    const QUANTITIES: &[&str] = &[
        "how many ",
        "how much ",
        "how old ",
        "how tall ",
        "how far ",
        "how long ",
        "how high ",
        "how deep ",
    ];
    QUANTITIES.iter().any(|p| q.starts_with(p))
}

/// For factoid questions without a direct DuckDuckGo answer, promote the top web
/// result (title + snippet + domain) into the answer block — a featured-snippet
/// style answer, since the topic abstract is usually generic or missing here.
fn apply_featured_answer(result: &mut SearchResult, query: &str) {
    if result.answer_is_direct || !is_factoid_question(query) {
        return;
    }
    if let Some(link) = result.links.iter().find(|l| !l.snippet.is_empty()) {
        result.heading = link.title.clone();
        result.answer_text = link.snippet.clone();
        result.source = link.domain.clone();
        result.answer_url = link.url.clone();
    }
}

/// Fetch both the Instant Answer (definition/abstract) and the ranked web links
/// for a query, merging them into a single [`SearchResult`]. The two network
/// calls run concurrently (sharing the connection pool). Returns an error only
/// if *both* fail. For factoid questions the top web result is promoted into the
/// answer block (see [`apply_featured_answer`]).
pub fn fetch_all(client: &Client, query: &str) -> Result<SearchResult, String> {
    let web_client = client.clone();
    let q_web = query.to_string();
    let web_handle = std::thread::spawn(move || fetch_web(&web_client, &q_web));
    let answer = fetch(client, query);
    let web = web_handle
        .join()
        .unwrap_or_else(|_| Err("web worker panicked".to_string()));

    let mut result = match (answer, web) {
        (Ok(mut result), Ok(links)) => {
            result.links = links;
            result
        }
        (Ok(result), Err(_)) => result,
        (Err(_), Ok(links)) => SearchResult {
            links,
            ..Default::default()
        },
        (Err(answer_err), Err(web_err)) => return Err(format!("{web_err} / {answer_err}")),
    };
    apply_featured_answer(&mut result, query);
    Ok(result)
}

/// Fetch autocomplete suggestions from DuckDuckGo.
/// Returns an empty vec on any failure (network, parse, etc).
pub fn suggest(client: &Client, query: &str) -> Vec<String> {
    if query.trim().is_empty() {
        return vec![];
    }
    let url = format!("https://duckduckgo.com/ac/?q={}", percent_encode(query));
    let body = match client.get(&url).timeout(Duration::from_secs(3)).send() {
        Ok(r) => match r.error_for_status().and_then(|r| r.text()) {
            Ok(t) => t,
            Err(_) => return vec![],
        },
        Err(_) => return vec![],
    };
    let raw: Vec<serde_json::Value> = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    raw.iter()
        .filter_map(|v| v.get("phrase").and_then(|p| p.as_str()).map(String::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_abstract_answer() {
        let json = r#"{
            "Heading": "Rust (programming language)",
            "AbstractText": "Rust is a multi-paradigm programming language.",
            "AbstractSource": "Wikipedia",
            "AbstractURL": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "RelatedTopics": []
        }"#;
        let r = parse_response(json).unwrap();
        assert_eq!(r.heading, "Rust (programming language)");
        assert_eq!(r.answer_text, "Rust is a multi-paradigm programming language.");
        assert_eq!(r.source, "Wikipedia");
        assert!(!r.is_empty());
    }

    #[test]
    fn prefers_answer_field() {
        let json = r#"{
            "Answer": "42",
            "AbstractText": "ignored because Answer is present",
            "RelatedTopics": []
        }"#;
        let r = parse_response(json).unwrap();
        assert_eq!(r.answer_text, "42");
        assert!(r.answer_is_direct);
    }

    #[test]
    fn detects_factoid_questions() {
        assert!(is_factoid_question("who is pm of india"));
        assert!(is_factoid_question("Where is the Eiffel Tower"));
        assert!(is_factoid_question("how many feet in a mile"));
        assert!(is_factoid_question("Which planet is the largest"));
        // Definitional / open questions keep the Instant Answer abstract.
        assert!(!is_factoid_question("what is photosynthesis"));
        assert!(!is_factoid_question("rust programming language"));
        assert!(!is_factoid_question("how to write a for loop"));
        assert!(!is_factoid_question("why is the sky blue"));
    }

    fn link(snippet: &str) -> WebLink {
        WebLink {
            title: "Top Result".into(),
            url: "https://top.example/x".into(),
            snippet: snippet.into(),
            domain: "top.example".into(),
        }
    }

    #[test]
    fn featured_answer_promotes_top_snippet_for_factoids() {
        let mut r = SearchResult {
            heading: "Prime Minister of India".into(),
            answer_text: "The prime minister of India is the head of government.".into(),
            source: "Wikipedia".into(),
            links: vec![link("Narendra Modi was sworn in as PM in 2024.")],
            ..Default::default()
        };
        apply_featured_answer(&mut r, "who is pm of india");
        assert_eq!(r.answer_text, "Narendra Modi was sworn in as PM in 2024.");
        assert_eq!(r.source, "top.example");
        assert_eq!(r.heading, "Top Result");
    }

    #[test]
    fn featured_answer_used_when_abstract_empty() {
        let mut r = SearchResult {
            links: vec![link("Elon Musk is the CEO of Tesla.")],
            ..Default::default()
        };
        apply_featured_answer(&mut r, "who is the ceo of tesla");
        assert_eq!(r.answer_text, "Elon Musk is the CEO of Tesla.");
    }

    #[test]
    fn featured_answer_keeps_direct_answer() {
        let mut r = SearchResult {
            answer_text: "5280 feet".into(),
            answer_is_direct: true,
            links: vec![link("A mile is a unit of length.")],
            ..Default::default()
        };
        apply_featured_answer(&mut r, "how many feet in a mile");
        assert_eq!(r.answer_text, "5280 feet"); // not clobbered
    }

    #[test]
    fn featured_answer_skips_non_factoid() {
        let mut r = SearchResult {
            answer_text: "Rust is a programming language.".into(),
            links: vec![link("something else")],
            ..Default::default()
        };
        apply_featured_answer(&mut r, "rust programming language");
        assert_eq!(r.answer_text, "Rust is a programming language."); // unchanged
    }

    #[test]
    fn falls_back_to_definition() {
        let json = r#"{
            "Definition": "a thing",
            "DefinitionSource": "Dictionary",
            "DefinitionURL": "https://example.com",
            "RelatedTopics": []
        }"#;
        let r = parse_response(json).unwrap();
        assert_eq!(r.answer_text, "a thing");
        assert_eq!(r.source, "Dictionary");
        assert_eq!(r.answer_url, "https://example.com");
    }

    #[test]
    fn flattens_nested_related_topics() {
        let json = r#"{
            "RelatedTopics": [
                { "Text": "Top level topic", "FirstURL": "https://a.example" },
                { "Topics": [
                    { "Text": "Nested one", "FirstURL": "https://b.example" },
                    { "Text": "Nested two", "FirstURL": "https://c.example" }
                ]}
            ]
        }"#;
        let r = parse_response(json).unwrap();
        assert_eq!(r.related.len(), 3);
        assert_eq!(r.related[0].label, "Top level topic");
        assert_eq!(r.related[2].url, "https://c.example");
    }

    #[test]
    fn empty_response_is_empty() {
        let r = parse_response(r#"{"RelatedTopics":[]}"#).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn links_make_result_non_empty() {
        let mut r = SearchResult::default();
        assert!(r.is_empty());
        r.links.push(WebLink {
            title: "t".into(),
            url: "https://e.example".into(),
            snippet: String::new(),
            domain: "e.example".into(),
        });
        assert!(!r.is_empty());
    }

    #[test]
    fn skips_related_without_url_or_text() {
        let json = r#"{
            "RelatedTopics": [
                { "Text": "", "FirstURL": "https://a.example" },
                { "Text": "no url", "FirstURL": "" },
                { "Text": "good", "FirstURL": "https://good.example" }
            ]
        }"#;
        let r = parse_response(json).unwrap();
        assert_eq!(r.related.len(), 1);
        assert_eq!(r.related[0].label, "good");
    }

    #[test]
    fn encodes_query_for_urls() {
        assert_eq!(percent_encode("rust lang"), "rust%20lang");
        assert_eq!(percent_encode("c++"), "c%2B%2B");
        assert_eq!(percent_encode("a.b-c_d~e"), "a.b-c_d~e");
        assert!(api_url("hello world").contains("q=hello%20world"));
        assert!(results_url("a&b").contains("q=a%26b"));
        assert!(web_url("rust lang").starts_with("https://html.duckduckgo.com/html/?q=rust%20lang"));
    }

    #[test]
    fn decodes_percent_encoding() {
        assert_eq!(percent_decode("rust%20lang"), "rust lang");
        assert_eq!(percent_decode("a%2Bb"), "a+b");
        assert_eq!(percent_decode("https%3A%2F%2Fa.com%2Fx"), "https://a.com/x");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("a+b"), "a b");
        // Malformed escapes are preserved literally.
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn decodes_ddg_redirect_href() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=xyz";
        assert_eq!(
            decode_ddg_href(href),
            Some("https://www.rust-lang.org/".to_string())
        );
    }

    #[test]
    fn decode_href_passthrough_and_reject() {
        assert_eq!(
            decode_ddg_href("https://example.com/x"),
            Some("https://example.com/x".to_string())
        );
        assert_eq!(decode_ddg_href(""), None);
        assert_eq!(decode_ddg_href("/about"), None);
    }

    #[test]
    fn extracts_display_domain() {
        assert_eq!(extract_domain("https://www.rust-lang.org/learn"), "rust-lang.org");
        assert_eq!(extract_domain("http://docs.rs:443/foo"), "docs.rs");
        assert_eq!(extract_domain("https://en.wikipedia.org/wiki/Rust"), "en.wikipedia.org");
    }

    #[test]
    fn parses_web_results_and_skips_ads() {
        let html = r##"
        <div class="result results_links results_links_deep web-result">
          <div class="links_main">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=abc">The Rust Programming Language</a>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F">A language empowering everyone to build reliable and efficient software.</a>
          </div>
        </div>
        <div class="result result--ad">
          <div class="links_main">
            <a class="result__a" href="//duckduckgo.com/y.js?ad_provider=foo">Buy Rust Now</a>
          </div>
        </div>
        <div class="result results_links">
          <div class="links_main">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F">The Rust Book</a>
            <a class="result__snippet">Read   the    book.</a>
          </div>
        </div>
        "##;
        let links = parse_web_results(html);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].title, "The Rust Programming Language");
        assert_eq!(links[0].url, "https://www.rust-lang.org/");
        assert_eq!(links[0].domain, "rust-lang.org");
        assert!(links[0].snippet.starts_with("A language empowering"));
        assert_eq!(links[1].url, "https://doc.rust-lang.org/book/");
        assert_eq!(links[1].domain, "doc.rust-lang.org");
        // Whitespace in the snippet is collapsed.
        assert_eq!(links[1].snippet, "Read the book.");
    }
}
