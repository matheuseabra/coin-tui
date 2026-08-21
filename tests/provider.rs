#[path = "../src/api.rs"]
mod api;
#[path = "../src/domain.rs"]
mod domain;
#[path = "../src/http.rs"]
mod http;
#[path = "../src/news.rs"]
mod news;

use chrono::{DateTime, Utc};
use std::{
    io::{Read, Write},
    time::{Duration, SystemTime},
};

use api::{ApiError, CoinGeckoClient};
use wiremock::{
    matchers::{header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

const BODY: &str = r#"[{"id":"bitcoin","name":"Bitcoin","symbol":"btc","market_cap_rank":1,"current_price":50000,"price_change_percentage_1h_in_currency":0.1,"price_change_percentage_24h":1.2,"price_change_percentage_7d_in_currency":-2,"market_cap":1000000,"total_volume":25000,"circulating_supply":19,"sparkline_in_7d":{"price":[1,2,null,3]},"last_updated":"2023-11-14T22:13:20Z"}]"#;
const GLOBAL: &str = r#"{"data":{"total_market_cap":{"usd":2000000},"total_volume":{"usd":50000},"market_cap_percentage":{"btc":51.5},"market_cap_change_percentage_24h_usd":2.5,"updated_at":1700000001}}"#;

async fn fetch_via_market_data(
    provider: &impl api::MarketData,
) -> Result<api::FetchOutcome, ApiError> {
    provider.fetch_snapshot().await
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes(), "application/json")
}

fn client(server: &MockServer, key: Option<String>) -> CoinGeckoClient {
    CoinGeckoClient::with_timeouts(
        &server.uri(),
        key,
        Duration::from_millis(100),
        Duration::from_secs(1),
    )
    .unwrap()
}

#[tokio::test]
async fn requests_exact_markets_query_and_optional_key_and_converts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/markets"))
        .and(query_param("vs_currency", "usd"))
        .and(query_param("order", "market_cap_desc"))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .and(query_param("sparkline", "true"))
        .and(query_param("price_change_percentage", "1h,24h,7d"))
        .and(header("x-cg-demo-api-key", "secret-key"))
        .respond_with(json(BODY))
        .mount(&server)
        .await;
    let snapshot = client(&server, Some("secret-key".into()))
        .fetch_markets()
        .await
        .unwrap();
    let coin = &snapshot.coins()[0];
    assert_eq!(coin.id(), "bitcoin");
    assert_eq!(coin.price(), Some(50000.0));
    assert_eq!(coin.change_7d(), Some(-2.0));
    assert_eq!(coin.name(), "Bitcoin");
    assert_eq!(coin.symbol(), "btc");
    assert_eq!(coin.change_1h(), Some(0.1));
    assert_eq!(coin.change_24h(), Some(1.2));
    assert_eq!(coin.market_cap(), Some(1_000_000.0));
    assert_eq!(coin.volume_24h(), Some(25_000.0));
    assert_eq!(coin.circulating_supply(), Some(19.0));
    assert_eq!(coin.sparkline_7d(), &[1.0, 2.0, 3.0]);
    assert_eq!(
        snapshot.provider_updated_at(),
        Some(
            DateTime::parse_from_rfc3339("2023-11-14T22:13:20Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
    let request = &server.received_requests().await.unwrap()[0];
    let mut query: Vec<_> = request.url.query().unwrap().split('&').collect();
    query.sort_unstable();
    assert_eq!(
        query,
        vec![
            "order=market_cap_desc",
            "page=1",
            "per_page=100",
            "price_change_percentage=1h%2C24h%2C7d",
            "sparkline=true",
            "vs_currency=usd"
        ]
    );
    assert_eq!(request.headers.get("user-agent").unwrap(), "coin-tui/0.1");
}

#[tokio::test]
async fn converts_successful_empty_response_to_empty_snapshot() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json("[]"))
        .mount(&server)
        .await;

    let snapshot = client(&server, None).fetch_markets().await.unwrap();

    assert!(snapshot.coins().is_empty());
}

#[tokio::test]
async fn omits_key_and_classifies_failures_without_secret_in_display() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/markets"))
        .respond_with(json(BODY))
        .mount(&server)
        .await;
    client(&server, None).fetch_markets().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("x-cg-demo-api-key"));
    let malformed = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json("secret-key-in-body"))
        .mount(&malformed)
        .await;
    let error = client(&malformed, Some("never-display".into()))
        .fetch_markets()
        .await
        .unwrap_err();
    assert_eq!(error, ApiError::MalformedResponse);
    assert!(!error.to_string().contains("never-display"));
    assert!(!error.to_string().contains("secret-key-in-body"));
    assert!(!format!("{error:?}").contains("never-display"));
    assert!(!format!("{error:?}").contains("secret-key-in-body"));
}

#[tokio::test]
async fn accepts_missing_optional_provider_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json(r#"[{"id":"unknown","name":"Unknown","symbol":"?"}]"#))
        .mount(&server)
        .await;
    let snapshot = client(&server, None).fetch_markets().await.unwrap();
    let coin = &snapshot.coins()[0];
    assert!(coin.rank().is_none() && coin.price().is_none() && coin.sparkline_7d().is_empty());
}

#[tokio::test]
async fn converts_explicit_null_optional_fields_and_sparkline_entries_to_missing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json(
            r#"[{"id":"unknown","name":"Unknown","symbol":"?","market_cap_rank":null,"current_price":null,"price_change_percentage_1h_in_currency":null,"price_change_percentage_24h":null,"price_change_percentage_7d_in_currency":null,"market_cap":null,"total_volume":null,"circulating_supply":null,"sparkline_in_7d":{"price":[null,null]},"last_updated":null}]"#,
        ))
        .mount(&server)
        .await;

    let snapshot = client(&server, None).fetch_markets().await.unwrap();
    let coin = &snapshot.coins()[0];

    assert_eq!(coin.rank(), None);
    assert_eq!(coin.price(), None);
    assert_eq!(coin.change_1h(), None);
    assert_eq!(coin.change_24h(), None);
    assert_eq!(coin.change_7d(), None);
    assert_eq!(coin.market_cap(), None);
    assert_eq!(coin.volume_24h(), None);
    assert_eq!(coin.circulating_supply(), None);
    assert!(coin.sparkline_7d().is_empty());
    assert_eq!(snapshot.provider_updated_at(), None);
}

#[tokio::test]
async fn classifies_timeout_rate_limit_and_server_error() {
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json(BODY).set_delay(Duration::from_millis(200)))
        .mount(&slow)
        .await;
    let timeout_client = CoinGeckoClient::with_timeouts(
        &slow.uri(),
        None,
        Duration::from_millis(100),
        Duration::from_millis(50),
    )
    .unwrap();
    assert_eq!(
        timeout_client.fetch_markets().await.unwrap_err(),
        ApiError::Timeout
    );
    let limited = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
        .mount(&limited)
        .await;
    assert_eq!(
        client(&limited, None).fetch_markets().await.unwrap_err(),
        ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(7))
        }
    );
    let failed = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&failed)
        .await;
    assert_eq!(
        client(&failed, None).fetch_markets().await.unwrap_err(),
        ApiError::HttpStatus { status: 503 }
    );
}

#[test]
fn rejects_total_timeout_that_overflows_tokio_instant_without_exposing_value() {
    let result = CoinGeckoClient::with_timeouts(
        "http://localhost",
        None,
        Duration::from_secs(1),
        Duration::MAX,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("maximum total timeout must be rejected"),
    };

    assert_eq!(error, ApiError::InvalidTimeoutConfiguration);
    assert_eq!(error.to_string(), "invalid API timeout configuration");
    assert_eq!(format!("{error:?}"), "InvalidTimeoutConfiguration");
}

#[tokio::test]
async fn accepts_zero_total_timeout_as_an_immediate_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json(BODY).set_delay(Duration::from_millis(20)))
        .mount(&server)
        .await;
    let api = CoinGeckoClient::with_timeouts(
        &server.uri(),
        None,
        Duration::from_millis(100),
        Duration::ZERO,
    )
    .unwrap();

    assert_eq!(api.fetch_markets().await.unwrap_err(), ApiError::Timeout);
}

#[tokio::test]
async fn classifies_delayed_body_after_headers_as_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (headers_sent, headers_received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        headers_sent.send(()).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        let _ = stream.write_all(b"[]");
    });

    let timeout_client = CoinGeckoClient::with_timeouts(
        &format!("http://{address}"),
        None,
        Duration::from_secs(1),
        Duration::from_millis(250),
    )
    .unwrap();

    let request = tokio::spawn(async move { timeout_client.fetch_markets().await });
    tokio::task::spawn_blocking(move || headers_received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.await.unwrap().unwrap_err(), ApiError::Timeout);
}

#[tokio::test]
async fn delayed_global_body_after_headers_keeps_coin_rows_and_timestamp() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            std::thread::spawn(move || {
                let mut request = [0; 4096];
                let size = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..size]);
                let (body, delay) = if request.starts_with("GET /api/v3/global") {
                    (GLOBAL, true)
                } else {
                    (BODY, false)
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                stream.flush().unwrap();
                if delay {
                    std::thread::sleep(Duration::from_millis(500));
                }
                let _ = stream.write_all(body.as_bytes());
            });
        }
    });

    let api = CoinGeckoClient::with_timeouts(
        &format!("http://{address}"),
        None,
        Duration::from_millis(100),
        Duration::from_millis(100),
    )
    .unwrap();
    let snapshot = api.fetch_snapshot().await.unwrap().snapshot;

    assert_eq!(snapshot.coins().len(), 1);
    assert_eq!(snapshot.coins()[0].id(), "bitcoin");
    assert_eq!(snapshot.summary().total_market_cap(), None);
    assert_eq!(
        snapshot.provider_updated_at(),
        Some(
            DateTime::parse_from_rfc3339("2023-11-14T22:13:20Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
    server.join().unwrap();
}

#[test]
fn rejects_non_https_non_localhost_base_urls() {
    assert!(matches!(
        CoinGeckoClient::new("http://example.com", None),
        Err(ApiError::InvalidBaseUrl)
    ));
    for url in [
        "http://localhost.evil",
        "http://user@localhost",
        "http://127.0.0.2",
        "http://[::2]",
    ] {
        assert!(
            matches!(
                CoinGeckoClient::new(url, None),
                Err(ApiError::InvalidBaseUrl)
            ),
            "{url}"
        );
    }
    assert!(CoinGeckoClient::new("http://LOCALHOST", None).is_ok());
    assert!(CoinGeckoClient::new("http://127.0.0.1", None).is_ok());
    assert!(CoinGeckoClient::new("http://[::1]", None).is_ok());
}

#[tokio::test]
async fn rejects_redirect_without_sending_key_to_destination() {
    let destination = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json(BODY))
        .mount(&destination)
        .await;
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", destination.uri()))
        .mount(&origin)
        .await;
    assert_eq!(
        client(&origin, Some("secret".into()))
            .fetch_markets()
            .await
            .unwrap_err(),
        ApiError::HttpStatus { status: 302 }
    );
    assert_eq!(destination.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn enforces_json_type_and_streaming_size_limit() {
    let plain = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/plain")
                .set_body_string(BODY),
        )
        .mount(&plain)
        .await;
    assert_eq!(
        client(&plain, None).fetch_markets().await.unwrap_err(),
        ApiError::MalformedResponse
    );
    let huge = MockServer::start().await;
    // A body well past the 2 MiB response cap that also carries the key text.
    let body = format!(
        "[{}]",
        "\"secret-key-in-oversize\","
            .repeat(300_000)
            .trim_end_matches(',')
    );
    Mock::given(method("GET"))
        .respond_with(json(&body))
        .mount(&huge)
        .await;
    let error = client(&huge, Some("header-secret".into()))
        .fetch_markets()
        .await
        .unwrap_err();
    assert_eq!(error, ApiError::MalformedResponse);
    let display = error.to_string();
    let debug = format!("{error:?}");
    assert!(!display.contains("header-secret"), "display leaked the key");
    assert!(!display.contains("secret-key-in-oversize"));
    assert!(!debug.contains("header-secret"), "debug leaked the key");
    assert!(!debug.contains("secret-key-in-oversize"));
}

#[tokio::test]
async fn hostile_control_characters_pass_through_the_provider_boundary_for_render_sanitization() {
    let server = MockServer::start().await;
    let body = r#"[{"id":"bitcoin","name":"\u001b[31mBitcoin\u200e\u0000\u0007\u007f","symbol":"\u001b[1mBTC\u200b","market_cap_rank":1,"current_price":50000}]"#;
    Mock::given(method("GET"))
        .respond_with(json(body))
        .mount(&server)
        .await;

    let snapshot = client(&server, None).fetch_markets().await.unwrap();
    let coin = &snapshot.coins()[0];
    assert_eq!(coin.id(), "bitcoin");
    assert!(
        coin.name().contains('\u{001b}') && coin.name().contains('\u{200e}'),
        "provider text reaches the domain raw: {:?}",
        coin.name()
    );
    assert!(
        coin.symbol().contains('\u{001b}') && coin.symbol().contains('\u{200b}'),
        "provider symbol reaches the domain raw"
    );
}

#[tokio::test]
async fn retry_after_supports_dates_and_invalid_values() {
    let past = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "Wed, 21 Oct 2015 07:28:00 GMT"),
        )
        .mount(&past)
        .await;
    assert_eq!(
        client(&past, None).fetch_markets().await.unwrap_err(),
        ApiError::RateLimited {
            retry_after: Some(Duration::ZERO)
        }
    );
    let invalid = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "nope"))
        .mount(&invalid)
        .await;
    assert_eq!(
        client(&invalid, None).fetch_markets().await.unwrap_err(),
        ApiError::RateLimited { retry_after: None }
    );
}

#[tokio::test]
async fn rate_limit_without_retry_after_has_no_delay() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    assert_eq!(
        client(&server, None).fetch_markets().await.unwrap_err(),
        ApiError::RateLimited { retry_after: None }
    );
}

#[tokio::test]
async fn rate_limit_future_http_date_has_positive_tolerant_delay() {
    let server = MockServer::start().await;
    let retry_at = SystemTime::now() + Duration::from_secs(5);
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", httpdate::fmt_http_date(retry_at)),
        )
        .mount(&server)
        .await;

    let ApiError::RateLimited { retry_after } =
        client(&server, None).fetch_markets().await.unwrap_err()
    else {
        panic!("expected rate limit error");
    };
    let delay = retry_after.expect("future HTTP date should produce a delay");
    assert!(delay > Duration::ZERO, "delay was {delay:?}");
    assert!(delay <= Duration::from_secs(6), "delay was {delay:?}");
}

#[tokio::test]
async fn secret_bearing_rate_limit_and_server_errors_are_redacted() {
    let secret = "super-secret-api-key";
    for status in [429, 503] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_string(format!("provider details contain {secret}")),
            )
            .mount(&server)
            .await;

        let error = client(&server, Some(secret.into()))
            .fetch_markets()
            .await
            .unwrap_err();
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert!(
            !display.contains(secret),
            "display leaked for HTTP {status}"
        );
        assert!(!display.contains("provider details"));
        assert!(!debug.contains(secret), "debug leaked for HTTP {status}");
        assert!(!debug.contains("provider details"));
    }
}

#[tokio::test]
async fn reports_connection_failure_without_exposing_endpoint_or_key() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let uri = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let error = CoinGeckoClient::with_timeouts(
        &uri,
        Some("connection-secret".into()),
        Duration::from_millis(100),
        Duration::from_secs(1),
    )
    .unwrap()
    .fetch_markets()
    .await
    .unwrap_err();
    assert_eq!(error, ApiError::Transport);
    assert!(!error.to_string().contains("connection-secret"));
    assert!(!format!("{error:?}").contains("connection-secret"));
}

#[tokio::test]
async fn uses_oldest_valid_timestamp_and_ignores_invalid_values() {
    let server = MockServer::start().await;
    let body = r#"[{"id":"new","name":"N","symbol":"n","last_updated":"2024-01-01T00:00:00Z"},{"id":"bad","name":"B","symbol":"b","last_updated":"bad"},{"id":"old","name":"O","symbol":"o","last_updated":"2023-01-01T00:00:00Z"}]"#;
    Mock::given(method("GET"))
        .respond_with(json(body))
        .mount(&server)
        .await;
    assert_eq!(
        client(&server, None)
            .fetch_markets()
            .await
            .unwrap()
            .provider_updated_at(),
        Some(
            DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
}

#[tokio::test]
async fn combined_snapshot_fetches_global_with_key_and_composes_summary_timestamp() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/markets"))
        .and(header("x-cg-demo-api-key", "secret-key"))
        .respond_with(json(BODY))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/global"))
        .and(header("x-cg-demo-api-key", "secret-key"))
        .respond_with(json(GLOBAL))
        .mount(&server)
        .await;

    let snapshot = client(&server, Some("secret-key".into()))
        .fetch_snapshot()
        .await
        .unwrap()
        .snapshot;
    assert_eq!(snapshot.coins().len(), 1);
    assert_eq!(snapshot.summary().total_market_cap(), Some(2_000_000.0));
    assert_eq!(snapshot.summary().total_volume_24h(), Some(50_000.0));
    assert_eq!(snapshot.summary().btc_dominance(), Some(51.5));
    assert_eq!(snapshot.summary().market_cap_change_24h(), Some(2.5));
    assert_eq!(
        snapshot.provider_updated_at(),
        Some(
            DateTime::parse_from_rfc3339("2023-11-14T22:13:20Z")
                .unwrap()
                .with_timezone(&Utc)
        )
    );
    let requests = server.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/api/v3/coins/markets"));
    assert!(requests
        .iter()
        .any(|request| request.url.path() == "/api/v3/global"));
    let global_request = requests
        .iter()
        .find(|request| request.url.path() == "/api/v3/global")
        .unwrap();
    assert_eq!(global_request.url.query(), None);
    assert_eq!(
        global_request.headers.get("x-cg-demo-api-key").unwrap(),
        "secret-key"
    );
}

#[tokio::test]
async fn market_data_trait_exposes_a_sendable_future() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/markets"))
        .respond_with(json(BODY))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/global"))
        .respond_with(json(GLOBAL))
        .mount(&server)
        .await;

    let outcome = fetch_via_market_data(&client(&server, None)).await.unwrap();
    assert_eq!(outcome.snapshot.coins().len(), 1);
    assert_eq!(outcome.summary_notice, None);
}

#[tokio::test]
async fn global_optional_containers_can_be_missing_or_null_independently() {
    for body in [
        r#"{"data":{"total_market_cap":{"usd":2000000},"market_cap_change_percentage_24h_usd":2.5}}"#,
        r#"{"data":{"total_market_cap":null,"total_volume":null,"market_cap_percentage":null,"market_cap_change_percentage_24h_usd":2.5}}"#,
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/coins/markets"))
            .respond_with(json(BODY))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/global"))
            .respond_with(json(body))
            .mount(&server)
            .await;
        let summary = client(&server, None)
            .fetch_snapshot()
            .await
            .unwrap()
            .snapshot
            .summary()
            .clone();
        assert_eq!(
            summary.total_market_cap(),
            if body.contains("2000000") {
                Some(2_000_000.0)
            } else {
                None
            }
        );
        assert_eq!(summary.total_volume_24h(), None);
        assert_eq!(summary.btc_dominance(), None);
        assert_eq!(summary.market_cap_change_24h(), Some(2.5));
    }
}

#[tokio::test]
async fn timestamp_combination_uses_the_oldest_valid_nonnegative_source() {
    for (global_timestamp, expected) in [
        ("1700000001", "2023-11-14T22:13:20Z"),
        ("1700000000", "2023-11-14T22:13:20Z"),
        ("-1", "2023-11-14T22:13:20Z"),
        ("9223372036854775807", "2023-11-14T22:13:20Z"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/coins/markets"))
            .respond_with(json(BODY))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/global"))
            .respond_with(json(&GLOBAL.replace("1700000001", global_timestamp)))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server, None)
                .fetch_snapshot()
                .await
                .unwrap()
                .snapshot
                .provider_updated_at(),
            Some(
                DateTime::parse_from_rfc3339(expected)
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }
}

#[tokio::test]
async fn summary_failures_keep_coin_rows_and_coin_timestamp() {
    for (response, expected_notice) in [
        (
            ResponseTemplate::new(429),
            ApiError::RateLimited { retry_after: None },
        ),
        (
            ResponseTemplate::new(500),
            ApiError::HttpStatus { status: 500 },
        ),
        (json("not-json"), ApiError::MalformedResponse),
        (
            json(GLOBAL).set_delay(Duration::from_millis(200)),
            ApiError::Timeout,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/coins/markets"))
            .respond_with(json(BODY))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/global"))
            .respond_with(response)
            .mount(&server)
            .await;
        let api = CoinGeckoClient::with_timeouts(
            &server.uri(),
            None,
            Duration::from_millis(100),
            Duration::from_millis(50),
        )
        .unwrap();
        let outcome = api.fetch_snapshot().await.unwrap();
        let snapshot = outcome.snapshot;
        assert_eq!(snapshot.coins().len(), 1);
        assert_eq!(snapshot.summary().total_market_cap(), None);
        assert_eq!(snapshot.summary().total_volume_24h(), None);
        assert_eq!(snapshot.summary().btc_dominance(), None);
        assert_eq!(snapshot.summary().market_cap_change_24h(), None);
        assert_eq!(outcome.summary_notice, Some(expected_notice));
        assert_eq!(
            snapshot.provider_updated_at(),
            Some(
                DateTime::parse_from_rfc3339("2023-11-14T22:13:20Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }
}

#[tokio::test]
async fn global_rate_limit_notice_keeps_rows_and_retry_after_metadata() {
    for (retry_after, expected) in [
        ("7".to_owned(), Some(Duration::from_secs(7))),
        (
            httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(5)),
            None,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/coins/markets"))
            .respond_with(json(BODY))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/global"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", retry_after))
            .mount(&server)
            .await;

        let outcome = client(&server, None).fetch_snapshot().await.unwrap();
        assert_eq!(outcome.snapshot.coins().len(), 1);
        let Some(ApiError::RateLimited { retry_after }) = outcome.summary_notice else {
            panic!("expected global rate-limit notice");
        };
        match expected {
            Some(expected) => assert_eq!(retry_after, Some(expected)),
            None => assert!(retry_after.is_some_and(|delay| {
                delay > Duration::ZERO && delay <= Duration::from_secs(6)
            })),
        }
    }
}

#[tokio::test]
async fn coin_failure_remains_fatal_even_when_summary_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/markets"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/global"))
        .respond_with(json(GLOBAL))
        .mount(&server)
        .await;
    assert_eq!(
        client(&server, None).fetch_snapshot().await.unwrap_err(),
        ApiError::HttpStatus { status: 503 }
    );
}

#[tokio::test]
async fn coin_failure_returns_promptly_after_both_requests_start_and_drops_global() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/markets"))
        .respond_with(ResponseTemplate::new(503).set_delay(Duration::from_millis(250)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/global"))
        .respond_with(json(GLOBAL).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;
    let api = client(&server, None);
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move { api.fetch_snapshot().await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let requests = server.received_requests().await.unwrap();
        if requests
            .iter()
            .any(|request| request.url.path() == "/api/v3/coins/markets")
            && requests
                .iter()
                .any(|request| request.url.path() == "/api/v3/global")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "both requests did not start"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("coin failure should not await delayed global")
        .unwrap();
    assert_eq!(result.unwrap_err(), ApiError::HttpStatus { status: 503 });
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// A rich `/coins/{id}` body with every extended market-data field the
/// CoinMarketCap-style sidebar reads.
const COIN_DETAIL_BODY: &str = r#"{
  "id": "bitcoin",
  "symbol": "btc",
  "name": "Bitcoin",
  "market_cap_rank": 1,
  "categories": ["layer-1", "store-of-value"],
  "description": {"en": "A peer-to-peer network."},
  "market_data": {
    "current_price": {"usd": 50000.0},
    "market_cap": {"usd": 1000000000000.0},
    "fully_diluted_valuation": {"usd": 1100000000000.0},
    "total_volume": {"usd": 25000000000.0},
    "high_24h": {"usd": 52000.0},
    "low_24h": {"usd": 49000.0},
    "ath": {"usd": 100000.0},
    "atl": {"usd": 3000.0},
    "ath_change_percentage": {"usd": -50.0},
    "atl_change_percentage": {"usd": 1500.0},
    "price_change_percentage_1h_in_currency": {"usd": 0.1},
    "price_change_percentage_24h": 2.0,
    "price_change_percentage_7d_in_currency": {"usd": -1.5},
    "price_change_percentage_14d_in_currency": {"usd": 3.0},
    "price_change_percentage_30d_in_currency": {"usd": -2.0},
    "price_change_percentage_60d_in_currency": {"usd": 5.0},
    "price_change_percentage_1y_in_currency": {"usd": 40.0},
    "circulating_supply": 19.0,
    "total_supply": 21.0,
    "max_supply": 21.0,
    "sentiment_votes_up_percentage": 70.0,
    "sentiment_votes_down_percentage": 30.0,
    "sparkline_7d": {"price": [1.0, 2.0, null, 4.0]}
  }
}"#;

#[tokio::test]
async fn fetch_coin_detail_requests_exact_path_query_and_key_and_converts() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/bitcoin"))
        .and(query_param("localization", "false"))
        .and(query_param("tickers", "false"))
        .and(query_param("market_data", "true"))
        .and(query_param("community_data", "false"))
        .and(query_param("developer_data", "false"))
        .and(query_param("sparkline", "true"))
        .and(query_param("vs_currency", "usd"))
        .and(header("x-cg-demo-api-key", "secret-key"))
        .respond_with(json(COIN_DETAIL_BODY))
        .mount(&server)
        .await;

    let detail = client(&server, Some("secret-key".into()))
        .fetch_coin_detail("bitcoin")
        .await
        .unwrap();
    assert_eq!(detail.ath(), Some(100_000.0));
    assert_eq!(detail.atl(), Some(3_000.0));
    assert_eq!(detail.ath_change(), Some(-50.0));
    assert_eq!(detail.atl_change(), Some(1500.0));
    assert_eq!(detail.change_7d(), Some(-1.5));
    assert_eq!(detail.change_30d(), Some(-2.0));
    assert_eq!(detail.change_60d(), Some(5.0));
    assert_eq!(detail.change_1y(), Some(40.0));
    assert_eq!(detail.fully_diluted_valuation(), Some(1_100_000_000_000.0));
    assert_eq!(detail.volume_24h(), Some(25_000_000_000.0));
    assert_eq!(detail.high_24h(), Some(52_000.0));
    assert_eq!(detail.low_24h(), Some(49_000.0));
    assert_eq!(detail.circulating_supply(), Some(19.0));
    assert_eq!(detail.total_supply(), Some(21.0));
    assert_eq!(detail.max_supply(), Some(21.0));
    assert_eq!(detail.sentiment_up(), Some(70.0));
    assert_eq!(detail.sentiment_down(), Some(30.0));
    assert_eq!(detail.sparkline_7d(), &[1.0, 2.0, 4.0]);
    assert_eq!(detail.categories(), &["layer-1", "store-of-value"]);
    assert_eq!(detail.description(), Some("A peer-to-peer network."));

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(request.url.path(), "/api/v3/coins/bitcoin");
    assert_eq!(request.headers.get("user-agent").unwrap(), "coin-tui/0.1");
}

#[tokio::test]
async fn fetch_market_chart_requests_path_query_and_key_and_extracts_prices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/bitcoin/market_chart"))
        .and(query_param("vs_currency", "usd"))
        .and(query_param("days", "30"))
        .and(header("x-cg-demo-api-key", "secret-key"))
        .respond_with(json(
            r#"{"prices":[[1700000000000,50000],[1700036400000,51000],[1700072400000,null],[1700108400000,52000]],"market_caps":[],"total_volumes":[]}"#,
        ))
        .mount(&server)
        .await;

    let prices = client(&server, Some("secret-key".into()))
        .fetch_market_chart("bitcoin")
        .await
        .unwrap();
    assert_eq!(
        prices,
        vec![
            domain::PricePoint {
                timestamp: 1_700_000_000_000.0,
                price: 50000.0
            },
            domain::PricePoint {
                timestamp: 1_700_036_400_000.0,
                price: 51000.0
            },
            domain::PricePoint {
                timestamp: 1_700_108_400_000.0,
                price: 52000.0
            },
        ],
        "null prices are dropped"
    );

    // The MarketData trait exposes the same method for object-safe providers.
    let keyed = client(&server, Some("secret-key".into()));
    let chart = api::MarketData::fetch_market_chart(&keyed, "bitcoin").await;
    assert!(chart.is_ok());
}

#[tokio::test]
async fn fetch_market_chart_percent_encodes_hostile_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/..%2Fadmin/market_chart"))
        .respond_with(json(r#"{"prices":[],"market_caps":[],"total_volumes":[]}"#))
        .mount(&server)
        .await;

    let prices = client(&server, None)
        .fetch_market_chart("../admin")
        .await
        .unwrap();
    assert!(prices.is_empty());
}

#[tokio::test]
async fn fetch_coin_detail_percent_encodes_hostile_ids_and_rejects_missing_market_data() {
    let server = MockServer::start().await;
    // A hostile id cannot smuggle extra path segments or query text.
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/..%2Fadmin"))
        .respond_with(json(COIN_DETAIL_BODY))
        .mount(&server)
        .await;
    let detail = client(&server, None)
        .fetch_coin_detail("../admin")
        .await
        .unwrap();
    assert_eq!(detail.ath(), Some(100_000.0));

    // A coin with no market_data and no description still normalizes to a
    // usable detail with every optional value absent.
    let bare = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/bare"))
        .respond_with(json(r#"{"id":"bare","symbol":"b","name":"Bare"}"#))
        .mount(&bare)
        .await;
    let detail = client(&bare, None).fetch_coin_detail("bare").await.unwrap();
    assert_eq!(detail.market_cap(), None);
    assert_eq!(detail.circulating_supply(), None);
    assert!(detail.sparkline_7d().is_empty());
    assert!(detail.categories().is_empty());
    assert_eq!(detail.description(), None);
}

#[tokio::test]
async fn fetch_coin_detail_normalizes_nullable_values_and_rejects_out_of_range() {
    // An out-of-range number cannot deserialize into f64; the boundary
    // rejects the whole response as malformed instead of leaking a NaN.
    let hostile = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/nan"))
        .respond_with(json(
            r#"{"id":"nan","symbol":"n","name":"N","market_data":{"ath":{"usd":1e999}}}"#,
        ))
        .mount(&hostile)
        .await;
    assert_eq!(
        client(&hostile, None)
            .fetch_coin_detail("nan")
            .await
            .unwrap_err(),
        ApiError::MalformedResponse
    );

    // Nulls in the sparkline normalize to missing values via `.flatten()`.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/coins/nan"))
        .respond_with(json(
            r#"{"id":"nan","symbol":"n","name":"N","market_data":{"ath":{"usd":null},"current_price":{"usd":null},"sparkline_7d":{"price":[1,null,null,2]}}}"#,
        ))
        .mount(&server)
        .await;
    let detail = client(&server, None)
        .fetch_coin_detail("nan")
        .await
        .unwrap();
    assert_eq!(detail.ath(), None);
    assert_eq!(detail.market_cap(), None);
    assert_eq!(detail.sparkline_7d(), &[1.0, 2.0]);

    let limited = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
        .mount(&limited)
        .await;
    assert_eq!(
        client(&limited, None)
            .fetch_coin_detail("any")
            .await
            .unwrap_err(),
        ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(7))
        }
    );
    let failed = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&failed)
        .await;
    assert_eq!(
        client(&failed, None)
            .fetch_coin_detail("any")
            .await
            .unwrap_err(),
        ApiError::HttpStatus { status: 503 }
    );
}

#[tokio::test]
async fn market_data_detail_default_returns_501_for_providers_without_detail() {
    struct RowOnly;
    impl api::MarketData for RowOnly {
        fn fetch_snapshot<'a>(
            &'a self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<api::FetchOutcome, ApiError>> + Send + 'a>,
        > {
            Box::pin(async move { unreachable!("not used in this test") })
        }
    }
    let detail = api::MarketData::fetch_coin_detail(&RowOnly, "bitcoin")
        .await
        .unwrap_err();
    assert_eq!(detail, ApiError::HttpStatus { status: 501 });
}

/// A small sanitized RSS 2.0 feed with a channel title and two items.
const RSS_BODY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Fixture Wire</title>
    <item>
      <title>Bitcoin rises above $100K</title>
      <link>https://example.com/stories/bitcoin-rises</link>
      <pubDate>Tue, 18 Aug 2026 14:41:31 +0000</pubDate>
    </item>
    <item>
      <title>Ethereum settles</title>
      <link>https://example.com/stories/ethereum-settles</link>
      <pubDate>Mon, 17 Aug 2026 09:00:00 +0000</pubDate>
    </item>
  </channel>
</rss>
"#;

fn news_client(server: &MockServer) -> news::RssNewsClient {
    news::RssNewsClient::with_timeouts(
        &format!("{}/rss", server.uri()),
        Duration::from_millis(100),
        Duration::from_secs(1),
    )
    .unwrap()
}

#[tokio::test]
async fn rss_client_fetches_and_normalizes_headlines() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rss"))
        .respond_with(json(RSS_BODY))
        .mount(&server)
        .await;
    let items = news_client(&server).fetch_headlines().await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title(), "Bitcoin rises above $100K");
    assert_eq!(items[0].source(), "Fixture Wire");
    assert_eq!(items[0].url(), "https://example.com/stories/bitcoin-rises");
    assert!(items[0].published_at().is_some());
    assert_eq!(items[1].title(), "Ethereum settles");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn rss_client_rejects_non_rss_and_oversized_bodies_without_panicking() {
    let plain = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json("<html><body>error page</body></html>"))
        .mount(&plain)
        .await;
    assert_eq!(
        news_client(&plain).fetch_headlines().await.unwrap_err(),
        ApiError::MalformedResponse
    );

    let huge = MockServer::start().await;
    // A body well past the 1 MiB feed cap.
    let body = format!(
        "<rss><channel><title>W</title>{}</channel></rss>",
        "<item><title>t</title></item>".repeat(200_000)
    );
    Mock::given(method("GET"))
        .respond_with(json(&body))
        .mount(&huge)
        .await;
    assert_eq!(
        news_client(&huge).fetch_headlines().await.unwrap_err(),
        ApiError::MalformedResponse
    );
}

#[tokio::test]
async fn rss_client_classifies_rate_limit_server_error_and_timeout() {
    let limited = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "5"))
        .mount(&limited)
        .await;
    assert_eq!(
        news_client(&limited).fetch_headlines().await.unwrap_err(),
        ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(5))
        }
    );
    let failed = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&failed)
        .await;
    assert_eq!(
        news_client(&failed).fetch_headlines().await.unwrap_err(),
        ApiError::HttpStatus { status: 503 }
    );
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(json(RSS_BODY).set_delay(Duration::from_millis(200)))
        .mount(&slow)
        .await;
    // A client with a 50 ms total timeout times out on the 200 ms delay.
    let client = news::RssNewsClient::with_timeouts(
        &format!("{}/rss", slow.uri()),
        Duration::from_millis(100),
        Duration::from_millis(50),
    )
    .unwrap();
    assert_eq!(
        client.fetch_headlines().await.unwrap_err(),
        ApiError::Timeout
    );
}

#[tokio::test]
async fn news_provider_trait_is_object_safe_and_never_provider_fails_cleanly() {
    // The production loop calls the trait through a trait object; prove the
    // never-provider (used by loop helpers) surfaces a clean HTTP error.
    let provider: Box<dyn news::NewsProvider> = Box::new(news::NoNewsProvider);
    assert_eq!(
        provider.fetch_headlines().await.unwrap_err(),
        ApiError::HttpStatus { status: 404 }
    );
    // The fixture constructor produces a headline the render path consumes.
    let item = news::NewsItem::fixture("title", "source", "https://x.test/1");
    assert_eq!(item.title(), "title");
    assert_eq!(item.source(), "source");
    assert_eq!(item.url(), "https://x.test/1");
}
