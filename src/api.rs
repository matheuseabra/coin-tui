//! CoinGecko market provider and conversion at the provider boundary.

use std::{fmt, future::Future, pin::Pin, time::Duration};

use chrono::{DateTime, TimeZone, Utc};
use reqwest::Url;
use serde::Deserialize;

use crate::domain::{
    CoinDetail, CoinDetailInput, CoinMarketInput, MarketSnapshot, MarketSummaryInput, PricePoint,
};
use crate::http::{validate_url, HttpClient};

const MARKETS_PATH: &str = "api/v3/coins/markets";
const GLOBAL_PATH: &str = "api/v3/global";
const COINS_PATH: &str = "api/v3/coins";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Days of history for the detail candlestick chart's price series.
const MARKET_CHART_DAYS: &str = "30";

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
    client: HttpClient,
    base_url: Url,
    api_key: Option<String>,
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
        let base_url = validate_url(base_url)?;
        let client = HttpClient::new(connect_timeout, total_timeout)?;
        Ok(Self {
            client,
            base_url,
            api_key,
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

    /// Rich CoinMarketCap-shaped detail for one coin (`GET /coins/{id}`):
    /// high/low, ATH/ATL, 14d/30d/60d/1y changes, supply totals, fully
    /// diluted valuation, categories, community sentiment votes, a description
    /// snippet, and a dense hourly 7-day sparkline.
    pub async fn fetch_coin_detail(&self, id: &str) -> Result<CoinDetail, ApiError> {
        let coin: CoinGeckoCoin = self
            .request_url(
                self.coin_detail_url(id)?,
                &[
                    ("localization", "false"),
                    ("tickers", "false"),
                    ("market_data", "true"),
                    ("community_data", "false"),
                    ("developer_data", "false"),
                    ("sparkline", "true"),
                    ("vs_currency", "usd"),
                ],
            )
            .await?;
        convert_detail(coin)
    }

    /// Price history for the detail chart (`GET /coins/{id}/market_chart`),
    /// `days=30`, so the candlestick chart has enough candles to stretch the
    /// pane. Returns just the `prices` series as (timestamp, price) pairs.
    pub async fn fetch_market_chart(&self, id: &str) -> Result<Vec<PricePoint>, ApiError> {
        let mut url = self.coin_detail_url(id)?;
        url.path_segments_mut()
            .map_err(|_| ApiError::InvalidBaseUrl)?
            .push("market_chart");
        let chart: CoinGeckoMarketChart = self
            .request_url(url, &[("vs_currency", "usd"), ("days", MARKET_CHART_DAYS)])
            .await?;
        Ok(chart
            .prices
            .into_iter()
            .filter_map(|pair| {
                let mut values = pair.into_iter();
                let timestamp = values.next().flatten()?;
                let price = values.next().flatten()?;
                if !timestamp.is_finite() || !price.is_finite() {
                    return None;
                }
                Some(PricePoint { timestamp, price })
            })
            .collect())
    }

    /// Percent-encode the coin id into a `/coins/{id}` URL so a hostile id
    /// cannot smuggle extra path segments or query text into the request.
    fn coin_detail_url(&self, id: &str) -> Result<Url, ApiError> {
        let mut url = self
            .base_url
            .join(&format!("{COINS_PATH}/"))
            .map_err(|_| ApiError::InvalidBaseUrl)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| ApiError::InvalidBaseUrl)?;
            segments.pop_if_empty().push(id);
        }
        Ok(url)
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
        self.request_url(url, query).await
    }

    async fn request_url<T: for<'de> Deserialize<'de>>(
        &self,
        url: Url,
        query: &[(&str, &str)],
    ) -> Result<T, ApiError> {
        let body = self
            .client
            .get(
                url,
                query,
                self.api_key.as_deref(),
                MAX_RESPONSE_BYTES,
                true,
            )
            .await?;
        serde_json::from_slice(&body).map_err(|_| ApiError::MalformedResponse)
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

    /// Rich detail for one coin. The default is unsupported: providers that
    /// cannot serve it leave the app on the fallback row-based detail.
    fn fetch_coin_detail<'a>(
        &'a self,
        _id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<CoinDetail, ApiError>> + Send + 'a>> {
        Box::pin(async move { Err(ApiError::HttpStatus { status: 501 }) })
    }

    /// Price history for the detail chart. The default is unsupported, like
    /// `fetch_coin_detail`; the detail chart then falls back to the snapshot's
    /// 7-day series.
    fn fetch_market_chart<'a>(
        &'a self,
        _id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PricePoint>, ApiError>> + Send + 'a>> {
        Box::pin(async move { Err(ApiError::HttpStatus { status: 501 }) })
    }
}

impl MarketData for CoinGeckoClient {
    fn fetch_snapshot<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, ApiError>> + Send + 'a>> {
        Box::pin(CoinGeckoClient::fetch_snapshot(self))
    }

    fn fetch_coin_detail<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<CoinDetail, ApiError>> + Send + 'a>> {
        Box::pin(CoinGeckoClient::fetch_coin_detail(self, id))
    }

    fn fetch_market_chart<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<PricePoint>, ApiError>> + Send + 'a>> {
        Box::pin(CoinGeckoClient::fetch_market_chart(self, id))
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

/// `GET /coins/{id}` response: the top-level identity plus the rich
/// `market_data` block that backs the CoinMarketCap-style sidebar.
#[derive(Deserialize)]
struct CoinGeckoCoin {
    id: String,
    symbol: String,
    name: String,
    market_cap_rank: Option<u32>,
    categories: Option<Vec<String>>,
    description: Option<CoinGeckoDescription>,
    market_data: Option<CoinGeckoMarketData>,
}

#[derive(Default, Deserialize)]
struct CoinGeckoDescription {
    #[serde(default)]
    en: Option<String>,
}

#[derive(Default, Deserialize)]
struct CoinGeckoMarketData {
    #[serde(default)]
    current_price: Option<CurrencyValue>,
    #[serde(default)]
    market_cap: Option<CurrencyValue>,
    #[serde(default)]
    fully_diluted_valuation: Option<CurrencyValue>,
    #[serde(default)]
    total_volume: Option<CurrencyValue>,
    #[serde(default)]
    high_24h: Option<CurrencyValue>,
    #[serde(default)]
    low_24h: Option<CurrencyValue>,
    #[serde(default)]
    ath: Option<CurrencyValue>,
    #[serde(default)]
    atl: Option<CurrencyValue>,
    #[serde(default)]
    ath_change_percentage: Option<CurrencyValue>,
    #[serde(default)]
    atl_change_percentage: Option<CurrencyValue>,
    #[serde(default)]
    price_change_percentage_1h_in_currency: Option<CurrencyValue>,
    #[serde(default)]
    price_change_percentage_24h: Option<f64>,
    #[serde(default)]
    price_change_percentage_7d_in_currency: Option<CurrencyValue>,
    #[serde(default)]
    price_change_percentage_14d_in_currency: Option<CurrencyValue>,
    #[serde(default)]
    price_change_percentage_30d_in_currency: Option<CurrencyValue>,
    #[serde(default)]
    price_change_percentage_60d_in_currency: Option<CurrencyValue>,
    #[serde(default)]
    price_change_percentage_1y_in_currency: Option<CurrencyValue>,
    #[serde(default)]
    circulating_supply: Option<f64>,
    #[serde(default)]
    total_supply: Option<f64>,
    #[serde(default)]
    max_supply: Option<f64>,
    #[serde(default)]
    sentiment_votes_up_percentage: Option<f64>,
    #[serde(default)]
    sentiment_votes_down_percentage: Option<f64>,
    #[serde(default)]
    sparkline_7d: Option<Sparkline>,
}

fn convert_detail(coin: CoinGeckoCoin) -> Result<CoinDetail, ApiError> {
    let market = coin.market_data.unwrap_or_default();
    Ok(CoinDetail::new(CoinDetailInput {
        id: coin.id,
        symbol: coin.symbol,
        name: coin.name,
        rank: coin.market_cap_rank,
        price: market.current_price.and_then(|value| value.usd),
        change_1h: market
            .price_change_percentage_1h_in_currency
            .and_then(|value| value.usd),
        change_24h: market.price_change_percentage_24h,
        change_7d: market
            .price_change_percentage_7d_in_currency
            .and_then(|value| value.usd),
        change_14d: market
            .price_change_percentage_14d_in_currency
            .and_then(|value| value.usd),
        change_30d: market
            .price_change_percentage_30d_in_currency
            .and_then(|value| value.usd),
        change_60d: market
            .price_change_percentage_60d_in_currency
            .and_then(|value| value.usd),
        change_1y: market
            .price_change_percentage_1y_in_currency
            .and_then(|value| value.usd),
        market_cap: market.market_cap.and_then(|value| value.usd),
        volume_24h: market.total_volume.and_then(|value| value.usd),
        high_24h: market.high_24h.and_then(|value| value.usd),
        low_24h: market.low_24h.and_then(|value| value.usd),
        ath: market.ath.and_then(|value| value.usd),
        atl: market.atl.and_then(|value| value.usd),
        ath_change: market.ath_change_percentage.and_then(|value| value.usd),
        atl_change: market.atl_change_percentage.and_then(|value| value.usd),
        circulating_supply: market.circulating_supply,
        total_supply: market.total_supply,
        max_supply: market.max_supply,
        fully_diluted_valuation: market.fully_diluted_valuation.and_then(|value| value.usd),
        categories: coin.categories.unwrap_or_default(),
        sentiment_up: market.sentiment_votes_up_percentage,
        sentiment_down: market.sentiment_votes_down_percentage,
        sparkline_7d: market
            .sparkline_7d
            .and_then(|value| value.price)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect(),
        description: coin.description.and_then(|description| description.en),
    }))
}

#[derive(Deserialize)]
struct Sparkline {
    price: Option<Vec<Option<f64>>>,
}

/// `GET /coins/{id}/market_chart` response: `prices` is a list of
/// `[timestamp, price]` pairs; `market_caps` and `total_volumes` are unused.
#[derive(Deserialize)]
struct CoinGeckoMarketChart {
    #[serde(default)]
    prices: Vec<Vec<Option<f64>>>,
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
