//! CoinGecko market provider and conversion at the provider boundary.

use std::{fmt, future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, TimeZone, Utc};
use reqwest::{header, redirect::Policy, StatusCode, Url};
use serde::Deserialize;

use crate::domain::{CoinMarketInput, MarketSnapshot, MarketSummaryInput};

const MARKETS_PATH: &str = "api/v3/coins/markets";
const GLOBAL_PATH: &str = "api/v3/global";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    InvalidBaseUrl,
    InvalidTimeoutConfiguration,
    Timeout,
    Transport,
    MalformedResponse,
    RateLimited { retry_after: Option<Duration> },
    HttpStatus { status: u16 },
}

/// A successful coin refresh may still have an unavailable optional summary.
#[derive(Clone, Debug, PartialEq)]
pub struct FetchOutcome {
    pub snapshot: MarketSnapshot,
    pub summary_notice: Option<ApiError>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl => formatter.write_str("invalid API base URL"),
            Self::InvalidTimeoutConfiguration => {
                formatter.write_str("invalid API timeout configuration")
            }
            Self::Timeout => formatter.write_str("API request timed out"),
            Self::Transport => formatter.write_str("API transport failed"),
            Self::MalformedResponse => formatter.write_str("API returned malformed JSON"),
            Self::RateLimited { retry_after } => match retry_after {
                Some(delay) => write!(
                    formatter,
                    "API rate limited; retry after {} seconds",
                    delay.as_secs()
                ),
                None => formatter.write_str("API rate limited"),
            },
            Self::HttpStatus { status } => write!(formatter, "API returned HTTP status {status}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// Reusable CoinGecko client. The key is kept only in the request header and
/// is intentionally absent from this type's formatting implementations.
pub struct CoinGeckoClient {
    client: reqwest::Client,
    base_url: Url,
    api_key: Option<String>,
    total_timeout: Duration,
}

impl CoinGeckoClient {
    pub fn new(base_url: &str, api_key: Option<String>) -> Result<Self, ApiError> {
        Self::with_timeouts(
            base_url,
            api_key,
            Duration::from_secs(10),
            Duration::from_secs(30),
        )
    }

    pub fn with_timeouts(
        base_url: &str,
        api_key: Option<String>,
        connect_timeout: Duration,
        total_timeout: Duration,
    ) -> Result<Self, ApiError> {
        let base_url = Url::parse(base_url).map_err(|_| ApiError::InvalidBaseUrl)?;
        let allowed_http =
            base_url.scheme() == "http" && base_url.host().map(is_loopback_host).unwrap_or(false);
        if base_url.scheme() != "https" && !allowed_http
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
        {
            return Err(ApiError::InvalidBaseUrl);
        }
        if tokio::time::Instant::now()
            .checked_add(total_timeout)
            .is_none()
        {
            return Err(ApiError::InvalidTimeoutConfiguration);
        }
        let client = reqwest::Client::builder()
            .user_agent("coin-tui/0.1")
            .redirect(Policy::none())
            .connect_timeout(connect_timeout)
            .build()
            .map_err(|_| ApiError::Transport)?;
        Ok(Self {
            client,
            base_url,
            api_key,
            total_timeout,
        })
    }

    pub async fn fetch_markets(&self) -> Result<MarketSnapshot, ApiError> {
        let coins: Vec<CoinGeckoMarket> = self
            .request_json(
                MARKETS_PATH,
                &[
                    ("vs_currency", "usd"),
                    ("order", "market_cap_desc"),
                    ("per_page", "100"),
                    ("page", "1"),
                    ("sparkline", "true"),
                    ("price_change_percentage", "1h,24h,7d"),
                ],
            )
            .await?;
        convert(coins)
    }

    /// Fetch rows and the optional global summary as one concurrent snapshot.
    pub async fn fetch_snapshot(&self) -> Result<FetchOutcome, ApiError> {
        let coins = self.fetch_markets();
        let global = self.fetch_global();
        tokio::pin!(coins, global);
        let (coins, summary) = tokio::select! {
            coin_result = &mut coins => {
                // A coin failure is fatal. Returning here drops the optional
                // request instead of waiting for it to finish.
                (coin_result?, global.await)
            }
            global_result = &mut global => {
                (coins.await?, global_result)
            }
        };
        Ok(match summary {
            Ok((summary, updated_at)) => FetchOutcome {
                snapshot: coins.with_summary(summary, updated_at),
                summary_notice: None,
            },
            Err(error) => FetchOutcome {
                snapshot: coins,
                summary_notice: Some(error),
            },
        })
    }

    async fn fetch_global(&self) -> Result<(MarketSummaryInput, Option<DateTime<Utc>>), ApiError> {
        let global: CoinGeckoGlobal = self.request_json(GLOBAL_PATH, &[]).await?;
        let updated_at = global.data.updated_at.and_then(|seconds| {
            (seconds >= 0)
                .then(|| Utc.timestamp_opt(seconds, 0).single())
                .flatten()
        });
        Ok((
            MarketSummaryInput {
                total_market_cap: global.data.total_market_cap.and_then(|value| value.usd),
                total_volume_24h: global.data.total_volume.and_then(|value| value.usd),
                btc_dominance: global
                    .data
                    .market_cap_percentage
                    .and_then(|value| value.btc),
                market_cap_change_24h: global.data.market_cap_change_percentage_24h_usd,
            },
            updated_at,
        ))
    }

    async fn request_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, ApiError> {
        let url = self
            .base_url
            .join(path)
            .map_err(|_| ApiError::InvalidBaseUrl)?;
        let mut request = self.client.get(url).query(query);
        if let Some(key) = &self.api_key {
            request = request.header("x-cg-demo-api-key", key);
        }
        match tokio::time::timeout(self.total_timeout, async move {
            let response = request.send().await.map_err(classify_request_error)?;
            let status = response.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                return Err(ApiError::RateLimited {
                    retry_after: parse_retry_after(response.headers()),
                });
            }
            if !status.is_success() {
                return Err(ApiError::HttpStatus {
                    status: status.as_u16(),
                });
            }
            if !is_json_content_type(response.headers()) {
                return Err(ApiError::MalformedResponse);
            }
            let mut body = Vec::new();
            let mut response = response;
            while let Some(chunk) = response.chunk().await.map_err(classify_request_error)? {
                if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
                    return Err(ApiError::MalformedResponse);
                }
                body.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&body).map_err(|_| ApiError::MalformedResponse)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ApiError::Timeout),
        }
    }
}

/// Provider-independent async boundary for application refresh orchestration.
///
/// Implementations must return a cooperative async future: dropping the future
/// must stop its work rather than detach it. The production Reqwest client
/// satisfies this contract because its request future is cancellation-safe.
pub trait MarketData: Send + Sync {
    fn fetch_snapshot<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, ApiError>> + Send + 'a>>;
}

impl MarketData for CoinGeckoClient {
    fn fetch_snapshot<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, ApiError>> + Send + 'a>> {
        Box::pin(CoinGeckoClient::fetch_snapshot(self))
    }
}

fn classify_request_error(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        ApiError::Timeout
    } else {
        ApiError::Transport
    }
}

fn parse_retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(
        date.duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

fn is_json_content_type(headers: &header::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address == std::net::Ipv4Addr::LOCALHOST,
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}

#[derive(Deserialize)]
struct CoinGeckoMarket {
    id: String,
    name: String,
    symbol: String,
    market_cap_rank: Option<u32>,
    current_price: Option<f64>,
    price_change_percentage_1h_in_currency: Option<f64>,
    price_change_percentage_24h: Option<f64>,
    price_change_percentage_7d_in_currency: Option<f64>,
    market_cap: Option<f64>,
    total_volume: Option<f64>,
    circulating_supply: Option<f64>,
    sparkline_in_7d: Option<Sparkline>,
    last_updated: Option<String>,
}

#[derive(Deserialize)]
struct Sparkline {
    price: Option<Vec<Option<f64>>>,
}

#[derive(Deserialize)]
struct CoinGeckoGlobal {
    data: CoinGeckoGlobalData,
}

#[derive(Deserialize)]
struct CoinGeckoGlobalData {
    total_market_cap: Option<CurrencyValue>,
    total_volume: Option<CurrencyValue>,
    market_cap_percentage: Option<BtcValue>,
    market_cap_change_percentage_24h_usd: Option<f64>,
    updated_at: Option<i64>,
}

#[derive(Deserialize)]
struct CurrencyValue {
    usd: Option<f64>,
}

#[derive(Deserialize)]
struct BtcValue {
    btc: Option<f64>,
}

fn convert(coins: Vec<CoinGeckoMarket>) -> Result<MarketSnapshot, ApiError> {
    let provider_updated_at = coins
        .iter()
        .filter_map(|coin| coin.last_updated.as_deref())
        .filter_map(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .min();
    let inputs = coins
        .into_iter()
        .map(|coin| CoinMarketInput {
            id: coin.id,
            rank: coin.market_cap_rank,
            name: coin.name,
            symbol: coin.symbol,
            price: coin.current_price,
            change_1h: coin.price_change_percentage_1h_in_currency,
            change_24h: coin.price_change_percentage_24h,
            change_7d: coin.price_change_percentage_7d_in_currency,
            market_cap: coin.market_cap,
            volume_24h: coin.total_volume,
            circulating_supply: coin.circulating_supply,
            sparkline_7d: coin
                .sparkline_in_7d
                .and_then(|value| value.price)
                .unwrap_or_default()
                .into_iter()
                .flatten()
                .collect(),
        })
        .collect();
    Ok(MarketSnapshot::new(
        MarketSummaryInput {
            total_market_cap: None,
            total_volume_24h: None,
            btc_dominance: None,
            market_cap_change_24h: None,
        },
        inputs,
        provider_updated_at,
    ))
}
