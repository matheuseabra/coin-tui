//! Provider-independent market types and the boundary normalization step.
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq)]
pub struct MarketSnapshot {
    summary: MarketSummary,
    coins: Vec<CoinMarket>,
    provider_updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketSummary {
    total_market_cap: Option<f64>,
    total_volume_24h: Option<f64>,
    btc_dominance: Option<f64>,
    market_cap_change_24h: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoinMarket {
    id: String,
    rank: Option<u32>,
    name: String,
    symbol: String,
    price: Option<f64>,
    change_1h: Option<f64>,
    change_24h: Option<f64>,
    change_7d: Option<f64>,
    market_cap: Option<f64>,
    volume_24h: Option<f64>,
    circulating_supply: Option<f64>,
    sparkline_7d: Vec<f64>,
}

/// Provider-independent values supplied to the domain boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketSummaryInput {
    pub total_market_cap: Option<f64>,
    pub total_volume_24h: Option<f64>,
    pub btc_dominance: Option<f64>,
    pub market_cap_change_24h: Option<f64>,
}

/// Provider-independent coin values supplied to the domain boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct CoinMarketInput {
    pub id: String,
    pub rank: Option<u32>,
    pub name: String,
    pub symbol: String,
    pub price: Option<f64>,
    pub change_1h: Option<f64>,
    pub change_24h: Option<f64>,
    pub change_7d: Option<f64>,
    pub market_cap: Option<f64>,
    pub volume_24h: Option<f64>,
    pub circulating_supply: Option<f64>,
    pub sparkline_7d: Vec<f64>,
}

impl MarketSnapshot {
    /// Build a snapshot while normalizing every untrusted numeric value.
    pub fn new(
        summary: MarketSummaryInput,
        coins: Vec<CoinMarketInput>,
        provider_updated_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            summary: MarketSummary {
                total_market_cap: finite(summary.total_market_cap),
                total_volume_24h: finite(summary.total_volume_24h),
                btc_dominance: finite(summary.btc_dominance),
                market_cap_change_24h: finite(summary.market_cap_change_24h),
            },
            coins: coins.into_iter().map(normalize_coin).collect(),
            provider_updated_at,
        }
    }

    /// Return the immutable global market metrics for this snapshot.
    pub fn summary(&self) -> &MarketSummary {
        &self.summary
    }
    pub fn coins(&self) -> &[CoinMarket] {
        &self.coins
    }
    #[cfg(test)]
    pub fn provider_updated_at(&self) -> Option<DateTime<Utc>> {
        self.provider_updated_at
    }

    /// Replace the summary while retaining rows and the oldest source time.
    pub fn with_summary(
        mut self,
        summary: MarketSummaryInput,
        summary_updated_at: Option<DateTime<Utc>>,
    ) -> Self {
        self.summary = MarketSummary {
            total_market_cap: finite(summary.total_market_cap),
            total_volume_24h: finite(summary.total_volume_24h),
            btc_dominance: finite(summary.btc_dominance),
            market_cap_change_24h: finite(summary.market_cap_change_24h),
        };
        self.provider_updated_at = match (self.provider_updated_at, summary_updated_at) {
            (Some(coins), Some(summary)) => Some(coins.min(summary)),
            (coins, summary) => coins.or(summary),
        };
        self
    }
}

impl MarketSummary {
    pub fn total_market_cap(&self) -> Option<f64> {
        self.total_market_cap
    }
    pub fn total_volume_24h(&self) -> Option<f64> {
        self.total_volume_24h
    }
    pub fn btc_dominance(&self) -> Option<f64> {
        self.btc_dominance
    }
    pub fn market_cap_change_24h(&self) -> Option<f64> {
        self.market_cap_change_24h
    }
}

impl CoinMarket {
    #[cfg(test)]
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn rank(&self) -> Option<u32> {
        self.rank
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
    pub fn price(&self) -> Option<f64> {
        self.price
    }
    #[cfg(test)]
    pub fn change_1h(&self) -> Option<f64> {
        self.change_1h
    }
    #[cfg(test)]
    pub fn change_24h(&self) -> Option<f64> {
        self.change_24h
    }
    #[cfg(test)]
    pub fn change_7d(&self) -> Option<f64> {
        self.change_7d
    }
    #[cfg(test)]
    pub fn market_cap(&self) -> Option<f64> {
        self.market_cap
    }
    #[cfg(test)]
    pub fn volume_24h(&self) -> Option<f64> {
        self.volume_24h
    }
    #[cfg(test)]
    pub fn circulating_supply(&self) -> Option<f64> {
        self.circulating_supply
    }
    #[cfg(test)]
    pub fn sparkline_7d(&self) -> &[f64] {
        &self.sparkline_7d
    }
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

fn normalize_coin(coin: CoinMarketInput) -> CoinMarket {
    CoinMarket {
        id: coin.id,
        rank: coin.rank,
        name: coin.name,
        symbol: coin.symbol,
        price: finite(coin.price),
        change_1h: finite(coin.change_1h),
        change_24h: finite(coin.change_24h),
        change_7d: finite(coin.change_7d),
        market_cap: finite(coin.market_cap),
        volume_24h: finite(coin.volume_24h),
        circulating_supply: finite(coin.circulating_supply),
        sparkline_7d: coin
            .sparkline_7d
            .into_iter()
            .filter(|value| value.is_finite())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::Value;

    fn optional_number(object: &Value, key: &str) -> Result<Option<f64>, String> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number
                .as_f64()
                .map(Some)
                .ok_or_else(|| format!("{key} is not a number")),
            Some(_) => Err(format!("{key} is not a number or null")),
        }
    }

    fn required_string(object: &Value, key: &str) -> Result<String, String> {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is required and must be a string"))
    }

    fn optional_rank(object: &Value) -> Result<Option<u32>, String> {
        match object.get("rank") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number
                .as_u64()
                .and_then(|rank| rank.try_into().ok())
                .map(Some)
                .ok_or_else(|| "rank is outside u32".into()),
            Some(_) => Err("rank is not an integer".into()),
        }
    }

    // This deliberately strict test adapter checks fixture shape instead of silently
    // turning malformed provider-shaped JSON into a plausible domain value.
    fn fixture_snapshot(json: &str) -> Result<MarketSnapshot, String> {
        let root: Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let summary = root
            .get("summary")
            .filter(|value| value.is_object())
            .ok_or("summary must be an object")?;
        let coins = root
            .get("coins")
            .and_then(Value::as_array)
            .ok_or("coins must be an array")?;
        let summary_input = MarketSummaryInput {
            total_market_cap: optional_number(summary, "total_market_cap")?,
            total_volume_24h: optional_number(summary, "total_volume_24h")?,
            btc_dominance: optional_number(summary, "btc_dominance")?,
            market_cap_change_24h: optional_number(summary, "market_cap_change_24h")?,
        };
        let coin_inputs = coins
            .iter()
            .map(|coin| {
                let object = coin.as_object().ok_or("coin must be an object")?;
                let sparkline = object
                    .get("sparkline_7d")
                    .and_then(Value::as_array)
                    .ok_or("sparkline_7d must be an array")?
                    .iter()
                    .map(|value| match value {
                        Value::Null => Ok(None),
                        Value::Number(number) => number
                            .as_f64()
                            .map(Some)
                            .ok_or("sparkline value is not a number"),
                        _ => Err("sparkline value is not a number or null"),
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                Ok(CoinMarketInput {
                    id: required_string(coin, "id")?,
                    rank: optional_rank(coin)?,
                    name: required_string(coin, "name")?,
                    symbol: required_string(coin, "symbol")?,
                    price: optional_number(coin, "price")?,
                    change_1h: optional_number(coin, "change_1h")?,
                    change_24h: optional_number(coin, "change_24h")?,
                    change_7d: optional_number(coin, "change_7d")?,
                    market_cap: optional_number(coin, "market_cap")?,
                    volume_24h: optional_number(coin, "volume_24h")?,
                    circulating_supply: optional_number(coin, "circulating_supply")?,
                    sparkline_7d: sparkline,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(MarketSnapshot::new(summary_input, coin_inputs, None))
    }

    fn complete_input() -> CoinMarketInput {
        CoinMarketInput {
            id: "bitcoin".into(),
            rank: Some(1),
            name: "Bitcoin".into(),
            symbol: "btc".into(),
            price: Some(50_000.0),
            change_1h: Some(0.1),
            change_24h: Some(1.2),
            change_7d: Some(-2.0),
            market_cap: Some(1_000_000.0),
            volume_24h: Some(25_000.0),
            circulating_supply: Some(19.0),
            sparkline_7d: vec![1.0, 2.0, 3.0],
        }
    }

    #[test]
    fn fixtures_cover_complete_missing_empty_and_json_null() {
        let complete = fixture_snapshot(include_str!("../tests/fixtures/complete.json")).unwrap();
        assert_eq!(complete.summary().total_market_cap(), Some(1_000_000.0));
        assert_eq!(complete.summary().total_volume_24h(), Some(25_000.0));
        assert_eq!(complete.summary().btc_dominance(), Some(52.5));
        assert_eq!(complete.summary().market_cap_change_24h(), Some(1.2));
        let coin = &complete.coins()[0];
        assert_eq!(
            (coin.id(), coin.rank(), coin.name(), coin.symbol()),
            ("bitcoin", Some(1), "Bitcoin", "btc")
        );
        assert_eq!(
            (
                coin.price(),
                coin.change_1h(),
                coin.change_24h(),
                coin.change_7d(),
                coin.market_cap(),
                coin.volume_24h(),
                coin.circulating_supply()
            ),
            (
                Some(50_000.0),
                Some(0.1),
                Some(1.2),
                Some(-2.0),
                Some(1_000_000.0),
                Some(25_000.0),
                Some(19.0)
            )
        );
        assert_eq!(coin.sparkline_7d(), &[1.0, 2.0, 3.0]);

        let missing =
            fixture_snapshot(include_str!("../tests/fixtures/missing-optional.json")).unwrap();
        assert_eq!(
            (
                missing.summary().total_market_cap(),
                missing.summary().total_volume_24h(),
                missing.summary().btc_dominance(),
                missing.summary().market_cap_change_24h()
            ),
            (None, None, None, None)
        );
        let missing_coin = &missing.coins()[0];
        assert_eq!(
            (
                missing_coin.id(),
                missing_coin.name(),
                missing_coin.symbol()
            ),
            ("unknown", "Unknown", "?")
        );
        assert_eq!(
            (
                missing_coin.rank(),
                missing_coin.price(),
                missing_coin.change_1h(),
                missing_coin.change_24h(),
                missing_coin.change_7d(),
                missing_coin.market_cap(),
                missing_coin.volume_24h(),
                missing_coin.circulating_supply()
            ),
            (None, None, None, None, None, None, None, None)
        );
        assert!(missing_coin.sparkline_7d().is_empty());
        assert!(
            fixture_snapshot(include_str!("../tests/fixtures/empty.json"))
                .unwrap()
                .coins()
                .is_empty()
        );
        let null = fixture_snapshot(include_str!("../tests/fixtures/json-null.json")).unwrap();
        assert!(
            null.coins()[0].change_24h().is_none() && null.coins()[0].sparkline_7d().len() == 2
        );
    }

    #[test]
    fn every_scalar_and_sparkline_non_finite_value_becomes_missing() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let coin = CoinMarketInput {
                price: Some(invalid),
                change_1h: Some(invalid),
                change_24h: Some(invalid),
                change_7d: Some(invalid),
                market_cap: Some(invalid),
                volume_24h: Some(invalid),
                circulating_supply: Some(invalid),
                sparkline_7d: vec![invalid, 1.0],
                ..complete_input()
            };
            let summary = MarketSummaryInput {
                total_market_cap: Some(invalid),
                total_volume_24h: Some(invalid),
                btc_dominance: Some(invalid),
                market_cap_change_24h: Some(invalid),
            };
            let result = MarketSnapshot::new(summary, vec![coin], None);
            assert_eq!(
                (
                    result.summary().total_market_cap(),
                    result.summary().total_volume_24h(),
                    result.summary().btc_dominance(),
                    result.summary().market_cap_change_24h()
                ),
                (None, None, None, None)
            );
            let coin = &result.coins()[0];
            assert_eq!(
                (
                    coin.price(),
                    coin.change_1h(),
                    coin.change_24h(),
                    coin.change_7d(),
                    coin.market_cap(),
                    coin.volume_24h(),
                    coin.circulating_supply()
                ),
                (None, None, None, None, None, None, None)
            );
            assert_eq!(coin.sparkline_7d(), &[1.0]);
        }
    }

    #[test]
    fn timestamp_is_preserved_and_fixture_adapter_rejects_bad_shape_and_rank_overflow() {
        let timestamp = Utc.timestamp_opt(1_700_000_000, 123).single();
        let snapshot = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![],
            timestamp,
        );
        assert_eq!(snapshot.provider_updated_at(), timestamp);
        assert!(fixture_snapshot(r#"{"summary":{},"coins":[{"id":1}]}"#).is_err());
        assert!(fixture_snapshot(r#"{"summary":{},"coins":[{"id":"x","rank":4294967296,"name":"x","symbol":"x","sparkline_7d":[] }]}"#).is_err());
    }
}
