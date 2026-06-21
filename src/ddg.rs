//! DuckDuckGo Instant Answer API client.
//!
//! Uses the official, ToS-friendly JSON endpoint at `api.duckduckgo.com`.
//! This returns instant answers (abstracts, definitions, related topics) rather
//! than a ranked list of web links — so "open full results" links out to the
//! regular DuckDuckGo results page in the browser.

use std::time::Duration;

use serde::Deserialize;

/// A single related topic / link extracted from the API response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    pub label: String,
    pub url: String,
}

/// Normalized, UI-friendly view of an instant answer.
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
    /// Scrollable related topics.
    pub related: Vec<Topic>,
}

impl SearchResult {
    /// True when the API returned no usable instant answer or topics.
    pub fn is_empty(&self) -> bool {
        self.heading.is_empty()
            && self.answer_text.is_empty()
            && self.related.is_empty()
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
    let (answer_text, source, answer_url) = if !raw.Answer.is_empty() {
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

/// Perform a blocking fetch + parse for a query. Intended to be called from a
/// worker thread.
pub fn fetch(query: &str) -> Result<SearchResult, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("ddg-spotlight/0.1 (+https://duckduckgo.com)")
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("client error: {e}"))?;

    let body = client
        .get(api_url(query))
        .send()
        .map_err(|e| format!("network error: {e}"))?
        .error_for_status()
        .map_err(|e| format!("http error: {e}"))?
        .text()
        .map_err(|e| format!("read error: {e}"))?;

    parse_response(&body).map_err(|e| format!("parse error: {e}"))
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
    }
}
