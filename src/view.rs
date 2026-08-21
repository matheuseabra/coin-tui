//! Pure projections used by the terminal renderer.

use crate::{
    api::ApiError,
    app::{DetailState, NewsFeed},
    domain::{CoinMarket, MarketSnapshot, PricePoint},
    news::NewsItem,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SentimentView {
    pub up: usize,
    pub down: usize,
    pub flat: usize,
    pub bullish: usize,
    pub average: f64,
    pub best: Option<(String, f64)>,
    pub worst: Option<(String, f64)>,
}

pub(crate) fn sentiment(snapshot: &MarketSnapshot) -> Option<SentimentView> {
    let changes: Vec<(&CoinMarket, f64)> = snapshot
        .coins()
        .iter()
        .filter_map(|coin| {
            coin.change_24h()
                .filter(|value| value.is_finite())
                .map(|value| (coin, value))
        })
        .collect();
    if changes.is_empty() {
        return None;
    }
    let up = changes.iter().filter(|(_, value)| *value > 0.0).count();
    let down = changes.iter().filter(|(_, value)| *value < 0.0).count();
    let flat = changes.len() - up - down;
    let best = changes
        .iter()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(coin, value)| (coin.symbol().to_owned(), *value));
    let worst = changes
        .iter()
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(coin, value)| (coin.symbol().to_owned(), *value));
    Some(SentimentView {
        up,
        down,
        flat,
        bullish: ((up as f64 * 100.0) / changes.len() as f64).round() as usize,
        average: changes.iter().map(|(_, value)| *value).sum::<f64>() / changes.len() as f64,
        best,
        worst,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DetailView {
    pub series: Vec<PricePoint>,
    pub low: f64,
    pub high: f64,
    pub is_30_day: bool,
}

pub(crate) fn detail(state: &DetailState) -> Option<DetailView> {
    let (series, is_30_day) = if !state.chart_30d().is_empty() {
        (state.chart_30d().to_vec(), true)
    } else {
        let series = match state {
            DetailState::Ready { detail, .. } if !detail.sparkline_7d().is_empty() => {
                detail.sparkline_7d()
            }
            _ => state.base().sparkline_7d(),
        };
        (
            series
                .iter()
                .enumerate()
                .map(|(index, &price)| PricePoint {
                    timestamp: index as f64 * 3_600_000.0,
                    price,
                })
                .collect(),
            false,
        )
    };
    let values: Vec<f64> = series
        .iter()
        .map(|point| point.price)
        .filter(|value| value.is_finite())
        .collect();
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (!values.is_empty()).then_some(DetailView {
        series,
        low,
        high,
        is_30_day,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NewsView {
    pub loading: bool,
    pub enabled: bool,
    pub items: Vec<NewsItem>,
    pub notice: Option<ApiError>,
}

pub(crate) fn news(feed: Option<&NewsFeed>, enabled: bool) -> NewsView {
    match feed {
        Some(feed) => NewsView {
            loading: false,
            enabled,
            items: feed.items.clone(),
            notice: feed.notice.clone(),
        },
        None => NewsView {
            loading: enabled,
            enabled,
            items: Vec::new(),
            notice: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoinMarketInput, MarketSummaryInput};

    fn snapshot(changes: &[Option<f64>]) -> MarketSnapshot {
        MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            changes
                .iter()
                .enumerate()
                .map(|(index, change_24h)| CoinMarketInput {
                    id: index.to_string(),
                    rank: Some(index as u32),
                    name: format!("Coin {index}"),
                    symbol: format!("C{index}"),
                    price: None,
                    change_1h: None,
                    change_24h: *change_24h,
                    change_7d: None,
                    market_cap: None,
                    volume_24h: None,
                    circulating_supply: None,
                    sparkline_7d: Vec::new(),
                })
                .collect(),
            None,
        )
    }

    #[test]
    fn sentiment_projection_ignores_missing_and_non_finite_changes() {
        let view = sentiment(&snapshot(&[
            Some(2.0),
            Some(-1.0),
            Some(0.0),
            None,
            Some(f64::NAN),
        ]))
        .unwrap();
        assert_eq!(view.up, 1);
        assert_eq!(view.down, 1);
        assert_eq!(view.flat, 1);
        assert_eq!(view.bullish, 33);
        assert_eq!(view.best, Some(("C0".into(), 2.0)));
        assert_eq!(view.worst, Some(("C1".into(), -1.0)));
    }

    #[test]
    fn news_projection_distinguishes_loading_from_disabled() {
        let loading = news(None, true);
        assert!(loading.loading);
        assert!(loading.enabled);
        let disabled = news(None, false);
        assert!(!disabled.loading);
        assert!(!disabled.enabled);
    }
}
