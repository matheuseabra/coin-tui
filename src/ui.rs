use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_SANITIZED_SCALARS: usize = 256;

use crate::{
    api::ApiError,
    app::{App, DataState},
    domain::{CoinMarket, MarketSnapshot},
    format::{format_age, format_compact_money, format_percentage, format_price},
};

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let (title, body) = match app.state_ref() {
        DataState::Initial => (
            "Market | Initial",
            vec![Line::from("Starting market data...")],
        ),
        DataState::Loading => (
            "Market | Loading",
            vec![Line::from("Loading market data...")],
        ),
        DataState::Ready {
            snapshot, notice, ..
        } => ("Market | Live", rows_body(snapshot, notice.as_ref())),
        DataState::Empty { notice, .. } => {
            let mut body = vec![Line::from("No market rows were returned.")];
            if app.fetching() {
                body.push(Line::from("Refreshing market data..."));
            } else {
                body.push(Line::from(if notice.is_some() {
                    "Summary unavailable; press r to refresh."
                } else {
                    "Press r to refresh."
                }));
            }
            ("Market | Empty", body)
        }
        DataState::Stale {
            snapshot,
            error,
            notice,
            ..
        } => (
            "Market | Stale",
            stale_rows_body(snapshot, error, notice.as_ref()),
        ),
        DataState::Fatal(error) => {
            let mut body = vec![Line::from(fatal_message(error))];
            if app.fetching() {
                body.push(Line::from("Refreshing market data..."));
            }
            ("Market | Error", body)
        }
    };

    frame.render_widget(
        Paragraph::new(summary_line(app, frame.area().width))
            .style(Style::default().fg(Color::Cyan))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Market summary "),
            ),
        areas[0],
    );
    frame.render_widget(
        Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(title)),
        areas[1],
    );
    frame.render_widget(Paragraph::new(status_line(app)), areas[2]);
}

fn rows_body(snapshot: &MarketSnapshot, notice: Option<&ApiError>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("#  Name (symbol)       Price")];
    if let Some(error) = notice {
        lines.push(Line::from(summary_message(error)));
    }
    lines.extend(
        snapshot
            .coins()
            .iter()
            .map(|coin| Line::from(format_row(coin))),
    );
    lines
}

fn stale_rows_body(
    snapshot: &MarketSnapshot,
    error: &ApiError,
    notice: Option<&ApiError>,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("#  Name (symbol)       Price")];
    if let Some(error) = notice {
        lines.push(Line::from(summary_message(error)));
    }
    let reason = match error {
        ApiError::Timeout | ApiError::Transport => "Offline",
        _ => short_error(error),
    };
    lines.push(Line::from(format!(
        "Refresh failed: {reason}. Press r to retry."
    )));
    lines.extend(
        snapshot
            .coins()
            .iter()
            .map(|coin| Line::from(format_row(coin))),
    );
    lines
}

fn summary_line(app: &App, width: u16) -> String {
    let values = match app.state_ref() {
        DataState::Ready { snapshot, .. }
        | DataState::Empty { snapshot, .. }
        | DataState::Stale { snapshot, .. } => snapshot.summary(),
        DataState::Initial | DataState::Loading | DataState::Fatal(_) => {
            return "Cap:- Vol24:- BTCdom:- Mkt24:-".into()
        }
    };
    let cap = format_compact_money(values.total_market_cap());
    let volume = format_compact_money(values.total_volume_24h());
    let dominance = summary_percentage(values.btc_dominance(), false);
    let change = summary_percentage(values.market_cap_change_24h(), true);
    if width < 80 {
        format!("Cap:{cap} Vol24:{volume} BTCdom:{dominance} Mkt24:{change}")
    } else {
        format!("Cap: {cap} | Vol 24h: {volume} | BTC dom: {dominance} | Mkt 24h: {change}")
    }
}

fn summary_percentage(value: Option<f64>, signed: bool) -> String {
    let formatted = format_percentage(value);
    if formatted == "-" {
        return formatted;
    }
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "-".into();
    };
    if value.abs() >= 999_999.995 {
        return match (signed, value.is_sign_negative()) {
            (_, true) => "<-999K%".into(),
            (true, false) => ">+999K%".into(),
            (false, false) => ">999K%".into(),
        };
    }
    if signed {
        formatted
    } else {
        formatted.strip_prefix('+').unwrap_or(&formatted).to_owned()
    }
}

fn status_line(app: &App) -> String {
    let (state, age, notice) = match app.state_ref() {
        DataState::Initial | DataState::Loading => ("LOADING".into(), None, None),
        DataState::Ready {
            refreshed_at,
            notice,
            ..
        } => (
            if app.fetching() { "REFRESHING" } else { "LIVE" }.into(),
            Some(refreshed_at),
            notice.as_ref(),
        ),
        DataState::Empty {
            refreshed_at,
            notice,
            ..
        } => (
            if app.fetching() {
                "REFRESHING"
            } else {
                "EMPTY"
            }
            .into(),
            Some(refreshed_at),
            notice.as_ref(),
        ),
        DataState::Stale {
            refreshed_at,
            error,
            notice,
            ..
        } => (
            if app.fetching() {
                format!("REFRESHING | cached {}", stale_status(error))
            } else {
                stale_status(error).into()
            },
            Some(refreshed_at),
            notice.as_ref(),
        ),
        DataState::Fatal(error) => (
            if app.fetching() {
                format!("REFRESHING | previous {}", fatal_status(error))
            } else {
                fatal_status(error).into()
            },
            None,
            None,
        ),
    };
    let mut line = state;
    if let Some(time) = age {
        line.push_str(&format!(" | age {}", format_age(time.elapsed())));
    }
    if notice.is_some() {
        line.push_str(" | SUMMARY DEGRADED");
    }
    line.push_str(" | q quit | r refresh");
    line
}

fn stale_status(error: &ApiError) -> &'static str {
    match error {
        ApiError::Timeout | ApiError::Transport => "OFFLINE",
        ApiError::RateLimited { .. } => "RATE LIMITED",
        _ => "STALE",
    }
}

fn fatal_status(error: &ApiError) -> &'static str {
    match error {
        ApiError::Timeout | ApiError::Transport => "OFFLINE",
        ApiError::RateLimited { .. } => "RATE LIMITED",
        _ => "ERROR",
    }
}

fn format_row(coin: &CoinMarket) -> String {
    let rank = coin
        .rank()
        .map_or_else(|| "-".into(), |rank| rank.to_string());
    let name = clean_remote(coin.name(), 18);
    let symbol = clean_remote(coin.symbol(), 8);
    let price = format_price(coin.price());
    format!(
        "{rank:>2}  {} ({}) {:>10}",
        pad_cells(&name, 18),
        pad_cells(&symbol, 8),
        price
    )
}

fn clean_remote(value: &str, max_cells: usize) -> String {
    truncate_cells(
        &value
            .chars()
            .filter(|character| !character.is_control() && !is_terminal_format(*character))
            .take(MAX_SANITIZED_SCALARS)
            .collect::<String>(),
        max_cells,
    )
}

fn is_terminal_format(character: char) -> bool {
    matches!(
        character as u32,
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x0890..=0x0891
            | 0x08e2 | 0x180e | 0x200b..=0x200f | 0x202a..=0x202e
            | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb
            | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f
    )
}

fn truncate_cells(value: &str, max_cells: usize) -> String {
    let mut used = 0;
    value
        .chars()
        .take_while(|character| {
            let width = UnicodeWidthChar::width(*character).unwrap_or(0);
            let fits = used + width <= max_cells;
            if fits {
                used += width;
            }
            fits
        })
        .collect()
}

fn pad_cells(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(value));
    format!("{value}{}", " ".repeat(padding))
}

fn summary_message(error: &ApiError) -> String {
    match error {
        ApiError::RateLimited {
            retry_after: Some(delay),
        } => {
            format!(
                "Summary unavailable: rate limited; retry after {}s.",
                delay.as_secs()
            )
        }
        ApiError::RateLimited { retry_after: None } => {
            "Summary unavailable: rate limited; retry later.".into()
        }
        _ => format!("Summary unavailable: {}.", short_error(error)),
    }
}

fn fatal_message(error: &ApiError) -> String {
    match error {
        ApiError::RateLimited {
            retry_after: Some(delay),
        } => {
            format!(
                "Rate limited: retry after {}s; no market data is available.",
                delay.as_secs()
            )
        }
        ApiError::RateLimited { retry_after: None } => {
            "Rate limited: retry later; no market data is available.".into()
        }
        ApiError::Timeout | ApiError::Transport => {
            "Offline: no market data is available; press r to retry.".into()
        }
        _ => format!("Error: {}; press r to retry.", short_error(error)),
    }
}

fn short_error(error: &ApiError) -> &'static str {
    match error {
        ApiError::Timeout | ApiError::Transport => "offline",
        ApiError::MalformedResponse => "invalid provider response",
        ApiError::InvalidBaseUrl | ApiError::InvalidTimeoutConfiguration => "invalid configuration",
        ApiError::HttpStatus { .. } => "provider request failed",
        ApiError::RateLimited { .. } => "rate limited",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{Command, Event},
        domain::{CoinMarketInput, MarketSummaryInput},
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
    use std::time::Duration;

    fn app_with(result: Result<crate::api::FetchOutcome, ApiError>) -> App {
        let mut app = App::new();
        let Command::Fetch { generation } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult { generation, result });
        app
    }
    fn snapshot(name: &str) -> MarketSnapshot {
        MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![CoinMarketInput {
                id: "x".into(),
                rank: Some(1),
                name: name.into(),
                symbol: "BTC\n\t".into(),
                price: Some(123.0),
                change_1h: None,
                change_24h: None,
                change_7d: None,
                market_cap: None,
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            }],
            None,
        )
    }

    fn summary_snapshot() -> MarketSnapshot {
        MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: Some(1_234_000_000_000.0),
                total_volume_24h: Some(987_000_000.0),
                btc_dominance: Some(52.5),
                market_cap_change_24h: Some(-1.25),
            },
            vec![CoinMarketInput {
                id: "x".into(),
                rank: Some(1),
                name: "Bitcoin".into(),
                symbol: "BTC".into(),
                price: Some(1.0),
                change_1h: None,
                change_24h: None,
                change_7d: None,
                market_cap: None,
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            }],
            None,
        )
    }

    fn many_rows() -> MarketSnapshot {
        let mut rows = Vec::new();
        for rank in 1..=100 {
            rows.push(CoinMarketInput {
                id: rank.to_string(),
                rank: Some(rank),
                name: format!("Coin {rank}"),
                symbol: "x".into(),
                price: Some(1.0),
                change_1h: None,
                change_24h: None,
                change_7d: None,
                market_cap: None,
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            });
        }
        MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            rows,
            None,
        )
    }
    fn text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer: &Buffer = terminal.backend().buffer();
        buffer.content().iter().map(|cell| cell.symbol()).collect()
    }

    fn text_at(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn summary_is_labeled_complete_or_missing_and_has_no_color_dependency() {
        let complete = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        let rendered = text_at(&complete, 80, 24);
        assert!(rendered.contains("Cap: $1.23T"));
        assert!(rendered.contains("Vol 24h: $987M"));
        assert!(rendered.contains("BTC dom: 52.50%"));
        assert!(rendered.contains("Mkt 24h: -1.25%"));
        assert_eq!(summary_line(&complete, 80).matches("|").count(), 3);

        let missing = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        let compact = summary_line(&missing, 60);
        assert_eq!(compact, "Cap:- Vol24:- BTCdom:- Mkt24:-");
        assert!(compact.contains("Cap") && compact.contains("Vol24") && compact.contains("BTCdom"));
    }

    #[test]
    fn compact_summary_is_one_fitting_line_and_standard_summary_is_separated() {
        let app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        let compact = summary_line(&app, 60);
        assert!(UnicodeWidthStr::width(compact.as_str()) <= 58);
        let rendered = text_at(&app, 60, 16);
        assert_eq!(rendered.matches("Cap:$").count(), 1);
        let standard = text_at(&app, 80, 24);
        assert!(standard.contains("Market summary") && standard.contains("Cap: $1.23T"));

        let extreme = app_with(Ok(crate::api::FetchOutcome {
            snapshot: MarketSnapshot::new(
                MarketSummaryInput {
                    total_market_cap: Some(f64::MAX),
                    total_volume_24h: Some(f64::MAX),
                    btc_dominance: Some(f64::MAX),
                    market_cap_change_24h: Some(-f64::MAX),
                },
                vec![],
                None,
            ),
            summary_notice: None,
        }));
        let extreme_line = summary_line(&extreme, 60);
        assert!(UnicodeWidthStr::width(extreme_line.as_str()) <= 58);
        for value in [
            "Cap:$999T+",
            "Vol24:$999T+",
            "BTCdom:>999K%",
            "Mkt24:<-999K%",
        ] {
            assert!(
                extreme_line.contains(value),
                "missing {value} in {extreme_line}"
            );
            assert!(text_at(&extreme, 60, 16).contains(value));
        }
    }

    #[test]
    fn status_line_names_states_age_notice_and_controls() {
        let ready = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: Some(ApiError::RateLimited { retry_after: None }),
        }));
        let live = text_at(&ready, 80, 24);
        assert!(live.contains("LIVE") && live.contains("age") && live.contains("SUMMARY DEGRADED"));
        assert!(live.contains("q quit") && live.contains("r refresh"));

        let mut loading = App::new();
        loading.update(Event::Start);
        assert!(text_at(&loading, 80, 24).contains("LOADING"));

        let mut stale = ready;
        let Command::Fetch { generation } = stale.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        stale.update(Event::FetchResult {
            generation,
            result: Err(ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(4)),
            }),
        });
        assert!(text_at(&stale, 80, 24).contains("RATE LIMITED"));
        let _ = stale.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        let retrying = text_at(&stale, 80, 24);
        assert!(retrying.contains("REFRESHING") && retrying.contains("cached RATE LIMITED"));
        assert!(retrying.contains("Summary unavailable"));

        let offline = app_with(Err(ApiError::Transport));
        assert!(text_at(&offline, 80, 24).contains("OFFLINE"));
        let fatal = app_with(Err(ApiError::MalformedResponse));
        assert!(text_at(&fatal, 80, 24).contains("ERROR"));
        let empty = app_with(Ok(crate::api::FetchOutcome {
            snapshot: MarketSnapshot::new(
                MarketSummaryInput {
                    total_market_cap: None,
                    total_volume_24h: None,
                    btc_dominance: None,
                    market_cap_change_24h: None,
                },
                vec![],
                None,
            ),
            summary_notice: None,
        }));
        assert!(text_at(&empty, 80, 24).contains("EMPTY"));
    }
    #[test]
    fn renders_states_rows_refreshing_notices_and_sanitized_names() {
        assert!(text(&App::new()).contains("Starting market data"));
        let mut loading = App::new();
        loading.update(Event::Start);
        assert!(text(&loading).contains("Loading market data"));
        let ready = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bad\nName"),
            summary_notice: None,
        }));
        assert!(text(&ready).contains("BadName") && text(&ready).contains("$123.00"));
        let mut refreshing = ready;
        let _ = refreshing.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(text(&refreshing).contains("REFRESHING"));
        let notice = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: Some(ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(9)),
            }),
        }));
        let rendered = text(&notice);
        assert!(
            rendered.contains("Bitcoin")
                && rendered.contains("retry after 9s")
                && rendered.contains("r refresh")
        );
    }

    #[test]
    fn persistent_status_stays_visible_before_one_hundred_rows() {
        let ready = app_with(Ok(crate::api::FetchOutcome {
            snapshot: many_rows(),
            summary_notice: Some(ApiError::HttpStatus { status: 500 }),
        }));
        let ready_text = text(&ready);
        assert!(ready_text.contains("LIVE") && ready_text.contains("SUMMARY DEGRADED"));

        let mut refreshing = ready;
        let _ = refreshing.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(text(&refreshing).contains("REFRESHING"));

        let mut stale = app_with(Ok(crate::api::FetchOutcome {
            snapshot: many_rows(),
            summary_notice: None,
        }));
        let Command::Fetch { generation } = stale.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        stale.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        let stale_text = text(&stale);
        assert!(stale_text.contains("age") && stale_text.contains("OFFLINE"));
    }

    #[test]
    fn rows_are_cell_bounded_and_strip_terminal_formats_and_extreme_prices() {
        let input = format!("wide界{}\u{202e}hidden\u{001b}[31m", "界".repeat(40));
        let market = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![CoinMarketInput {
                id: "x".into(),
                rank: Some(1),
                name: input.clone(),
                symbol: input,
                price: Some(f64::MAX),
                change_1h: None,
                change_24h: None,
                change_7d: None,
                market_cap: None,
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            }],
            None,
        );
        let line = format_row(&market.coins()[0]);
        assert!(UnicodeWidthStr::width(line.as_str()) <= 44);
        assert!(!line.contains('\u{202e}') && !line.contains('\u{001b}'));
        assert!(line.contains("$999T+"));
        assert!(text(&app_with(Ok(crate::api::FetchOutcome {
            snapshot: market,
            summary_notice: None,
        })))
        .contains("LIVE"));
    }

    #[test]
    fn remote_text_has_scalar_bound_even_with_zero_width_marks() {
        let input = format!("Visible{}", "\u{301}".repeat(10_000));
        let cleaned = clean_remote(&input, 18);

        assert!(cleaned.contains("Visible"));
        assert!(cleaned.chars().count() <= MAX_SANITIZED_SCALARS);
        assert!(cleaned.len() <= MAX_SANITIZED_SCALARS * 4);

        let rendered = text(&app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot(&input),
            summary_notice: None,
        })));
        assert!(rendered.contains("Visible"));
    }
    #[test]
    fn renders_stale_empty_offline_and_rate_limit() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        let Command::Fetch { generation } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        let stale = text(&app);
        assert!(stale.contains("Bitcoin") && stale.contains("age") && stale.contains("OFFLINE"));
        let mut empty = app_with(Ok(crate::api::FetchOutcome {
            snapshot: MarketSnapshot::new(
                MarketSummaryInput {
                    total_market_cap: None,
                    total_volume_24h: None,
                    btc_dominance: None,
                    market_cap_change_24h: None,
                },
                vec![],
                None,
            ),
            summary_notice: None,
        }));
        assert!(text(&empty).contains("No market rows") && text(&empty).contains("EMPTY"));
        let _ = empty.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(text(&empty).contains("REFRESHING"));

        let mut fatal = app_with(Err(ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(4)),
        }));
        assert!(text(&fatal).contains("Rate limited") && text(&fatal).contains("4s"));
        let _ = fatal.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(text(&fatal).contains("Refreshing market data"));
    }
}
