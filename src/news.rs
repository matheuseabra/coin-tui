//! RSS headline feed provider for the news pane.
//!
//! The production provider reads a conventional RSS 2.0 feed over HTTPS and
//! normalizes items into bounded `NewsItem` values at the provider boundary.
//! It is configured through the same URL rules as the market provider (HTTPS,
//! or raw HTTP only for loopback hosts) so the fixture server can serve an
//! RSS fixture fully offline.

use std::{future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Url;

use crate::{
    api::ApiError,
    http::{validate_url, HttpClient},
};

const MAX_FEED_BYTES: usize = 1024 * 1024;
/// Headlines shown per refresh, so a chatty feed cannot grow the pane state.
const MAX_NEWS_ITEMS: usize = 15;
const MAX_TITLE_CHARS: usize = 220;
const MAX_SOURCE_CHARS: usize = 28;
const MAX_CATEGORY_CHARS: usize = 32;
const MAX_URL_CHARS: usize = 300;

/// One normalized headline. All text is provider-blob and is sanitized and
/// bounded before it leaves this module.
#[derive(Clone, Debug, PartialEq)]
pub struct NewsItem {
    title: String,
    source: String,
    category: String,
    url: String,
    published_at: Option<DateTime<Utc>>,
}

impl NewsItem {
    /// Test-only constructor for feed state that has not crossed the provider
    /// boundary (the production path builds items inside `parse_rss`).
    #[cfg(test)]
    pub fn fixture(title: &str, source: &str, url: &str) -> Self {
        Self {
            title: title.to_owned(),
            source: source.to_owned(),
            category: source.to_owned(),
            url: url.to_owned(),
            published_at: None,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }
    #[allow(dead_code)]
    pub fn source(&self) -> &str {
        &self.source
    }
    #[allow(dead_code)]
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn category(&self) -> &str {
        &self.category
    }
    pub fn published_at(&self) -> Option<DateTime<Utc>> {
        self.published_at
    }
}

/// Provider-independent async boundary for the news feed.
pub trait NewsProvider: Send + Sync {
    fn fetch_headlines<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NewsItem>, ApiError>> + Send + 'a>>;
}

/// A provider that never serves news; test and loop helpers use it when the
/// app runs without a feed.
#[cfg(test)]
pub struct NoNewsProvider;

#[cfg(test)]
impl NewsProvider for NoNewsProvider {
    fn fetch_headlines<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NewsItem>, ApiError>> + Send + 'a>> {
        Box::pin(async move { Err(ApiError::HttpStatus { status: 404 }) })
    }
}

pub struct RssNewsClient {
    client: HttpClient,
    feed_url: Url,
}

impl RssNewsClient {
    pub fn new(feed_url: &str) -> Result<Self, ApiError> {
        Self::with_timeouts(feed_url, Duration::from_secs(10), Duration::from_secs(30))
    }

    pub fn with_timeouts(
        feed_url: &str,
        connect_timeout: Duration,
        total_timeout: Duration,
    ) -> Result<Self, ApiError> {
        let feed_url = validate_url(feed_url)?;
        let client = HttpClient::new(connect_timeout, total_timeout)?;
        Ok(Self { client, feed_url })
    }

    pub async fn fetch_headlines(&self) -> Result<Vec<NewsItem>, ApiError> {
        let body = self.fetch_body().await?;
        let (items, saw_feed) = parse_rss(&body);
        if !saw_feed {
            return Err(ApiError::MalformedResponse);
        }
        Ok(items.into_iter().take(MAX_NEWS_ITEMS).collect())
    }

    async fn fetch_body(&self) -> Result<Vec<u8>, ApiError> {
        self.client
            .get(self.feed_url.clone(), &[], None, MAX_FEED_BYTES, false)
            .await
    }
}

impl NewsProvider for RssNewsClient {
    fn fetch_headlines<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<NewsItem>, ApiError>> + Send + 'a>> {
        Box::pin(RssNewsClient::fetch_headlines(self))
    }
}

/// Parse an RSS 2.0 (or generic feed) document into bounded headlines. Returns
/// the items and whether a recognizable feed root was seen, so an HTML error
/// page cannot masquerade as an empty feed.
fn parse_rss(body: &[u8]) -> (Vec<NewsItem>, bool) {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut saw_feed = false;
    let mut channel_title = String::new();
    let mut items: Vec<NewsItem> = Vec::new();
    let mut in_item = false;
    let mut element = Vec::new();
    let mut title = String::new();
    let mut link = String::new();
    let mut pub_date = String::new();
    let mut category = String::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) => {
                let qualified = start.name();
                let name = qualified.as_ref();
                match name {
                    b"rss" | b"feed" => saw_feed = true,
                    b"item" => {
                        in_item = true;
                        title.clear();
                        link.clear();
                        pub_date.clear();
                        category.clear();
                    }
                    _ => {}
                }
                element = name.to_vec();
            }
            Ok(Event::Text(text)) => {
                if let Ok(value) = text.unescape() {
                    let value = value.into_owned();
                    if in_item {
                        append_item_field(
                            &element,
                            &value,
                            &mut title,
                            &mut link,
                            &mut pub_date,
                            &mut category,
                        );
                    } else if element == b"title" && channel_title.is_empty() {
                        channel_title = trim_control(&value);
                    }
                }
            }
            Ok(Event::CData(cdata)) => {
                let value = String::from_utf8_lossy(&cdata.into_inner()).into_owned();
                if in_item {
                    append_item_field(
                        &element,
                        &value,
                        &mut title,
                        &mut link,
                        &mut pub_date,
                        &mut category,
                    );
                } else if element == b"title" && channel_title.is_empty() {
                    channel_title = trim_control(&value);
                }
            }
            Ok(Event::End(end)) => {
                let qualified = end.name();
                let name = qualified.as_ref();
                if name == b"item" && !trim_control(&title).is_empty() {
                    items.push(NewsItem {
                        title: bound_text(&title, MAX_TITLE_CHARS),
                        source: bound_text(&channel_title, MAX_SOURCE_CHARS),
                        category: bound_text(
                            if category.is_empty() {
                                &channel_title
                            } else {
                                &category
                            },
                            MAX_CATEGORY_CHARS,
                        ),
                        url: bound_text(&link, MAX_URL_CHARS),
                        published_at: parse_rfc2822(&pub_date),
                    });
                    in_item = false;
                }
                element.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        if items.len() >= MAX_NEWS_ITEMS {
            break;
        }
    }
    (items, saw_feed)
}

fn append_item_field(
    element: &[u8],
    value: &str,
    title: &mut String,
    link: &mut String,
    pub_date: &mut String,
    category: &mut String,
) {
    match element {
        b"title" if title.is_empty() => *title = value.to_owned(),
        b"link" if link.is_empty() => *link = value.to_owned(),
        b"pubDate" if pub_date.is_empty() => *pub_date = value.to_owned(),
        b"category" if category.is_empty() => *category = value.to_owned(),
        _ => {}
    }
}

fn parse_rfc2822(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

/// Strip every control character (RSS text is single-line) but keep the
/// scalar value intact otherwise.
fn trim_control(raw: &str) -> String {
    raw.chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// Sanitize and bound one text field, capping the scalar count.
fn bound_text(raw: &str, max_chars: usize) -> String {
    trim_control(raw).chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Wire</title>
    <item>
      <title><![CDATA[Bitcoin rises ]]></title>
      <category>Markets</category>
      <link>https://example.com/stories/bitcoin-rises</link>
      <pubDate>Tue, 18 Aug 2026 14:41:31 +0000</pubDate>
    </item>
    <item>
      <title>Ethereum falls &amp; settles</title>
      <link>https://example.com/stories/ethereum-settles</link>
      <pubDate>Mon, 17 Aug 2026 09:00:00 +0000</pubDate>
    </item>
  </channel>
</rss>
"#;

    #[test]
    fn parses_cdata_and_escaped_titles_with_channel_source() {
        let (items, saw_feed) = parse_rss(MINIMAL_RSS.as_bytes());
        assert!(saw_feed);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title(), "Bitcoin rises ");
        assert_eq!(items[0].source(), "Test Wire");
        assert_eq!(items[0].category(), "Markets");
        assert_eq!(items[0].url(), "https://example.com/stories/bitcoin-rises");
        assert!(items[0].published_at().is_some());
        assert_eq!(items[1].title(), "Ethereum falls & settles");
    }

    #[test]
    fn caps_items_and_sanitizes_control_characters() {
        let mut body = String::from("<rss><channel><title>W</title>");
        for index in 0..40 {
            body.push_str(&format!(
                "<item><title>Item \u{1b}[31m{index}</title><link>https://x.test/{index}</link></item>"
            ));
        }
        body.push_str("</channel></rss>");
        let (items, saw_feed) = parse_rss(body.as_bytes());
        assert!(saw_feed);
        assert_eq!(items.len(), MAX_NEWS_ITEMS);
        assert!(
            !items[0].title().contains('\u{1b}'),
            "{:?}",
            items[0].title()
        );
    }

    #[test]
    fn html_body_is_not_a_feed_and_malformed_xml_yields_no_items() {
        let (items, saw_feed) = parse_rss(b"<html><body>error</body></html>");
        assert!(!saw_feed);
        assert!(items.is_empty());
    }

    #[test]
    fn rejects_invalid_feed_urls_like_the_market_provider() {
        for bad in [
            "ftp://example.com/feed",
            "not a url",
            "http://example.com/feed",
            "https://user:pass@example.com/feed",
        ] {
            assert!(RssNewsClient::new(bad).is_err(), "{bad} should be rejected");
        }
        assert!(RssNewsClient::new("https://example.com/feed").is_ok());
        assert!(RssNewsClient::new("http://127.0.0.1:8787/feed").is_ok());
    }

    #[test]
    fn parses_rfc2822_into_utc() {
        let parsed = parse_rfc2822("Tue, 18 Aug 2026 14:41:31 +0000").unwrap();
        assert_eq!(parsed.to_rfc3339(), "2026-08-18T14:41:31+00:00");
        assert!(parse_rfc2822("not a date").is_none());
    }

    #[test]
    fn feed_without_items_and_missing_item_fields_are_tolerated() {
        let (items, saw_feed) =
            parse_rss(b"<rss version=\"2.0\"><channel><title>Empty Wire</title></channel></rss>");
        assert!(saw_feed);
        assert!(items.is_empty());

        // An item with no title (only a description) is dropped; an item with
        // a title but no link or date still yields a bounded headline.
        let body = br#"<rss version="2.0"><channel><title>Wire</title>
            <item><title></title><link>https://x.test/1</link></item>
            <item><title>No link or date</title></item>
            <item><description>only a description</description></item>
        </channel></rss>"#;
        let (items, saw_feed) = parse_rss(body);
        assert!(saw_feed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title(), "No link or date");
        assert_eq!(items[0].url(), "");
        assert!(items[0].published_at().is_none());
    }

    #[test]
    fn atom_root_is_recognized_but_only_rss_items_are_collected() {
        // A `<feed>` root is recognized as a feed (so an Atom feed is not
        // mistaken for an HTML error page), but the parser only extracts RSS
        // `<item>` entries; Atom `<entry>` elements are ignored.
        let body = br#"<feed xmlns="http://www.w3.org/2005/Atom">
            <title>Atom Wire</title>
            <entry><title>Bitcoin &amp; Ethereum settle</title>
                <link href="https://x.test/entry"/>
                <updated>2026-08-18T10:00:00Z</updated>
            </entry>
        </feed>"#;
        let (items, saw_feed) = parse_rss(body);
        assert!(saw_feed);
        assert!(items.is_empty());
    }

    #[test]
    fn bound_text_caps_scalars_and_trim_control_strips_terminal_escape() {
        let long = "ab\u{1b}[31mc".repeat(50);
        let bounded = bound_text(&long, MAX_TITLE_CHARS);
        assert_eq!(bounded.chars().count(), MAX_TITLE_CHARS);
        assert!(!bounded.contains('\u{1b}'), "control char must be stripped");
        assert_eq!(trim_control("ok\u{0}\u{7f}fine"), "okfine");
        assert_eq!(bound_text("", 10), "");
    }
}
