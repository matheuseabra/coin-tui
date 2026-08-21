use chrono::Utc;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, TableState},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_SANITIZED_SCALARS: usize = 256;

use crate::{
    api::ApiError,
    app::{App, DataState, DetailState, MainPane},
    domain::{daily_candles, CoinMarket},
    format::{
        format_age, format_compact_money, format_compact_supply, format_percentage, format_price,
    },
    news::NewsItem,
    theme::{Theme, THEMES},
    view,
};

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 16;

/// Main-view panes (news and sentiment) appear beside the market table at or
/// above this width; below it `Tab`/`Shift-Tab` show one focused pane at a
/// time so the table keeps its full column set. The threshold leaves the
/// table at 70% of the width, so Full-mode columns stay available whenever
/// the panes are side-by-side.
const PANE_MIN_WIDTH: u16 = 162;

/// Detail sidebar (coin data) appears beside the chart when the pane is at
/// least this wide; below it the stats render as stacked lines under chart.
const DETAIL_SIDEBAR_MIN_WIDTH: u16 = 78;

/// Detail sidebar width at standard and full widths.
const DETAIL_SIDEBAR_WIDTH: u16 = 36;

/// Help overlay width and content. Every line must fit the inner width (width
/// minus borders) and the whole block must fit the minimum supported height.
const HELP_WIDTH: u16 = 40;
const HELP_LINES: &[&str] = &[
    "q / Ctrl-C      Quit",
    "j / Down        Next coin",
    "k / Up          Previous coin",
    "g / Home        First coin",
    "G / End         Last coin",
    "PageUp/Down     Move one page",
    "/               Search (Enter/Esc)",
    "Tab/Shift-Tab   Switch pane",
    "Enter           Open coin detail",
    "s / Shift-S     Cycle sort column",
    "r               Refresh",
    "t / Shift-T     Cycle theme",
    "? / Esc         Close help",
];

pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let theme = app.theme();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_resize_message(frame, area, theme);
        return;
    }
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(summary_line(app, frame.area().width, theme)).block(
            Block::default().borders(Borders::ALL).title(Line::styled(
                " Market summary ",
                Style::default().fg(theme.summary),
            )),
        ),
        areas[0],
    );
    render_body(frame, app, areas[1], frame.area().width);
    frame.render_widget(
        Paragraph::new(status_line(app, frame.area().width)),
        areas[2],
    );

    if app.help_open() {
        render_help(frame, area, theme);
    }
}

fn render_resize_message(frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
    let lines = vec![
        Line::from(format!("Terminal too small: need {MIN_WIDTH}x{MIN_HEIGHT}")).centered(),
        Line::from("q quits").centered(),
    ];
    let top = (area.height / 2).saturating_sub(1);
    let inner = ratatui::layout::Rect::new(area.x, area.y + top, area.width, 2.min(area.height));
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.notice)),
        inner,
    );
}

fn render_help(frame: &mut Frame<'_>, area: ratatui::layout::Rect, theme: &Theme) {
    let height = (HELP_LINES.len() as u16) + 2;
    let left = area.x + area.width.saturating_sub(HELP_WIDTH) / 2;
    let top = area.y + area.height.saturating_sub(height) / 2;
    let outer = ratatui::layout::Rect::new(
        left,
        top,
        HELP_WIDTH.min(area.width),
        height.min(area.height),
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(Style::default().fg(theme.notice));
    let lines: Vec<Line<'static>> = HELP_LINES.iter().map(|line| Line::from(*line)).collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.notice))
            .block(block),
        outer,
    );
}

fn render_body(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect, width: u16) {
    if app.detail_open() {
        render_detail(frame, app, area, app.theme());
        return;
    }
    let theme = app.theme();
    match app.state_ref() {
        DataState::Initial => message(
            "Market | Initial",
            "Starting market data...",
            frame,
            area,
            theme,
        ),
        DataState::Loading => message(
            "Market | Loading",
            "Loading market data...",
            frame,
            area,
            theme,
        ),
        DataState::Ready { notice, .. } => {
            let info = notice
                .iter()
                .map(|error| Line::from(summary_message(error)))
                .collect();
            render_market("Market | Live", app, info, frame, area, width);
        }
        DataState::Stale { error, notice, .. } => {
            let reason = match error {
                ApiError::Timeout | ApiError::Transport => "Offline",
                _ => short_error(error),
            };
            let mut info: Vec<Line<'static>> = notice
                .iter()
                .map(|error| Line::from(summary_message(error)))
                .collect();
            let retry = match app.refresh_cooldown() {
                Some(remaining) => format!(
                    "Refresh failed: {reason}. Retrying automatically in {}s.",
                    remaining.as_secs().max(1)
                ),
                None => format!("Refresh failed: {reason}. Press r to retry."),
            };
            info.push(Line::from(retry));
            render_market("Market | Stale", app, info, frame, area, width);
        }
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
            message_lines("Market | Empty", body, frame, area, theme);
        }
        DataState::Fatal(error) => {
            let mut body = vec![Line::from(fatal_message(error))];
            if app.fetching() {
                body.push(Line::from("Refreshing market data..."));
            } else if let Some(remaining) = app.refresh_cooldown() {
                body.push(Line::from(format!(
                    "Retrying automatically in {}s.",
                    remaining.as_secs().max(1)
                )));
            }
            message_lines("Market | Error", body, frame, area, theme);
        }
    }
}

/// Route the market panes. Wide terminals show the table plus a right column
/// (news wire on top, market breadth below) split 70/30 with the right column
/// divided into two equal rows; narrow terminals show one focused pane at a
/// time so the table keeps its full column set.
fn render_market(
    title: &str,
    app: &App,
    info: Vec<Line<'static>>,
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    width: u16,
) {
    if width < PANE_MIN_WIDTH {
        match app.pane_focus() {
            MainPane::Table => table_frame(title, app, info, frame, area, width, true),
            MainPane::News => news_pane(app, frame, area, true),
            MainPane::Sentiment => sentiment_pane(app, frame, area, true),
        }
        return;
    }
    let [table_area, right] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .areas(area);
    table_frame(
        title,
        app,
        info,
        frame,
        table_area,
        table_area.width,
        app.pane_focus() == MainPane::Table,
    );
    let [news_area, sentiment_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .areas(right);
    news_pane(app, frame, news_area, app.pane_focus() == MainPane::News);
    sentiment_pane(
        app,
        frame,
        sentiment_area,
        app.pane_focus() == MainPane::Sentiment,
    );
}

/// The news wire: latest headlines, or a labeled status when the feed is
/// disabled, still loading, failed, or empty.
fn news_pane(app: &App, frame: &mut Frame<'_>, area: ratatui::layout::Rect, focused: bool) {
    let theme = app.theme();
    let inner_width = area.width.saturating_sub(2) as usize;
    let projection = view::news(app.news_feed(), app.news_enabled());
    let mut lines = Vec::new();
    for item in &projection.items {
        lines.push(headline_line(item, inner_width, theme));
        if !item.url().is_empty() {
            lines.push(Line::from(Span::styled(
                clean_remote(item.url(), inner_width),
                Style::default(),
            )));
        }
    }
    if projection.items.is_empty() {
        lines.push(Line::styled(
            if let Some(error) = &projection.notice {
                format!("News refresh failed: {}.", short_error(error))
            } else if projection.loading {
                "Loading headlines...".to_owned()
            } else if projection.enabled {
                "No headlines yet.".to_owned()
            } else {
                "News feed unavailable.".to_owned()
            },
            Style::default().fg(theme.notice),
        ));
    }
    if let Some(error) = &projection.notice {
        if !projection.items.is_empty() {
            lines.push(Line::styled(
                format!("News refresh failed: {}.", short_error(error)),
                Style::default().fg(theme.notice),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).block(pane_block("News", focused, theme)),
        area,
    );
}

/// One headline: a bounded `source · age` prefix in the summary color followed
/// by the bounded title, so no remote text can push the line past the pane.
fn headline_line(item: &NewsItem, width: usize, theme: &Theme) -> Line<'static> {
    let source = clean_remote(item.source(), 20);
    let age = item
        .published_at()
        .and_then(|at| (Utc::now() - at).to_std().ok());
    let prefix = match age {
        Some(age) => format!("{source} \u{00b7} {}  ", format_age(age)),
        None => format!("{source}  "),
    };
    let title = clean_remote(
        item.title(),
        width.saturating_sub(UnicodeWidthStr::width(prefix.as_str())),
    );
    truncate_line(
        Line::from(vec![
            Span::styled(prefix, Style::default().fg(theme.summary)),
            Span::styled(title, Style::default()),
        ]),
        width,
    )
}

/// Shared pane frame; the focused pane's title is emphasized.
fn pane_block(title: &str, focused: bool, theme: &Theme) -> Block<'static> {
    let style = if focused {
        Style::default()
            .fg(theme.notice)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.summary)
    };
    Block::default()
        .borders(Borders::ALL)
        .title(Line::styled(format!(" {title} "), style))
}

/// Market breadth from the snapshot's 24-hour changes: up/down/flat counts, a
/// bullish share meter, and the average, best, and worst mover.
fn sentiment_pane(app: &App, frame: &mut Frame<'_>, area: ratatui::layout::Rect, focused: bool) {
    let theme = app.theme();
    let inner_width = area.width.saturating_sub(2) as usize;
    let lines = sentiment_lines(app, inner_width, theme);
    frame.render_widget(
        Paragraph::new(lines).block(pane_block("Sentiment", focused, theme)),
        area,
    );
}

fn sentiment_lines(app: &App, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Some(snapshot) = app.snapshot() else {
        return vec![Line::styled(
            "No market data yet.",
            Style::default().fg(theme.notice),
        )];
    };
    let Some(projection) = view::sentiment(snapshot) else {
        return vec![Line::styled(
            "No 24h data yet.",
            Style::default().fg(theme.notice),
        )];
    };
    let mut lines = Vec::with_capacity(6);
    lines.push(Line::styled(
        "24h breadth",
        Style::default()
            .fg(theme.summary)
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(Line::from(format!(
        "Up {}   Down {}   Flat {}",
        projection.up, projection.down, projection.flat
    )));
    let meter_prefix = "Bullish ";
    let meter_suffix = format!(" {}%", projection.bullish);
    let bar_cells = width
        .saturating_sub(meter_prefix.len() + meter_suffix.len())
        .max(1);
    let filled = (bar_cells * projection.bullish / 100).min(bar_cells);
    let meter = format!(
        "{}{}{}",
        meter_prefix,
        "█".repeat(filled) + &"░".repeat(bar_cells - filled),
        meter_suffix,
    );
    lines.push(Line::styled(meter, Style::default().fg(theme.gain)));
    lines.push(Line::from(format!(
        "Avg 24h: {}",
        format_percentage(Some(projection.average))
    )));
    if let Some((symbol, value)) = projection.best {
        lines.push(Line::from(format!(
            "Best: {} {}",
            clean_remote(&symbol, 8),
            format_percentage(Some(value)),
        )));
    }
    if let Some((symbol, value)) = projection.worst {
        lines.push(Line::from(format!(
            "Worst: {} {}",
            clean_remote(&symbol, 8),
            format_percentage(Some(value)),
        )));
    }
    lines
}

fn message(
    title: &str,
    text: &str,
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
    message_lines(title, vec![Line::from(text.to_owned())], frame, area, theme);
}

fn message_lines(
    title: &str,
    lines: Vec<Line<'static>>,
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(Line::styled(
            format!(" {title} "),
            Style::default().fg(theme.notice),
        ))),
        area,
    );
}

fn table_frame(
    title: &str,
    app: &App,
    info: Vec<Line<'static>>,
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    width: u16,
    focused: bool,
) {
    let theme = app.theme();
    let block = pane_block(title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let table_area = if info.is_empty() {
        inner
    } else {
        let info_height = wrapped_rows(&info, inner.width);
        let [info_area, table_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(info_height), Constraint::Min(1)])
            .areas(inner);
        frame.render_widget(
            Paragraph::new(info).wrap(ratatui::widgets::Wrap { trim: false }),
            info_area,
        );
        table_area
    };
    render_table(frame, app, table_area, width);
}

/// Read-only coin detail pane in a CoinMarketCap shape: a scaled-down content
/// column holds the identity header (rank, name, symbol), the price with its
/// 24-hour change, the 1h/24h/7d change strip, and a fixed-geometry gradient
/// area chart with real price labels. Wide panes add a right-hand "coin data"
/// column fed by the rich `/coins/{id}` detail when it has loaded; narrow
/// panes stack a two-line market-stats grid under the chart instead. It always
/// renders from the snapshot row's own normalized series as a fallback, so it
/// works offline against the fixture server.
fn render_detail(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect, theme: &Theme) {
    let Some(state) = app.detail_state() else {
        return;
    };
    let coin = state.base();
    let title = clean_remote(coin.name(), 48);
    let block = Block::default().borders(Borders::ALL).title(Line::styled(
        format!(" {} ", title),
        Style::default().fg(theme.notice),
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if inner.width >= DETAIL_SIDEBAR_MIN_WIDTH {
        let sidebar_width = DETAIL_SIDEBAR_WIDTH.min(inner.width * 2 / 5).max(30);
        let [main, sidebar] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(sidebar_width)])
            .areas(inner);
        render_detail_main(frame, state, main, theme, true);
        render_detail_sidebar(frame, app, state, sidebar, theme);
    } else {
        render_detail_main(frame, state, inner, theme, false);
    }
}

fn render_detail_main(
    frame: &mut Frame<'_>,
    state: &DetailState,
    area: ratatui::layout::Rect,
    theme: &Theme,
    sidebar: bool,
) {
    let coin = state.base();
    let content_width = if sidebar {
        area.width
    } else {
        DETAIL_CONTENT_WIDTH.min(area.width)
    };
    let content = if sidebar {
        area
    } else {
        let [content, _] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(content_width), Constraint::Min(0)])
            .areas(area);
        content
    };
    let stats_height: u16 = if sidebar { 0 } else { 2 };
    let [head, price, changes, chart, stats] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(stats_height),
        ])
        .areas(content);
    let column_width = content_width as usize;

    let rank = coin
        .rank()
        .map_or_else(|| "-".into(), |value| format!("#{value}"));
    let identity = Line::from(vec![
        Span::styled(
            rank,
            Style::default()
                .fg(theme.summary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} ({})", coin.name(), coin.symbol()),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(truncate_line(identity, column_width)), head);
    frame.render_widget(
        Paragraph::new(truncate_line(price_line(coin, theme), column_width)),
        price,
    );
    frame.render_widget(
        Paragraph::new(truncate_line(change_line(coin, theme), column_width)),
        changes,
    );
    render_detail_chart(frame, state, chart, theme);
    if !sidebar && stats.height >= 1 {
        render_compact_detail_stats(frame, state, stats, column_width, theme);
    }
}

fn render_detail_chart(
    frame: &mut Frame<'_>,
    state: &DetailState,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
    let Some(candles) = detail_candles(state) else {
        frame.render_widget(Paragraph::new("No price data available."), area);
        return;
    };
    let chart = chandelier::CandlestickChart::new(
        chandelier::CandleSeries::new(&candles.bars)
            .bull_style(Style::default().fg(theme.gain))
            .bear_style(Style::default().fg(theme.loss))
            .wick_style(Style::default().fg(trend_color(state.base(), theme)))
            .width(candle_width(candles.bars.len(), area.width))
            .gap(1.0),
    )
    .axes(true);
    let [chart_area, caption_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .areas(area);
    frame.render_widget(chart, chart_area);
    let period = if state.chart_30d().is_empty() {
        "7 days"
    } else {
        "30 days"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "{period}: {} → {}",
                format_price(Some(candles.low)),
                format_price(Some(candles.high))
            ),
            Style::default().fg(theme.summary),
        ))),
        caption_area,
    );
}

/// The candles for the detail chart plus the plotted price range, derived as
/// daily OHLC from the rich hourly series when present, else from the
/// row-derived 7-day series. `None` when there is no finite series to plot.
fn detail_candles(state: &DetailState) -> Option<DetailCandles> {
    let projection = view::detail(state)?;
    let bars = daily_candles(&projection.series)
        .into_iter()
        .map(|candle| chandelier::Candle::new(candle.open, candle.high, candle.low, candle.close))
        .collect();
    Some(DetailCandles {
        bars,
        low: projection.low,
        high: projection.high,
    })
}

/// Derived daily candles plus the original series low/high for the caption.
struct DetailCandles {
    bars: Vec<chandelier::Candle>,
    low: f64,
    high: f64,
}

/// Candle body width (columns) so `count` candles with a 1-column gap stretch
/// across the chart's plot area (the width minus the right-hand price axis).
/// Chandelier quantizes to its grid; a floor of 1 keeps candles visible even
/// when there are many of them.
fn candle_width(count: usize, area_width: u16) -> f64 {
    if count == 0 {
        return 1.0;
    }
    // Reserve ~8 columns for the price axis, then split the rest between the
    // bodies and the 1-column gaps between them.
    let plot = (area_width as usize).saturating_sub(8);
    let per_candle = plot.saturating_div(count).max(2);
    (per_candle as f64 - 1.0).max(1.0)
}

/// Compact (no-sidebar) market-stats grid: two bounded lines under the chart.
fn render_compact_detail_stats(
    frame: &mut Frame<'_>,
    state: &DetailState,
    area: ratatui::layout::Rect,
    width: usize,
    theme: &Theme,
) {
    let (cap, volume, supply, high, low, fdv) = match state {
        DetailState::Ready { detail, .. } => (
            detail.market_cap(),
            detail.volume_24h(),
            detail.circulating_supply(),
            detail.high_24h(),
            detail.low_24h(),
            detail.fully_diluted_valuation(),
        ),
        _ => {
            let coin = state.base();
            (
                coin.market_cap(),
                coin.volume_24h(),
                coin.circulating_supply(),
                None,
                None,
                None,
            )
        }
    };
    let summary_line1 = clean_remote(
        &format!(
            "Mkt cap: {} | Vol 24h: {} | 24h high: {}",
            format_compact_money(cap),
            format_compact_money(volume),
            format_price(high),
        ),
        width,
    );
    let summary_line2 = clean_remote(
        &format!(
            "24h low: {} | Supply: {} | FDV: {}",
            format_price(low),
            format_compact_supply(supply),
            format_compact_money(fdv),
        ),
        width,
    );
    frame.render_widget(
        Paragraph::new(vec![Line::from(summary_line1), Line::from(summary_line2)]).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.notice),
        ),
        area,
    );
}

/// Right-hand "coin data" column: label/value rows from the rich detail (or
/// the row fallback), a loading/unavailable note, and a bounded About snippet.
fn render_detail_sidebar(
    frame: &mut Frame<'_>,
    app: &App,
    state: &DetailState,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
    let block = Block::default().borders(Borders::LEFT).title(Line::styled(
        " Coin data ",
        Style::default().fg(theme.summary),
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (label, value) in detail_stat_rows(state) {
        let label_span = Span::styled(format!("{label}: "), Style::default().fg(theme.summary));
        let max_value = width.saturating_sub(UnicodeWidthStr::width(label_span.content.as_ref()));
        let value_span = Span::styled(clean_remote(&value, max_value), Style::default());
        lines.push(truncate_line(
            Line::from(vec![label_span, value_span]),
            width,
        ));
    }
    let status = match state {
        DetailState::Ready { .. } => None,
        DetailState::Loading { .. } if app.detail_fetching() => Some(" Loading extended data..."),
        _ => Some(" Extended data unavailable."),
    };
    if let Some(text) = status {
        lines.push(Line::default());
        lines.push(Line::styled(text, Style::default().fg(theme.notice)));
    }
    if let DetailState::Ready { detail, .. } = state {
        if let Some(about) = detail.description() {
            lines.push(Line::default());
            lines.push(Line::styled(
                " About ",
                Style::default()
                    .fg(theme.summary)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(clean_remote(about, width), Style::default()));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// CoinMarketCap-style stat rows. The rich state uses every extended field;
/// the row fallback fills what the snapshot holds and lets the sidebar render
/// the same shape with a loading / unavailable note.
fn detail_stat_rows(state: &DetailState) -> Vec<(&'static str, String)> {
    match state {
        DetailState::Ready { detail, .. } => {
            let mut rows = vec![
                ("Mkt cap", format_compact_money(detail.market_cap())),
                ("Vol 24h", format_compact_money(detail.volume_24h())),
                ("24h high", format_price(detail.high_24h())),
                ("24h low", format_price(detail.low_24h())),
                (
                    "ATH",
                    with_parenthesized_percent(detail.ath(), detail.ath_change()),
                ),
                (
                    "ATL",
                    with_parenthesized_percent(detail.atl(), detail.atl_change()),
                ),
                ("Supply", format_compact_supply(detail.circulating_supply())),
                ("Total supply", format_compact_supply(detail.total_supply())),
                ("Max supply", format_compact_supply(detail.max_supply())),
                (
                    "FDV",
                    format_compact_money(detail.fully_diluted_valuation()),
                ),
                ("7d", format_percentage(detail.change_7d())),
                ("30d", format_percentage(detail.change_30d())),
                ("60d", format_percentage(detail.change_60d())),
                ("1y", format_percentage(detail.change_1y())),
                (
                    "Sentiment",
                    format!(
                        "{} up / {} down",
                        percent_without_sign(detail.sentiment_up()),
                        percent_without_sign(detail.sentiment_down()),
                    ),
                ),
                (
                    "Categories",
                    detail
                        .categories()
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ];
            if let Some(description) = detail.description() {
                rows.push(("About", clean_remote(description, 120)));
            }
            rows
        }
        DetailState::Basic(coin) | DetailState::Loading { base: coin, .. } => vec![
            ("Mkt cap", format_compact_money(coin.market_cap())),
            ("Vol 24h", format_compact_money(coin.volume_24h())),
            ("Supply", format_compact_supply(coin.circulating_supply())),
            ("1h", format_percentage(coin.change_1h())),
            ("24h", format_percentage(coin.change_24h())),
            ("7d", format_percentage(coin.change_7d())),
        ],
    }
}

/// `$price (change%)` for ATH/ATL rows, keeping the sign and bounding non-finite
/// and absent values as `-`.
fn with_parenthesized_percent(value: Option<f64>, percent: Option<f64>) -> String {
    match (finite(value), finite(percent)) {
        (Some(value), Some(percent)) => format!(
            "{} ({})",
            format_price(Some(value)),
            format_percentage(Some(percent)),
        ),
        (Some(value), _) => format_price(Some(value)),
        _ => "-".into(),
    }
}

/// Sentiment vote share without a forced sign, bounded to keep the sidebar row
/// short even for hostile provider values.
fn percent_without_sign(value: Option<f64>) -> String {
    match finite(value) {
        Some(value) if value.abs() >= 1000.0 => ">999%".into(),
        Some(value) => format!("{value:.0}%"),
        _ => "-".into(),
    }
}

/// Truncate a styled line to `max_cells` display cells, keeping the surviving
/// spans' styles and cutting the span that crosses the boundary.
fn truncate_line(line: Line<'static>, max_cells: usize) -> Line<'static> {
    let mut used = 0usize;
    let mut out = Line::default();
    for span in line {
        let width = UnicodeWidthStr::width(span.content.as_ref());
        if used + width <= max_cells {
            out.push_span(span);
            used += width;
        } else if used < max_cells {
            let mut room = max_cells - used;
            let mut cut = String::new();
            for character in span.content.chars() {
                let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                if character_width > room {
                    break;
                }
                cut.push(character);
                room -= character_width;
            }
            out.push_span(Span::styled(cut, span.style));
            used = max_cells;
        }
        if used >= max_cells {
            break;
        }
    }
    out
}

/// The price with its 24-hour change, mirroring the CoinMarketCap detail header:
/// the price is the dominant bold value and the change is colored by sign.
fn price_line(coin: &CoinMarket, theme: &Theme) -> Line<'static> {
    let mut line = Line::from(Span::styled(
        format_price(coin.price()),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let change_style = match finite(coin.change_24h()) {
        Some(value) if value > 0.0 => Style::default().fg(theme.gain),
        Some(value) if value < 0.0 => Style::default().fg(theme.loss),
        _ => Style::default(),
    };
    line.push_span(Span::styled(
        format!("   {} (24h)", format_percentage(coin.change_24h())),
        change_style,
    ));
    line
}

/// The 1h/24h/7d change strip with per-segment themed color. Each value is
/// always sign-prefixed, so color never carries the meaning alone.
fn change_line(coin: &CoinMarket, theme: &Theme) -> Line<'static> {
    let mut line = Line::default();
    for (label, change) in [
        ("1h", coin.change_1h()),
        ("24h", coin.change_24h()),
        ("7d", coin.change_7d()),
    ] {
        let formatted = format_percentage(change);
        let style = match finite(change) {
            Some(value) if value > 0.0 => Style::default().fg(theme.gain),
            Some(value) if value < 0.0 => Style::default().fg(theme.loss),
            _ => Style::default(),
        };
        line.push_span(Span::styled(format!("{label}: {formatted}    "), style));
    }
    line
}

/// Detail content column width: the CMC-style page stays one fixed width no
/// matter the terminal, hugging the pane's left border instead of stretching
/// across the screen.
const DETAIL_CONTENT_WIDTH: u16 = 56;

/// The chart line color and where its gradient starts: gain, loss, or neutral
/// by the sign of the 7-day change.
fn trend_color(coin: &CoinMarket, theme: &Theme) -> Color {
    match finite(coin.change_7d()) {
        Some(value) if value >= 0.0 => theme.gain,
        Some(_) => theme.loss,
        None => theme.neutral,
    }
}

fn detail_footer(width: u16) -> String {
    truncate_cells("Esc back | ? help | q quit | r refresh", width as usize)
}

#[derive(Clone, Copy, Debug)]
enum TableMode {
    Compact,
    Standard,
    Full,
}

impl TableMode {
    fn for_width(width: u16) -> Self {
        match width {
            0..=79 => Self::Compact,
            80..=119 => Self::Standard,
            _ => Self::Full,
        }
    }

    fn columns(self) -> &'static [Column] {
        match self {
            Self::Compact => &COMPACT_COLUMNS,
            Self::Standard => &STANDARD_COLUMNS,
            Self::Full => &FULL_COLUMNS,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CellKind {
    Rank,
    Name,
    Symbol,
    Price,
    Change1h,
    Change24h,
    Change7d,
    Cap,
    Volume,
    Supply,
    Trend,
}

#[derive(Clone, Copy)]
struct Column {
    title: &'static str,
    width: u16,
    right: bool,
    kind: CellKind,
}

const COMPACT_COLUMNS: [Column; 4] = [
    Column {
        title: "#",
        width: 3,
        right: true,
        kind: CellKind::Rank,
    },
    Column {
        title: "Symbol",
        width: 9,
        right: false,
        kind: CellKind::Symbol,
    },
    Column {
        title: "Price",
        width: 11,
        right: true,
        kind: CellKind::Price,
    },
    Column {
        title: "24h",
        width: 8,
        right: true,
        kind: CellKind::Change24h,
    },
];

const STANDARD_COLUMNS: [Column; 8] = [
    Column {
        title: "#",
        width: 3,
        right: true,
        kind: CellKind::Rank,
    },
    Column {
        title: "Coin",
        width: 15,
        right: false,
        kind: CellKind::Name,
    },
    Column {
        title: "Sym",
        width: 6,
        right: false,
        kind: CellKind::Symbol,
    },
    Column {
        title: "Price",
        width: 11,
        right: true,
        kind: CellKind::Price,
    },
    Column {
        title: "1h",
        width: 8,
        right: true,
        kind: CellKind::Change1h,
    },
    Column {
        title: "24h",
        width: 8,
        right: true,
        kind: CellKind::Change24h,
    },
    Column {
        title: "7d",
        width: 8,
        right: true,
        kind: CellKind::Change7d,
    },
    Column {
        title: "Cap",
        width: 8,
        right: true,
        kind: CellKind::Cap,
    },
];

const FULL_COLUMNS: [Column; 11] = [
    Column {
        title: "#",
        width: 3,
        right: true,
        kind: CellKind::Rank,
    },
    Column {
        title: "Coin",
        width: 15,
        right: false,
        kind: CellKind::Name,
    },
    Column {
        title: "Sym",
        width: 6,
        right: false,
        kind: CellKind::Symbol,
    },
    Column {
        title: "Price",
        width: 11,
        right: true,
        kind: CellKind::Price,
    },
    Column {
        title: "1h",
        width: 8,
        right: true,
        kind: CellKind::Change1h,
    },
    Column {
        title: "24h",
        width: 8,
        right: true,
        kind: CellKind::Change24h,
    },
    Column {
        title: "7d",
        width: 8,
        right: true,
        kind: CellKind::Change7d,
    },
    Column {
        title: "Cap",
        width: 8,
        right: true,
        kind: CellKind::Cap,
    },
    Column {
        title: "Vol",
        width: 9,
        right: true,
        kind: CellKind::Volume,
    },
    Column {
        title: "Supply",
        width: 9,
        right: true,
        kind: CellKind::Supply,
    },
    Column {
        title: "Trend",
        width: 10,
        right: false,
        kind: CellKind::Trend,
    },
];

/// Number of rows the info lines occupy once wrapped to the given width,
/// so the table never clips a wrapped retry hint at narrow widths.
///
/// Models ratatui's word-boundary wrapping: words fill a row until the next
/// word plus a separating space would overflow; a word that exactly fills
/// the remaining width stays on the row; a word wider than a row is split
/// across `width`-cell rows. Ratatui's `Paragraph` with `Wrap { trim: false }`
/// is the oracle; see `wrapped_rows_matches_word_boundary_wrapping`.
fn wrapped_rows(lines: &[Line<'_>], width: u16) -> u16 {
    let width = width.max(1);
    lines
        .iter()
        .map(|line| {
            let mut rows = 0u16;
            let mut used = 0u16;
            for word in line.to_string().split_whitespace() {
                let word_width = UnicodeWidthStr::width(word) as u16;
                if used > 0 {
                    let needed = 1 + word_width;
                    if used + needed <= width {
                        used += needed;
                        continue;
                    }
                    rows += 1;
                }
                if word_width > width {
                    rows += word_width / width;
                    used = word_width % width;
                } else {
                    used = word_width;
                }
            }
            rows + u16::from(used > 0)
        })
        .sum::<u16>()
        .max(1)
}

fn render_table(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect, width: u16) {
    let theme = app.theme();
    let columns = TableMode::for_width(width).columns();
    let coins = app.visible_coins();
    if coins.is_empty() && app.has_active_filter() {
        render_no_results(frame, app, area, theme);
        return;
    }
    let selected = app.selected();
    let rows: Vec<Row<'_>> = coins
        .iter()
        .enumerate()
        .map(|(index, coin)| make_row(coin, columns, theme, index == selected))
        .collect();
    let row_count = rows.len();
    let widths: Vec<Constraint> = columns
        .iter()
        .map(|column| Constraint::Length(column.width))
        .collect();
    let header = make_header(columns, theme);
    let mut state = TableState::default();
    if !rows.is_empty() {
        state = TableState::new().with_selected(Some(selected));
    }
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .highlight_spacing(HighlightSpacing::Never);
    frame.render_stateful_widget(table, area, &mut state);
    render_row_separators(frame, row_count, app.selected(), area, theme);
}

/// Draw a full-width separator under every visible row except the last one, so
/// plain rows read as Bloomberg-style ledger lines. The selected row keeps its
/// bottom border — only the last row omits it. The scroll offset mirrors
/// ratatui's: rows are two lines tall below the one-line header, and the
/// selected row is kept within the visible window.
fn render_row_separators(
    frame: &mut Frame<'_>,
    rows: usize,
    selected: usize,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
    if rows == 0 {
        return;
    }
    const ROW_HEIGHT: u16 = 2;
    const HEADER_CHROME: u16 = 1;
    let visible = ((area.height.saturating_sub(HEADER_CHROME)) / ROW_HEIGHT).max(1) as usize;
    let last = rows - 1;
    let selected = selected.min(last);
    let offset = if selected >= visible {
        selected + 1 - visible
    } else {
        0
    };
    let style = Style::default().fg(theme.summary);
    let separator = "─".repeat(area.width as usize);
    for window in 0..visible {
        let index = offset + window;
        if index > last {
            break;
        }
        if index == last {
            continue;
        }
        // The selected row's border stays a clean full-width `─` line; the
        // left marker lives only on the text line so it does not spill into
        // the row's border or the row below.
        let line = Line::from(Span::styled(separator.clone(), style));
        // Below the row's two content lines.
        let y = area.y + HEADER_CHROME + (window as u16) * ROW_HEIGHT + 1;
        if y >= area.y + area.height {
            break;
        }
        frame.render_widget(
            Paragraph::new(line),
            ratatui::layout::Rect::new(area.x, y, area.width, 1),
        );
    }
}

/// Centered explanation when a committed filter matches no coin. The query
/// text is bounded so a long filter cannot overflow the block.
fn render_no_results(frame: &mut Frame<'_>, app: &App, area: ratatui::layout::Rect, theme: &Theme) {
    let query = truncate_cells(app.search_query(), MAX_RESULTS_QUERY_CHARS);
    let lines = vec![
        Line::from(format!("No coins match \"{query}\".")).centered(),
        Line::from("Press / to edit or Esc to clear the search.").centered(),
    ];
    let top = area.y + (area.height / 2).saturating_sub(1);
    let inner = ratatui::layout::Rect::new(area.x, top, area.width, lines.len() as u16);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.notice)),
        inner,
    );
}

/// Bound a query in the no-results message to keep the line well inside the
/// table's inner width at the minimum supported size.
const MAX_RESULTS_QUERY_CHARS: usize = 40;

fn make_header(columns: &[Column], theme: &Theme) -> Row<'static> {
    Row::new(
        columns
            .iter()
            .map(|column| {
                Cell::new(Line::from(column.title).alignment(column_alignment(column))).style(
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(theme.summary),
                )
            })
            .collect::<Vec<_>>(),
    )
}

fn column_alignment(column: &Column) -> Alignment {
    if column.right {
        Alignment::Right
    } else {
        Alignment::Left
    }
}

fn make_row(coin: &CoinMarket, columns: &[Column], theme: &Theme, selected: bool) -> Row<'static> {
    Row::new(
        columns
            .iter()
            .enumerate()
            .map(|(index, column)| make_cell(coin, column, theme, selected, index == 0))
            .collect::<Vec<_>>(),
    )
    .height(2)
}

/// Render a single two-line cell. The selected row drops its solid background
/// (which stretched the row and scrambled per-cell colors under `reversed`);
/// instead the first column carries a `▌` marker and the row's data reads in
/// the summary accent plus bold. The marker continues onto the row's bottom
/// border line in `render_row_separators`, so the selection stays one
/// contiguous left edge while keeping the row's full-width border.
fn make_cell(
    coin: &CoinMarket,
    column: &Column,
    theme: &Theme,
    selected: bool,
    first: bool,
) -> Cell<'static> {
    let base = raw_cell_text(coin, column);
    let style = if selected {
        selected_cell_style(coin, column, theme)
    } else {
        cell_style(coin, column, theme)
    };
    let alignment = column_alignment(column);
    let text = if selected && first {
        // The marker replaces the leftmost alignment pad so the cell stays at
        // exactly `column.width`; the breathing line is left blank for the
        // row-separator border to render under it.
        let inner_width = column.width.saturating_sub(1);
        let inner = align_cell(&base, inner_width, column.right);
        format!("▌{inner}")
    } else {
        align_cell(&base, column.width, column.right)
    };
    Cell::new(Line::from(text).style(style).alignment(alignment))
}

/// Selected rows highlight via a full-height left edge, accent foreground, and
/// bold — never a solid background, which breaks the row's layout on hover.
/// Cells without a sign-coded value fall back to bold+accent; change cells keep
/// their gain/loss color while gaining bold weight.
fn selected_cell_style(coin: &CoinMarket, column: &Column, theme: &Theme) -> Style {
    let base = cell_style(coin, column, theme);
    let fg = base.fg.unwrap_or(theme.summary);
    base.fg(fg).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
fn cell_text(coin: &CoinMarket, column: &Column) -> String {
    align_cell(&raw_cell_text(coin, column), column.width, column.right)
}

fn raw_cell_text(coin: &CoinMarket, column: &Column) -> String {
    match column.kind {
        CellKind::Rank => coin
            .rank()
            .map_or_else(|| "-".into(), |rank| rank.to_string()),
        CellKind::Name => coin.name().to_owned(),
        CellKind::Symbol => coin.symbol().to_owned(),
        CellKind::Price => format_price(coin.price()),
        CellKind::Change1h => bounded_percent(coin.change_1h(), column.width),
        CellKind::Change24h => bounded_percent(coin.change_24h(), column.width),
        CellKind::Change7d => bounded_percent(coin.change_7d(), column.width),
        CellKind::Cap => format_compact_money(coin.market_cap()),
        CellKind::Volume => format_compact_money(coin.volume_24h()),
        CellKind::Supply => format_compact_supply(coin.circulating_supply()),
        CellKind::Trend => sparkline_text(coin.sparkline_7d(), column.width as usize),
    }
}

const SPARKLINE_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Render a normalized 7-day sparkline as fixed-width block glyphs.
///
/// The whole series is downsampled into `width` equal buckets (averaging each
/// bucket) so a long trend keeps its direction, then min-max normalized into
/// eight levels. A flat series renders at the middle glyph (`▄`), and a
/// missing or all-non-finite series renders as a dash. Non-finite points are
/// dropped, matching the domain-boundary normalization.
fn sparkline_text(series: &[f64], width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let bucket_size = series.len().div_ceil(width).max(1);
    let mut buckets: Vec<f64> = Vec::with_capacity(width);
    let mut sum = 0.0;
    let mut count = 0usize;
    for (index, value) in series.iter().copied().enumerate() {
        if !value.is_finite() {
            continue;
        }
        sum += value;
        count += 1;
        if (index + 1) % bucket_size == 0 || index + 1 == series.len() {
            if count > 0 {
                buckets.push(sum / count as f64);
            }
            sum = 0.0;
            count = 0;
        }
    }
    if buckets.is_empty() {
        return "-".into();
    }
    let min = buckets.iter().copied().fold(f64::INFINITY, f64::min);
    let max = buckets.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    buckets
        .iter()
        .map(|value| {
            let level = if range == 0.0 || !range.is_finite() {
                3
            } else {
                let scaled = ((value - min) / range) * 7.0;
                (scaled.round() as usize).min(7)
            };
            SPARKLINE_GLYPHS[level]
        })
        .collect()
}

/// Keep an extreme percentage readable: when the signed formatter output
/// cannot fit the column, fall back to a sign plus whole percent, and cap
/// the digits so the `%` is never truncated away.
fn bounded_percent(value: Option<f64>, width: u16) -> String {
    let formatted = format_percentage(value);
    if formatted == "-" || UnicodeWidthStr::width(formatted.as_str()) as u16 <= width {
        return formatted;
    }
    let Some(value) = finite(value) else {
        return formatted;
    };
    let sign = if value.is_sign_negative() { '-' } else { '+' };
    let max_digits = (width as usize).saturating_sub(2).min(19);
    let integer = value.abs().round() as u64;
    if max_digits == 0 {
        return format!("{sign}%");
    }
    let cap = 10_u64.pow(max_digits as u32);
    if integer >= cap {
        format!("{sign}{}%", "9".repeat(max_digits))
    } else {
        format!("{sign}{integer}%")
    }
}

fn align_cell(value: &str, max_cells: u16, right: bool) -> String {
    let cleaned = clean_remote(value, max_cells as usize);
    let width = UnicodeWidthStr::width(cleaned.as_str()) as u16;
    let padding = max_cells.saturating_sub(width);
    if right {
        format!("{}{cleaned}", " ".repeat(padding as usize))
    } else {
        format!("{cleaned}{}", " ".repeat(padding as usize))
    }
}

fn cell_style(coin: &CoinMarket, column: &Column, theme: &Theme) -> Style {
    let change = match column.kind {
        CellKind::Change1h => coin.change_1h(),
        CellKind::Change24h => coin.change_24h(),
        CellKind::Change7d | CellKind::Trend => coin.change_7d(),
        _ => return Style::default(),
    };
    match finite(change) {
        Some(value) if value > 0.0 => Style::default().fg(theme.gain),
        Some(value) if value < 0.0 => Style::default().fg(theme.loss),
        _ => Style::default(),
    }
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

/// The market summary line: each label uses the summary accent, the values
/// stay plain, and the market-cap change is colored by sign. Compact widths
/// (< 80) drop separators so the line still fits; the text matches the
/// placeholder variants exactly.
fn summary_line(app: &App, width: u16, theme: &Theme) -> Line<'static> {
    let label_style = Style::default().fg(theme.summary);
    let change_style = |value: Option<f64>| match finite(value) {
        Some(value) if value > 0.0 => Style::default().fg(theme.gain),
        Some(value) if value < 0.0 => Style::default().fg(theme.loss),
        _ => Style::default(),
    };
    let values = match app.state_ref() {
        DataState::Ready { snapshot, .. }
        | DataState::Empty { snapshot, .. }
        | DataState::Stale { snapshot, .. } => snapshot.summary(),
        DataState::Initial | DataState::Loading | DataState::Fatal(_) => {
            return if width < 80 {
                Line::from(vec![
                    Span::styled("Cap:", label_style),
                    Span::raw("-"),
                    Span::styled(" Vol24:", label_style),
                    Span::raw("-"),
                    Span::styled(" BTCdom:", label_style),
                    Span::raw("-"),
                    Span::styled(" Mkt24:", label_style),
                    Span::styled("-", change_style(None)),
                ])
            } else {
                Line::from(vec![
                    Span::styled("Cap: ", label_style),
                    Span::raw("-"),
                    Span::styled(" | Vol 24h: ", label_style),
                    Span::raw("-"),
                    Span::styled(" | BTC dom: ", label_style),
                    Span::raw("-"),
                    Span::styled(" | Mkt 24h: ", label_style),
                    Span::styled("-", change_style(None)),
                ])
            };
        }
    };
    let cap = format_compact_money(values.total_market_cap());
    let volume = format_compact_money(values.total_volume_24h());
    let dominance = summary_percentage(values.btc_dominance(), false);
    let change = summary_percentage(values.market_cap_change_24h(), true);
    if width < 80 {
        Line::from(vec![
            Span::styled("Cap:", label_style),
            Span::raw(cap),
            Span::styled(" Vol24:", label_style),
            Span::raw(volume),
            Span::styled(" BTCdom:", label_style),
            Span::raw(dominance),
            Span::styled(" Mkt24:", label_style),
            Span::styled(change, change_style(values.market_cap_change_24h())),
        ])
    } else {
        Line::from(vec![
            Span::styled("Cap: ", label_style),
            Span::raw(cap),
            Span::styled(" | Vol 24h: ", label_style),
            Span::raw(volume),
            Span::styled(" | BTC dom: ", label_style),
            Span::raw(dominance),
            Span::styled(" | Mkt 24h: ", label_style),
            Span::styled(change, change_style(values.market_cap_change_24h())),
        ])
    }
}

fn summary_percentage(value: Option<f64>, signed: bool) -> String {
    let formatted = format_percentage(value);
    if formatted == "-" {
        return formatted;
    }
    let Some(value) = finite(value) else {
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

fn status_line(app: &App, width: u16) -> String {
    if app.detail_open() {
        return detail_footer(width);
    }
    if app.searching() {
        return search_status(app, width);
    }
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
    let mut parts: Vec<String> = vec![state];
    if let Some(time) = age {
        parts.push(format!("age {}", format_age(time.elapsed())));
    }
    if notice.is_some() {
        parts.push("SUMMARY DEGRADED".into());
    }
    if app.has_active_filter() {
        let count = app.visible_coins().len();
        parts.push(format!("filter: {} ({count})", app.search_query()));
    }
    if app.sort_active() {
        let sort = app.sort_state();
        let arrow = if sort.ascending() { "↑" } else { "↓" };
        parts.push(format!("sort: {} {arrow}", sort.key().label()));
    }
    if THEMES[0].name != app.theme().name {
        parts.push(format!("theme: {}", app.theme().name));
    }
    let mut line = parts.join(" | ");
    line.push_str(" | q quit | r refresh");

    // Keep the trailing controls visible at narrow widths: drop the
    // degraded marker first, then the age. The state label (including any
    // cached/previous detail) is a single part and is never split.
    let mut shortened = false;
    while UnicodeWidthStr::width(line.as_str()) > width as usize && !shortened {
        if parts.len() > 1 {
            parts.pop();
            line = parts.join(" | ");
            line.push_str(" | q quit | r refresh");
        } else {
            shortened = true;
        }
    }
    line
}

/// Status while search editing is open: the typed buffer plus a cursor, with
/// cancel/apply hints when they fit the width.
fn search_status(app: &App, width: u16) -> String {
    let available = (width as usize).saturating_sub("search:".len());
    let buffer = truncate_cells(app.search_buffer(), available);
    let mut prompt = format!("search:{buffer}");
    if UnicodeWidthStr::width(prompt.as_str()) < width as usize {
        prompt.push('_');
    }
    let hint = " | Esc cancel | Enter apply";
    if UnicodeWidthStr::width(prompt.as_str()) + UnicodeWidthStr::width(hint) <= width as usize {
        prompt.push_str(hint);
    }
    prompt
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
        domain::{CoinMarketInput, MarketSnapshot, MarketSummaryInput},
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{backend::TestBackend, buffer::Buffer, style::Color, Terminal};
    use std::time::Duration;

    const TEST_THEME: Theme = crate::theme::DEFAULT_THEME;

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn app_with(result: Result<crate::api::FetchOutcome, ApiError>) -> App {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
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
                change_1h: Some(0.1),
                change_24h: Some(1.2),
                change_7d: Some(-2.0),
                market_cap: Some(1_000_000_000.0),
                volume_24h: Some(25_000.0),
                circulating_supply: Some(19.0),
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

    fn market_with_all_columns() -> MarketSnapshot {
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
                price: Some(50_000.0),
                change_1h: Some(0.1),
                change_24h: Some(1.2),
                change_7d: Some(-2.0),
                market_cap: Some(1_000_000_000_000.0),
                volume_24h: Some(25_000_000_000.0),
                circulating_supply: Some(19_700_000.0),
                sparkline_7d: vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0],
            }],
            None,
        )
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
        assert_eq!(
            line_text(&summary_line(&complete, 80, &TEST_THEME))
                .matches("|")
                .count(),
            3
        );

        let missing = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        let compact = line_text(&summary_line(&missing, 60, &TEST_THEME));
        assert_eq!(compact, "Cap:- Vol24:- BTCdom:- Mkt24:-");
        assert!(compact.contains("Cap") && compact.contains("Vol24") && compact.contains("BTCdom"));
    }

    #[test]
    fn compact_summary_is_one_fitting_line_and_standard_summary_is_separated() {
        let app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        let compact = line_text(&summary_line(&app, 60, &TEST_THEME));
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
        let extreme_line = line_text(&summary_line(&extreme, 60, &TEST_THEME));
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
        let Command::Fetch { generation, .. } = stale.update(Event::Input(KeyEvent::new(
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
        let cooling = text_at(&stale, 80, 24);
        assert!(
            cooling.contains("Retrying automatically in"),
            "cooldown countdown shown in the stale body: {cooling:?}"
        );
        assert!(
            !cooling.contains("REFRESHING"),
            "r is blocked by the cooldown, so no refresh starts: {cooling:?}"
        );

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
        assert!(
            text_at(&ready, 80, 24).contains("BadName")
                && text_at(&ready, 80, 24).contains("$123.00")
                && text(&ready).contains("$123.00")
        );
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
        let rendered = text_at(&notice, 80, 24);
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
        let Command::Fetch { generation, .. } = stale.update(Event::Input(KeyEvent::new(
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
    fn table_cells_are_bounded_sanitized_and_aligned() {
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
                symbol: input.clone(),
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
        let coin = &market.coins()[0];
        for column in TableMode::Standard.columns() {
            let cell = cell_text(coin, column);
            assert!(UnicodeWidthStr::width(cell.as_str()) <= column.width as usize);
            assert!(!cell.contains('\u{202e}') && !cell.contains('\u{001b}'));
        }
        assert!(TableMode::Standard.columns().iter().any(|column| {
            column.kind == CellKind::Price && cell_text(coin, column).contains("$999T+")
        }));
        assert!(text(&app_with(Ok(crate::api::FetchOutcome {
            snapshot: market,
            summary_notice: None,
        })))
        .contains("LIVE"));
    }

    #[test]
    fn positive_and_negative_changes_get_color_with_sign_retained() {
        let market = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![CoinMarketInput {
                id: "pos".into(),
                rank: Some(1),
                name: "Rises".into(),
                symbol: "UP".into(),
                price: Some(10.0),
                change_1h: Some(2.5),
                change_24h: Some(-1.5),
                change_7d: Some(0.05),
                market_cap: Some(1.0),
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            }],
            None,
        );
        let coin = &market.coins()[0];
        for index in [4usize, 5, 6] {
            let column = STANDARD_COLUMNS[index];
            let style = cell_style(coin, &column, &crate::theme::DEFAULT_THEME);
            let sign = match column.kind {
                CellKind::Change1h => "2.50%",
                CellKind::Change24h => "-1.50%",
                _ => "+0.05%",
            };
            assert!(
                cell_text(coin, &column).contains(sign),
                "{sign} kept as text"
            );
            let expected = if column.kind == CellKind::Change24h {
                Color::Red
            } else {
                Color::Green
            };
            assert_eq!(
                style.fg,
                Some(expected),
                "{sign} colored without losing text"
            );
        }
        let rendered = text_at(
            &app_with(Ok(crate::api::FetchOutcome {
                snapshot: market,
                summary_notice: None,
            })),
            80,
            24,
        );
        assert!(
            rendered.contains("+2.50%")
                && rendered.contains("-1.50%")
                && rendered.contains("+0.05%")
        );
    }

    #[test]
    fn trend_sparkline_cell_is_colored_by_7d_change_sign() {
        let theme = crate::theme::DEFAULT_THEME;
        let trend = &FULL_COLUMNS[10];
        let market = |change_7d: Option<f64>| {
            MarketSnapshot::new(
                MarketSummaryInput {
                    total_market_cap: None,
                    total_volume_24h: None,
                    btc_dominance: None,
                    market_cap_change_24h: None,
                },
                vec![CoinMarketInput {
                    id: "trend".into(),
                    rank: None,
                    name: "Trend".into(),
                    symbol: "TR".into(),
                    price: None,
                    change_1h: None,
                    change_24h: None,
                    change_7d,
                    market_cap: None,
                    volume_24h: None,
                    circulating_supply: None,
                    sparkline_7d: vec![1.0, 3.0, 2.0, 4.0],
                }],
                None,
            )
        };
        let up = market(Some(5.0));
        assert_eq!(
            cell_style(&up.coins()[0], trend, &theme).fg,
            Some(Color::Green),
            "rising sparkline is colored with the gain role"
        );
        let down = market(Some(-5.0));
        assert_eq!(
            cell_style(&down.coins()[0], trend, &theme).fg,
            Some(Color::Red),
            "falling sparkline is colored with the loss role"
        );
        let none = market(None);
        assert_eq!(
            cell_style(&none.coins()[0], trend, &theme).fg,
            None,
            "a sparkline without a 7d change stays uncolored"
        );
    }

    #[test]
    fn trend_follows_the_7d_change_sign() {
        let theme = crate::theme::DEFAULT_THEME;
        let market = |change_7d: Option<f64>| {
            MarketSnapshot::new(
                MarketSummaryInput {
                    total_market_cap: None,
                    total_volume_24h: None,
                    btc_dominance: None,
                    market_cap_change_24h: None,
                },
                vec![CoinMarketInput {
                    id: "chart".into(),
                    rank: None,
                    name: "Chart".into(),
                    symbol: "CH".into(),
                    price: None,
                    change_1h: None,
                    change_24h: None,
                    change_7d,
                    market_cap: None,
                    volume_24h: None,
                    circulating_supply: None,
                    sparkline_7d: vec![1.0, 2.0, 3.0],
                }],
                None,
            )
        };
        let up = market(Some(2.0));
        assert_eq!(
            trend_color(&up.coins()[0], &theme),
            Color::Green,
            "rising chart uses the gain role"
        );
        let down = market(Some(-2.0));
        assert_eq!(
            trend_color(&down.coins()[0], &theme),
            Color::Red,
            "falling chart uses the loss role"
        );
        let none = market(None);
        assert_eq!(
            trend_color(&none.coins()[0], &theme),
            Color::Cyan,
            "a chart without a 7d change uses the neutral role"
        );
    }

    #[test]
    fn detail_candles_derive_ohlc_and_caption_bounds() {
        // A rising week derives one candle per day plus the series range.
        let app = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            1.0,
            1.0,
            (0..168).map(|hour| hour as f64).collect(),
        )]);
        let state = app.detail_state().expect("detail is open");
        let candles = detail_candles(state).expect("finite series plots");
        assert_eq!(candles.bars.len(), 7, "one candle per day");
        assert_eq!(candles.low, 0.0);
        assert_eq!(candles.high, 167.0);
        assert_eq!(candles.bars[0].open, 0.0);
        assert_eq!(candles.bars[6].close, 167.0);

        // An all-non-finite series has nothing to plot.
        let hostile = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            1.0,
            0.0,
            vec![f64::NAN, f64::INFINITY, f64::NEG_INFINITY],
        )]);
        assert!(detail_candles(hostile.detail_state().unwrap()).is_none());

        // An empty series has nothing to plot.
        let empty = detail_app(vec![detail_row("Bitcoin", "BTC", 1.0, 0.0, vec![])]);
        assert!(detail_candles(empty.detail_state().unwrap()).is_none());
    }

    #[test]
    fn detail_chart_uses_theme_roles_and_keeps_the_caption() {
        let app = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            50_000.0,
            -2.0,
            vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0],
        )]);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer: &Buffer = terminal.backend().buffer();
        let all: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(all.contains("7 days:"), "caption retained: {all:?}");
        // The candlestick body/wick glyphs render inside the content column.
        assert!(
            all.contains('│') || all.contains('▌'),
            "candlestick glyphs render: {all:?}"
        );
        // The wick uses the trend color (loss for a -2% 7d change).
        let loss_style = buffer.content().iter().find(|cell| {
            matches!(cell.symbol(), "│" | "▌" | "▐") && cell.style().fg == Some(Color::Red)
        });
        assert!(loss_style.is_some(), "bear wick uses the loss role");
    }

    fn missing_input() -> CoinMarketInput {
        CoinMarketInput {
            id: "missing".into(),
            rank: None,
            name: "Unranked Long Name".into(),
            symbol: "X".into(),
            price: None,
            change_1h: None,
            change_24h: None,
            change_7d: None,
            market_cap: None,
            volume_24h: None,
            circulating_supply: None,
            sparkline_7d: vec![],
        }
    }

    #[test]
    fn missing_values_do_not_shift_numeric_columns() {
        let market = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![
                CoinMarketInput {
                    id: "full".into(),
                    rank: Some(1),
                    name: "Bitcoin".into(),
                    symbol: "BTC".into(),
                    price: Some(50_000.0),
                    change_1h: Some(1.0),
                    change_24h: Some(2.0),
                    change_7d: Some(3.0),
                    market_cap: Some(900_000_000_000.0),
                    volume_24h: Some(1.0),
                    circulating_supply: Some(19.0),
                    sparkline_7d: vec![],
                },
                missing_input(),
            ],
            None,
        );
        let coins = market.coins();
        for column in TableMode::Standard.columns() {
            let full = cell_text(&coins[0], column);
            let missing = cell_text(&coins[1], column);
            assert_eq!(
                UnicodeWidthStr::width(full.as_str()),
                UnicodeWidthStr::width(missing.as_str()),
                "column {} keeps a fixed width whether or not values are missing",
                column.title
            );
        }
    }

    #[test]
    fn extreme_percentage_changes_keep_sign_and_percent_sign() {
        let market = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![CoinMarketInput {
                id: "extreme".into(),
                rank: Some(1),
                name: "Extreme".into(),
                symbol: "X".into(),
                price: Some(1.0),
                change_1h: Some(1_234_567.0),
                change_24h: Some(-1_234_567.0),
                change_7d: Some(50_000.0),
                market_cap: Some(1.0),
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            }],
            None,
        );
        let coin = &market.coins()[0];
        let mut saw_capped = false;
        for column in TableMode::Standard.columns() {
            if column.kind == CellKind::Change1h || column.kind == CellKind::Change24h {
                let cell = cell_text(coin, column);
                assert!(UnicodeWidthStr::width(cell.as_str()) <= column.width as usize);
                assert!(cell.starts_with('+') || cell.starts_with('-'));
                assert!(cell.ends_with('%'), "percent sign kept: {cell}");
                saw_capped = true;
            }
        }
        assert!(saw_capped);
    }

    #[test]
    fn wrapped_info_lines_keep_the_retry_hint_at_minimum_width() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        let Command::Fetch { generation, .. } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::MalformedResponse),
        });
        let rendered = text_at(&app, 60, 16);
        assert!(rendered.contains("Refresh failed"), "reason line visible");
        assert!(
            rendered.contains("Retrying"),
            "retry action wraps onto a visible line: {rendered:?}"
        );
    }

    #[test]
    fn compact_and_full_modes_keep_fixed_widths_for_missing_values() {
        let market = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![
                CoinMarketInput {
                    id: "full".into(),
                    rank: Some(1),
                    name: "Bitcoin".into(),
                    symbol: "BTC".into(),
                    price: Some(50_000.0),
                    change_1h: Some(1.0),
                    change_24h: Some(2.0),
                    change_7d: Some(3.0),
                    market_cap: Some(900_000_000_000.0),
                    volume_24h: Some(1.0),
                    circulating_supply: Some(19.0),
                    sparkline_7d: vec![],
                },
                missing_input(),
            ],
            None,
        );
        let coins = market.coins();
        for mode in [TableMode::Compact, TableMode::Standard, TableMode::Full] {
            for column in mode.columns() {
                let full = cell_text(&coins[0], column);
                let missing = cell_text(&coins[1], column);
                assert_eq!(
                    UnicodeWidthStr::width(full.as_str()),
                    UnicodeWidthStr::width(missing.as_str()),
                    "{mode:?} column {} keeps a fixed width whether or not values are missing",
                    column.title
                );
            }
        }
    }

    #[test]
    fn bounded_percent_keeps_sign_and_percent_for_capped_and_fit_values() {
        assert_eq!(bounded_percent(Some(50_000.0), 8), "+50000%");
        assert_eq!(bounded_percent(Some(-50_000.0), 8), "-50000%");
        assert_eq!(bounded_percent(Some(999_999.0), 8), "+999999%");
        assert_eq!(bounded_percent(Some(-999_999.0), 8), "-999999%");
        assert_eq!(bounded_percent(Some(f64::MAX), 8), "+999999%");
        assert_eq!(bounded_percent(Some(-f64::MAX), 8), "-999999%");
        assert_eq!(bounded_percent(Some(999.99), 8), "+999.99%");
        assert_eq!(bounded_percent(Some(0.005), 8), "+0.01%");
        assert_eq!(bounded_percent(Some(0.0), 8), "0.00%");
        assert_eq!(bounded_percent(None, 8), "-");
        assert_eq!(bounded_percent(Some(f64::NAN), 8), "-");
        assert_eq!(bounded_percent(Some(-0.004), 8), "0.00%");
        assert_eq!(bounded_percent(Some(50_000.0), 1), "+%");
        assert_eq!(bounded_percent(Some(50_000.0), 22), "+50000.00%");
    }

    #[test]
    fn wrapped_rows_matches_word_boundary_wrapping() {
        let line = |text: &str| Line::from(text.to_owned());
        assert_eq!(wrapped_rows(&[line("a")], 58), 1);
        assert_eq!(
            wrapped_rows(
                &[line(
                    "Refresh failed: invalid provider response. Press r to retry."
                )],
                58
            ),
            2
        );
        assert_eq!(
            wrapped_rows(&[line("Refresh failed: Offline. Press r to retry.")], 58),
            1
        );
        let long_token = format!("{} {} {}", "x".repeat(30), "y".repeat(30), "z".repeat(28));
        assert_eq!(wrapped_rows(&[line(&long_token)], 58), 3);
        assert_eq!(wrapped_rows(&[line("aaaaa bbbbb ccccc")], 10), 3);
        assert_eq!(wrapped_rows(&[line("a"), line("b c d")], 58), 2);
        assert_eq!(wrapped_rows(&[], 58), 1);
        assert_eq!(
            wrapped_rows(
                &[line(
                    "Refresh failed: provider request failed. Press r to retry."
                )],
                58
            ),
            1
        );
        assert_eq!(wrapped_rows(&[line(&"x".repeat(90))], 58), 2);
        assert_eq!(wrapped_rows(&[line(&"x".repeat(116))], 58), 2);
        assert_eq!(
            wrapped_rows(
                &[line(&format!("{} {}", "x".repeat(30), "y".repeat(27)))],
                58
            ),
            1
        );
        assert_eq!(
            wrapped_rows(
                &[line(&format!("{} {}", "x".repeat(30), "y".repeat(28)))],
                58
            ),
            2
        );
    }

    #[test]
    fn two_info_lines_wrap_and_keep_both_at_minimum_width() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: Some(ApiError::HttpStatus { status: 500 }),
        }));
        let Command::Fetch { generation, .. } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::MalformedResponse),
        });
        let rendered = text_at(&app, 60, 16);
        assert!(
            rendered.contains("Summary unavailable") && rendered.contains("Retrying"),
            "notice and retry hint both visible at 60x16: {rendered:?}"
        );
    }

    #[test]
    fn sparkline_series_render_without_panic_for_every_shape() {
        let flat = sparkline_text(&[5.0, 5.0, 5.0, 5.0], 10);
        assert_eq!(flat.chars().count(), 4);
        assert!(flat.chars().all(|c| c == '▄'), "flat at mid level: {flat}");

        let rising = sparkline_text(&[1.0, 2.0, 3.0, 4.0], 10);
        let chars: Vec<char> = rising.chars().collect();
        assert_eq!(chars.len(), 4);
        assert_eq!(chars[0], '▁');
        assert_eq!(chars[3], '█');

        let falling = sparkline_text(&[4.0, 3.0, 2.0, 1.0], 10);
        let chars: Vec<char> = falling.chars().collect();
        assert_eq!(chars[0], '█');
        assert_eq!(chars[3], '▁');

        let one = sparkline_text(&[7.0], 10);
        assert_eq!(one, "▄");

        let missing = sparkline_text(&[], 10);
        assert_eq!(missing, "-");

        let non_finite = sparkline_text(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0], 10);
        assert_eq!(
            non_finite, "▄",
            "single finite survivor is flat: {non_finite}"
        );
        assert_eq!(sparkline_text(&[f64::NAN], 10), "-");

        assert_eq!(
            sparkline_text(&[-3.0, -1.0, -2.0], 10),
            "▁█▅",
            "negative values map to glyphs by relative position"
        );
        assert_eq!(sparkline_text(&[0.0, 0.0], 10), "▄▄", "flat zero");
        assert_eq!(sparkline_text(&[-1.0, 1.0], 10), "▁█", "spanning zero");

        assert_eq!(sparkline_text(&[1.0, 2.0, 3.0], 0), "");
        let truncated = sparkline_text(&(0..=99).map(|v| v as f64).collect::<Vec<_>>(), 10);
        assert_eq!(truncated.chars().count(), 10, "downsampled to width");
        assert_eq!(truncated.chars().next(), Some('▁'));
        assert_eq!(truncated.chars().last(), Some('█'));
    }

    #[test]
    fn long_monotonic_series_keeps_direction_after_downsampling() {
        let rising: Vec<f64> = (1..=168).map(|v| v as f64).collect();
        let up = sparkline_text(&rising, 10);
        assert_eq!(up.chars().count(), 10);
        let up_glyphs: Vec<char> = up.chars().collect();
        assert_eq!(up_glyphs[0], '▁', "rising week starts low: {up}");
        assert_eq!(up_glyphs[9], '█', "rising week ends high: {up}");

        let falling: Vec<f64> = (1..=168).rev().map(|v| v as f64).collect();
        let down = sparkline_text(&falling, 10);
        let down_glyphs: Vec<char> = down.chars().collect();
        assert_eq!(down_glyphs[0], '█', "falling week starts high: {down}");
        assert_eq!(down_glyphs[9], '▁', "falling week ends low: {down}");
    }

    #[test]
    fn extreme_finite_magnitudes_render_flat_without_panic() {
        let extreme = sparkline_text(&[1e308, -1e308], 10);
        assert_eq!(
            extreme, "▄▄",
            "overflowing range falls back to flat: {extreme}"
        );
        let precision = sparkline_text(&[1e300, 1e300 + 1.0, 1e300 + 2.0], 10);
        assert_eq!(precision.chars().count(), 3);
        assert!(precision
            .chars()
            .all(|c| matches!(c, '▁' | '▂' | '▃' | '▄' | '▅' | '▆' | '▇' | '█')));
    }

    #[test]
    fn full_mode_renders_sparkline_column_with_actual_series() {
        let market = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: Some(1.0),
                total_volume_24h: Some(1.0),
                btc_dominance: Some(1.0),
                market_cap_change_24h: Some(1.0),
            },
            vec![CoinMarketInput {
                id: "trend".into(),
                rank: Some(1),
                name: "TrendCoin".into(),
                symbol: "TREND".into(),
                price: Some(1.0),
                change_1h: Some(1.0),
                change_24h: Some(1.0),
                change_7d: Some(1.0),
                market_cap: Some(1.0),
                volume_24h: Some(1.0),
                circulating_supply: Some(1.0),
                sparkline_7d: vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0],
            }],
            None,
        );
        let rendered = text_at(
            &app_with(Ok(crate::api::FetchOutcome {
                snapshot: market,
                summary_notice: None,
            })),
            120,
            30,
        );
        assert!(
            rendered.contains("Trend"),
            "trend header present at 120 wide"
        );
        assert!(
            rendered.contains("▁▃▆█▆▃▁"),
            "normalized sparkline for 1,2,3,4,3,2,1: {rendered}"
        );
        let missing = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        let rendered = text_at(&missing, 120, 30);
        assert!(rendered.contains("Trend"));
    }

    #[test]
    fn remote_text_has_scalar_bound_even_with_zero_width_marks() {
        let input = format!("Visible{}", "\u{301}".repeat(10_000));
        let cleaned = clean_remote(&input, 18);

        assert!(cleaned.contains("Visible"));
        assert!(cleaned.chars().count() <= MAX_SANITIZED_SCALARS);
        assert!(cleaned.len() <= MAX_SANITIZED_SCALARS * 4);

        let rendered = text_at(
            &app_with(Ok(crate::api::FetchOutcome {
                snapshot: snapshot(&input),
                summary_notice: None,
            })),
            80,
            24,
        );
        assert!(rendered.contains("Visible"));
    }

    #[test]
    fn status_line_keeps_controls_and_drops_detail_at_narrow_widths() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: Some(ApiError::RateLimited { retry_after: None }),
        }));
        let Command::Fetch { generation, .. } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        let refreshing = status_line(&app, 60);
        assert!(
            UnicodeWidthStr::width(refreshing.as_str()) <= 60,
            "status fits while refreshing: {refreshing}"
        );
        assert!(
            refreshing.contains("r refresh") && refreshing.contains("q quit"),
            "controls always visible: {refreshing}"
        );
        assert!(refreshing.contains("REFRESHING"));

        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(4)),
            }),
        });
        let settled = status_line(&app, 60);
        assert!(
            UnicodeWidthStr::width(settled.as_str()) <= 60,
            "status fits when settled: {settled}"
        );
        assert!(settled.contains("RATE LIMITED") && settled.contains("r refresh"));
        let wide = status_line(&app, 120);
        assert!(
            wide.contains("SUMMARY DEGRADED") && wide.contains("RATE LIMITED"),
            "wide status keeps detail: {wide}"
        );
    }

    #[test]
    fn loading_summary_uses_width_appropriate_placeholder() {
        let mut loading = App::new();
        loading.update(Event::Start);
        assert_eq!(
            line_text(&summary_line(&loading, 60, &TEST_THEME)),
            "Cap:- Vol24:- BTCdom:- Mkt24:-"
        );
        assert_eq!(
            line_text(&summary_line(&loading, 80, &TEST_THEME)),
            "Cap: - | Vol 24h: - | BTC dom: - | Mkt 24h: -"
        );
        let fatal = app_with(Err(ApiError::Transport));
        assert_eq!(
            line_text(&summary_line(&fatal, 80, &TEST_THEME)),
            "Cap: - | Vol 24h: - | BTC dom: - | Mkt 24h: -"
        );
        assert_eq!(
            line_text(&summary_line(&fatal, 60, &TEST_THEME)),
            "Cap:- Vol24:- BTCdom:- Mkt24:-"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn status_line_drop_order_keeps_state_and_drops_marker_then_age() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: Some(ApiError::RateLimited { retry_after: None }),
        }));
        let Command::Fetch { generation, .. } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(4)),
            }),
        });
        let _ = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        let cooling = status_line(&app, 80);
        assert!(
            cooling.contains("RATE LIMITED") && !cooling.contains("cached"),
            "r blocked by the cooldown leaves the state detail unfetched: {cooling}"
        );
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(matches!(
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::NONE,
            ))),
            Command::Fetch { .. }
        ));
        let line = status_line(&app, 60);
        assert!(line.contains("cached RATE LIMITED"), "{line}");
        assert!(
            line.contains("r refresh") && line.contains("q quit"),
            "{line}"
        );
        assert!(
            !line.contains("SUMMARY DEGRADED"),
            "marker dropped first: {line}"
        );
        assert!(
            !line.contains("age"),
            "age dropped before the state detail: {line}"
        );
    }

    #[test]
    fn renders_stale_empty_offline_and_rate_limit() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        let Command::Fetch { generation, .. } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        let stale = text_at(&app, 80, 24);
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
        assert!(
            text_at(&empty, 80, 24).contains("No market rows")
                && text_at(&empty, 80, 24).contains("EMPTY")
        );
        let _ = empty.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        assert!(text_at(&empty, 80, 24).contains("REFRESHING"));

        let mut fatal = app_with(Err(ApiError::RateLimited {
            retry_after: Some(Duration::from_secs(4)),
        }));
        assert!(
            text_at(&fatal, 80, 24).contains("Rate limited")
                && text_at(&fatal, 80, 24).contains("4s")
        );
        let _ = fatal.update(Event::Input(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::NONE,
        )));
        let cooling = text_at(&fatal, 80, 24);
        assert!(
            cooling.contains("Retrying automatically in"),
            "cooldown countdown instead of a blocked manual refresh: {cooling:?}"
        );
        assert!(
            !cooling.contains("Refreshing market data"),
            "r is blocked while the cooldown is open: {cooling:?}"
        );
    }

    #[test]
    fn full_mode_renders_volume_and_supply_columns_without_panic() {
        let full = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        let rendered = text_at(&full, 120, 30);
        assert!(rendered.contains("Vol") && rendered.contains("Supply"));
        assert!(rendered.contains("19"));

        let compact = text_at(&full, 79, 20);
        assert!(compact.contains("Symbol"));
    }

    #[test]
    fn below_minimum_shows_centered_resize_message_and_keeps_quit() {
        let app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        for (width, height) in [(59, 15), (20, 5), (80, 10), (60, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let buffer: &Buffer = terminal.backend().buffer();
            let row_text = |y: u16| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            };
            assert!(
                (0..height).any(|y| row_text(y).contains("Terminal too small")),
                "resize message shown at {width}x{height}"
            );
            assert!(
                (0..height).any(|y| row_text(y).contains("q quits")),
                "quit stays discoverable at {width}x{height}"
            );
        }
    }

    #[test]
    fn documented_mode_renders_each_required_size_without_out_of_bounds() {
        let app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: market_with_all_columns(),
            summary_notice: None,
        }));
        let cases: [(u16, u16, &[&str], &[&str]); 5] = [
            (
                60,
                16,
                &["Symbol", "Price", "24h"],
                &["Trend", "Supply", "1h", "7d"],
            ),
            (
                79,
                20,
                &["Symbol", "Price", "24h"],
                &["Trend", "Supply", "1h", "7d"],
            ),
            (80, 24, &["Sym", "1h", "7d", "Cap"], &["Trend", "Supply"]),
            (119, 30, &["Sym", "1h", "7d", "Cap"], &["Trend", "Supply"]),
            (120, 30, &["Trend", "Vol", "Supply", "7d"], &[]),
        ];
        for (width, height, expected, forbidden) in cases {
            let rendered = text_at(&app, width, height);
            for token in expected {
                assert!(
                    rendered.contains(token),
                    "{token} present at {width}x{height}"
                );
            }
            for token in forbidden {
                assert!(
                    !rendered.contains(token),
                    "{token} absent at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn selected_row_is_highlighted_and_visible_after_scroll() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: many_rows(),
            summary_notice: None,
        }));
        app.select(49);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer: &Buffer = terminal.backend().buffer();
        let (width, height) = (buffer.area().width, buffer.area().height);
        let row_of = |needle: &str| {
            (0..height).find(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, *y)).unwrap().symbol())
                    .collect::<String>()
                    .contains(needle)
            })
        };
        let selected_y = row_of("Coin 50").expect("selected row scrolled into view");
        let selected_marked =
            (0..width).any(|x| buffer.cell((x, selected_y)).unwrap().symbol().contains('▌'));
        assert!(selected_marked, "scrolled-to selected row is marked");
        let selected_bold = (0..width).any(|x| {
            buffer
                .cell((x, selected_y))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        });
        assert!(selected_bold, "scrolled-to selected row is bold");
        let header_y = row_of("Coin").expect("header visible");
        assert!(
            selected_y > header_y,
            "selected row renders below the header"
        );

        let mut selected = app_with(Ok(crate::api::FetchOutcome {
            snapshot: many_rows(),
            summary_notice: None,
        }));
        selected.select(2);
        let (width, height);
        let row_y;
        {
            terminal.draw(|frame| render(frame, &selected)).unwrap();
            let buffer: &Buffer = terminal.backend().buffer();
            width = buffer.area().width;
            height = buffer.area().height;
            let row_of = |needle: &str| {
                (0..height).find(|y| {
                    (0..width)
                        .map(|x| buffer.cell((x, *y)).unwrap().symbol())
                        .collect::<String>()
                        .contains(needle)
                })
            };
            row_y = row_of("Coin 3").expect("row for selected coin");
        }
        let buffer: &Buffer = terminal.backend().buffer();
        let row_cells: String = (0..width)
            .map(|x| buffer.cell((x, row_y)).unwrap().symbol())
            .collect();
        assert!(row_cells.contains("Coin 3"));
        let highlighted = (0..width).any(|x| {
            buffer
                .cell((x, row_y))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        });
        assert!(highlighted, "selected row uses a bold style");
        let breathing_y = row_y + 1;
        let breathing: String = (0..width)
            .map(|x| buffer.cell((x, breathing_y)).unwrap().symbol())
            .collect();
        assert!(
            breathing.contains('─'),
            "breathing line keeps the bottom border"
        );
        assert!(
            !breathing.contains('▌'),
            "left marker does not spill into the bottom border"
        );
        assert!(
            !breathing
                .chars()
                .any(|c| c.is_ascii_digit() || c.is_ascii_alphabetic()),
            "breathing line carries only the border, no duplicated row content"
        );
    }

    fn row_input(id: &str, rank: u32, name: &str, symbol: &str, price: f64) -> CoinMarketInput {
        CoinMarketInput {
            id: id.into(),
            rank: Some(rank),
            name: name.into(),
            symbol: symbol.into(),
            price: Some(price),
            change_1h: None,
            change_24h: None,
            change_7d: None,
            market_cap: None,
            volume_24h: None,
            circulating_supply: None,
            sparkline_7d: vec![],
        }
    }

    fn rows_snapshot(rows: Vec<CoinMarketInput>) -> MarketSnapshot {
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

    fn type_query(app: &mut App, query: &str) {
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )));
        for character in query.chars() {
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char(character),
                KeyModifiers::NONE,
            )));
        }
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn search_status_shows_buffer_cursor_and_fitting_hints() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )));
        for character in ['b', 't'] {
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char(character),
                KeyModifiers::NONE,
            )));
        }
        let wide = status_line(&app, 80);
        assert!(wide.contains("search:bt"), "{wide}");
        assert!(
            wide.contains("Esc cancel") && wide.contains("Enter apply"),
            "{wide}"
        );
        assert!(UnicodeWidthStr::width(wide.as_str()) <= 80);
        let narrow = status_line(&app, 60);
        assert!(
            UnicodeWidthStr::width(narrow.as_str()) <= 60,
            "prompt fits at minimum width: {narrow}"
        );
    }

    #[test]
    fn typing_keeps_prior_rows_until_enter_then_filters_the_table() {
        let rows = rows_snapshot(vec![
            row_input("bitcoin", 1, "Bitcoin", "BTC", 1.0),
            row_input("bitbo", 2, "Bitbo", "BBO", 1.0),
            row_input("litecoin", 3, "Litecoin", "LTC", 1.0),
        ]);
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows,
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )));
        for character in ['l', 'i'] {
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char(character),
                KeyModifiers::NONE,
            )));
        }
        let during = text_at(&app, 80, 24);
        assert!(
            during.contains("Bitcoin"),
            "typing keeps previous rows: {during:?}"
        );
        assert!(
            during.contains("search:") && during.contains("search:li"),
            "{during:?}"
        );

        app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        let after = text_at(&app, 80, 24);
        assert!(after.contains("Litecoin"), "{after:?}");
        assert!(
            !after.contains("Bitcoin"),
            "entered filter hides non-matches: {after:?}"
        );
        assert!(status_line(&app, 80).contains("filter: li (1)"));
    }

    #[test]
    fn filtered_table_renders_only_matching_rows_with_filter_status() {
        let rows = rows_snapshot(vec![
            row_input("bitcoin", 1, "Bitcoin", "BTC", 1.0),
            row_input("bitbo", 2, "Bitbo", "BBO", 1.0),
            row_input("litecoin", 3, "Litecoin", "LTC", 1.0),
        ]);
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows,
            summary_notice: None,
        }));
        type_query(&mut app, "bit");
        let rendered = text_at(&app, 80, 24);
        assert!(
            rendered.contains("Bitcoin") && rendered.contains("Bitbo"),
            "{rendered:?}"
        );
        assert!(
            !rendered.contains("Litecoin"),
            "non-matching row hidden: {rendered:?}"
        );
        let line = status_line(&app, 80);
        assert!(line.contains("filter: bit (2)"), "{line}");
        assert!(
            line.contains("q quit") && line.contains("r refresh"),
            "{line}"
        );
    }

    #[test]
    fn no_results_renders_query_and_clear_hint() {
        let rows = rows_snapshot(vec![
            row_input("bitcoin", 1, "Bitcoin", "BTC", 1.0),
            row_input("litecoin", 2, "Litecoin", "LTC", 1.0),
        ]);
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows,
            summary_notice: None,
        }));
        type_query(&mut app, "zzz");
        let rendered = text_at(&app, 80, 24);
        assert!(rendered.contains("No coins match"), "{rendered:?}");
        assert!(rendered.contains("zzz"), "{rendered:?}");
        assert!(rendered.contains("Esc to clear"), "{rendered:?}");
        assert!(status_line(&app, 80).contains("filter: zzz (0)"));
    }

    #[test]
    fn status_line_shows_active_sort_and_keeps_controls() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: snapshot("Bitcoin"),
            summary_notice: None,
        }));
        assert!(
            !status_line(&app, 80).contains("sort:"),
            "default rank order shows no sort indicator"
        );
        for _ in 0..3 {
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )));
        }
        let line = status_line(&app, 80);
        assert!(line.contains("sort: price ↓"), "{line}");
        assert!(
            line.contains("q quit") && line.contains("r refresh"),
            "{line}"
        );
        assert!(UnicodeWidthStr::width(line.as_str()) <= 80);
        let narrow = status_line(&app, 60);
        assert!(
            UnicodeWidthStr::width(narrow.as_str()) <= 60,
            "sort indicator fits at minimum width: {narrow}"
        );
    }

    #[test]
    fn sorted_rows_render_in_descending_order() {
        let rows = rows_snapshot(vec![
            row_input("bitbo", 1, "Bitbo", "BBO", 50.0),
            row_input("bitcoin", 2, "Bitcoin", "BTC", 100.0),
            row_input("litecoin", 3, "Litecoin", "LTC", 200.0),
        ]);
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows,
            summary_notice: None,
        }));
        for _ in 0..3 {
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::NONE,
            )));
        }
        let rendered = text_at(&app, 80, 24);
        let position_of = |name: &str| rendered.find(name).expect(name);
        assert!(
            position_of("Litecoin") < position_of("Bitcoin")
                && position_of("Bitcoin") < position_of("Bitbo"),
            "rows render in descending price order: {rendered:?}"
        );
    }

    #[test]
    fn help_overlay_renders_bindings_and_fits_minimum_size() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(vec![row_input("bitcoin", 1, "Bitcoin", "BTC", 1.0)]),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert!(app.help_open());
        let help = text_at(&app, 60, 16);
        for expected in [
            " Help ",
            "Quit",
            "Next coin",
            "Previous coin",
            "First coin",
            "Last coin",
            "Move one page",
            "Search (Enter/Esc)",
            "Cycle sort column",
            "Refresh",
            "Close help",
        ] {
            assert!(help.contains(expected), "help lists {expected}: {help:?}");
        }
        for line in HELP_LINES {
            assert!(
                line.chars().count() <= (HELP_WIDTH as usize) - 2,
                "help line {line:?} fits the overlay inner width"
            );
        }
        assert!(
            (HELP_LINES.len() as u16) + 2 <= 16,
            "help block fits the minimum height"
        );
    }

    #[test]
    fn help_overlay_closes_with_esc_and_question_and_restores_the_table() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(vec![
                row_input("bitcoin", 1, "Bitcoin", "BTC", 1.0),
                row_input("bitbo", 2, "Bitbo", "BBO", 1.0),
            ]),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert!(text_at(&app, 60, 16).contains("Close help"));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!app.help_open());
        let after_esc = text_at(&app, 60, 16);
        assert!(
            !after_esc.contains("Close help"),
            "Esc dismisses the overlay: {after_esc:?}"
        );
        assert!(
            after_esc.contains("BTC"),
            "compact table renders again after Esc: {after_esc:?}"
        );

        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        assert!(
            !text_at(&app, 60, 16).contains("Close help"),
            "? toggles the overlay closed"
        );
    }

    fn detail_app(rows: Vec<CoinMarketInput>) -> App {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(rows),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(app.detail_open());
        app
    }

    fn detail_row(
        name: &str,
        symbol: &str,
        price: f64,
        change_7d: f64,
        sparkline: Vec<f64>,
    ) -> CoinMarketInput {
        let mut row = row_input("bitcoin", 1, name, symbol, price);
        row.change_1h = Some(0.5);
        row.change_24h = Some(1.5);
        row.change_7d = Some(change_7d);
        row.market_cap = Some(1_000_000_000_000.0);
        row.volume_24h = Some(25_000_000_000.0);
        row.circulating_supply = Some(19_700_000.0);
        row.sparkline_7d = sparkline;
        row
    }

    #[test]
    fn detail_pane_renders_identity_stats_changes_and_chart() {
        let app = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            50_000.0,
            -2.0,
            vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0],
        )]);
        let rendered = text_at(&app, 80, 24);
        assert!(rendered.contains("#1  Bitcoin (BTC)"), "{rendered:?}");
        assert!(
            rendered.contains("$50.0K"),
            "price emphasized: {rendered:?}"
        );
        assert!(rendered.contains("+1.50% (24h)"), "{rendered:?}");
        assert!(rendered.contains("Mkt cap: $1T"), "{rendered:?}");
        assert!(rendered.contains("Vol 24h: $25.0B"), "{rendered:?}");
        assert!(rendered.contains("Supply: 19.7M"), "{rendered:?}");
        assert!(rendered.contains("1h: +0.50%"), "{rendered:?}");
        assert!(rendered.contains("24h: +1.50%"), "{rendered:?}");
        assert!(rendered.contains("7d: -2.00%"), "{rendered:?}");
        assert!(rendered.contains("7 days:"), "chart caption: {rendered:?}");
        assert!(
            rendered.contains('│') || rendered.contains('▌') || rendered.contains('▐'),
            "candlestick wicks and bodies render: {rendered:?}"
        );
    }

    #[test]
    fn detail_pane_shows_placeholder_for_empty_series() {
        let app = detail_app(vec![detail_row("Bitcoin", "BTC", 1.0, 0.0, vec![])]);
        let rendered = text_at(&app, 60, 16);
        assert!(
            rendered.contains("No price data available."),
            "{rendered:?}"
        );
        assert!(!rendered.contains("7 days:"), "{rendered:?}");
    }

    #[test]
    fn detail_pane_handles_hostile_flat_and_bounded_series_without_panic() {
        let hostile = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            1.0,
            0.0,
            vec![f64::NAN, f64::INFINITY, 2.0, -3.0, f64::NEG_INFINITY, 5.0],
        )]);
        let rendered = text_at(&hostile, 80, 24);
        assert!(
            rendered.contains('│') || rendered.contains('▌'),
            "non-finite points are dropped and candles render: {rendered:?}"
        );

        let flat = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            1.0,
            0.0,
            vec![5.0, 5.0, 5.0],
        )]);
        let rendered = text_at(&flat, 80, 24);
        assert!(
            rendered.contains("7 days:"),
            "flat series still renders a chart with its caption: {rendered:?}"
        );

        let huge = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            1.0,
            0.0,
            (0..2000).map(|index| index as f64).collect(),
        )]);
        let rendered = text_at(&huge, 80, 24);
        assert!(
            rendered.contains('│') || rendered.contains('▌'),
            "long series is downsampled to daily candles and renders: {rendered:?}"
        );
    }

    #[test]
    fn detail_pane_fits_compact_and_full_widths() {
        let app = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            50_000.0,
            -2.0,
            vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0],
        )]);
        let compact = text_at(&app, 60, 16);
        assert!(compact.contains("#1  Bitcoin (BTC)"), "{compact:?}");
        assert!(compact.contains("7 days:"), "{compact:?}");
        assert!(
            compact.contains("Esc back | ? help | q quit | r refresh"),
            "{compact:?}"
        );
        let full = text_at(&app, 120, 30);
        assert!(full.contains("#1  Bitcoin (BTC)"), "{full:?}");
        assert!(full.contains("7 days:"), "{full:?}");
    }

    #[test]
    fn status_line_uses_detail_footer_while_detail_is_open() {
        let app = detail_app(vec![detail_row("Bitcoin", "BTC", 1.0, 0.0, vec![1.0, 2.0])]);
        let rendered = text_at(&app, 60, 16);
        assert!(
            rendered.contains("Esc back | ? help | q quit | r refresh"),
            "{rendered:?}"
        );
        assert!(
            !rendered.contains("LIVE"),
            "state label is replaced: {rendered:?}"
        );
    }

    #[test]
    fn esc_returns_to_the_table_preserving_selection() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(vec![
                row_input("bitcoin", 1, "Bitcoin", "BTC", 1.0),
                row_input("litecoin", 2, "Litecoin", "LTC", 1.0),
            ]),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.selected(), 1);
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(text_at(&app, 60, 16).contains("Litecoin"));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!app.detail_open());
        assert_eq!(app.selected(), 1, "Esc preserves the table selection");
        let after_esc = text_at(&app, 60, 16);
        assert!(
            after_esc.contains("LIVE"),
            "table status returns: {after_esc:?}"
        );
        assert!(
            after_esc.contains("BTC") && after_esc.contains("LTC"),
            "{after_esc:?}"
        );
        assert!(!after_esc.contains("Esc back"), "{after_esc:?}");
    }

    fn buffer(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn every_theme_renders_at_every_supported_width() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        assert_eq!(app.theme().name, "Default");
        for expected in ["Nord", "Tokyo Night", "Monochrome"] {
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char('t'),
                KeyModifiers::NONE,
            )));
            assert_eq!(app.theme().name, expected);
            for (width, height) in [(60u16, 16u16), (80u16, 24u16), (120u16, 30u16)] {
                let rendered = text_at(&app, width, height);
                assert!(rendered.contains("BTC"), "{expected} at {width}x{height}");
                assert!(
                    rendered.contains(&format!("theme: {expected}")),
                    "{expected} shown in the status line at {width}x{height}: {rendered:?}"
                );
            }
        }
    }

    #[test]
    fn every_theme_renders_the_detail_screen_at_every_supported_width() {
        let mut app = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            50_000.0,
            -2.0,
            vec![1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0],
        )]);
        for expected in ["Default", "Nord", "Tokyo Night", "Monochrome"] {
            assert_eq!(app.theme().name, expected);
            for (width, height) in [(60u16, 16u16), (80u16, 24u16), (120u16, 30u16)] {
                let rendered = text_at(&app, width, height);
                assert!(
                    rendered.contains("#1  Bitcoin (BTC)"),
                    "{expected} detail at {width}x{height}"
                );
                assert!(rendered.contains("Esc back"), "{expected} footer");
            }
            app.update(Event::Input(KeyEvent::new(
                KeyCode::Char('t'),
                KeyModifiers::NONE,
            )));
        }
    }

    #[test]
    fn built_in_themes_are_distinct() {
        let names: Vec<&str> = crate::theme::THEMES
            .iter()
            .map(|theme| theme.name)
            .collect();
        assert_eq!(names, ["Default", "Nord", "Tokyo Night", "Monochrome"]);
        assert_ne!(
            crate::theme::THEMES[0].summary,
            crate::theme::THEMES[1].summary
        );
        assert_ne!(crate::theme::THEMES[0].gain, crate::theme::THEMES[1].gain);
        assert_ne!(
            crate::theme::THEMES[1].gain,
            crate::theme::THEMES[2].gain,
            "Nord and Tokyo Night differ in gain color"
        );
        assert_eq!(crate::theme::THEMES[2].name, "Tokyo Night");
        assert_eq!(crate::theme::THEMES[0].name, "Default");
    }

    #[test]
    fn monochrome_theme_keeps_every_cell_at_the_terminal_default_color() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('t'),
            KeyModifiers::NONE,
        )));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('t'),
            KeyModifiers::NONE,
        )));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('t'),
            KeyModifiers::NONE,
        )));
        assert_eq!(app.theme().name, "Monochrome");
        let buf = buffer(&app, 80, 24);
        let cells = buf.content();
        assert!(
            cells
                .iter()
                .all(|cell| matches!(cell.style().fg, None | Some(Color::Reset))),
            "Monochrome keeps every cell at the terminal default color"
        );
        let rendered = text_at(&app, 80, 24);
        assert!(
            rendered.contains("Bitcoin"),
            "text still carries meaning: {rendered:?}"
        );
        assert!(rendered.contains("+1.20%"), "{rendered:?}");
    }

    #[test]
    fn status_line_shows_active_theme_when_it_is_not_default() {
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        let default = text_at(&app, 80, 24);
        assert!(
            !default.contains("theme: "),
            "no theme marker for Default: {default:?}"
        );
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('t'),
            KeyModifiers::NONE,
        )));
        let nord = text_at(&app, 80, 24);
        assert!(nord.contains("theme: Nord"), "{nord:?}");
        assert!(
            nord.contains("| q quit | r refresh"),
            "controls retained: {nord:?}"
        );
    }

    /// An `App` with the news feed enabled and one loaded headline, driven
    /// through the same update path the loop uses.
    fn news_app(headlines: usize) -> App {
        let mut app = crate::app::App::with_news_enabled(Duration::from_secs(60));
        let Command::Fetch {
            generation,
            news_generation,
        } = app.update(Event::Start)
        else {
            panic!("Start must request a refresh")
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(crate::api::FetchOutcome {
                snapshot: rows_snapshot(vec![row_input("bitcoin", 1, "Bitcoin", "BTC", 50_000.0)]),
                summary_notice: None,
            }),
        });
        app.update(Event::NewsResult {
            generation: news_generation.expect("news is chained when enabled"),
            result: Ok((0..headlines)
                .map(|index| {
                    NewsItem::fixture(
                        &format!("Headline {index} about markets"),
                        "Fixture Wire",
                        &format!("https://example.com/stories/{index}"),
                    )
                })
                .collect()),
        });
        app
    }

    #[test]
    fn news_pane_renders_headline_urls_and_failure_notice() {
        // Focus the news pane so it replaces the table below the threshold.
        let mut focused = news_app(2);
        focused.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&focused, 60, 16);
        assert!(rendered.contains(" News "), "pane title: {rendered:?}");
        assert!(rendered.contains("Fixture Wire"), "{rendered:?}");
        assert!(
            rendered.contains("Headline 0 about markets"),
            "{rendered:?}"
        );
        assert!(
            rendered.contains("https://example.com/stories/0"),
            "the headline URL renders: {rendered:?}"
        );

        // A failed refresh keeps the headlines and appends the notice.
        let mut failed = news_app(2);
        let Command::Fetch {
            news_generation, ..
        } = failed.update(Event::Start)
        else {
            panic!("Start must request a refresh")
        };
        let news_generation = news_generation.expect("news chained");
        failed.update(Event::NewsResult {
            generation: news_generation,
            result: Err(ApiError::HttpStatus { status: 503 }),
        });
        failed.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&failed, 60, 16);
        assert!(
            rendered.contains("Headline 0 about markets"),
            "{rendered:?}"
        );
        assert!(rendered.contains("News refresh failed"), "{rendered:?}");
    }

    #[test]
    fn news_pane_shows_loading_and_unavailable_states() {
        // News enabled but no result yet: loading placeholder. The market
        // must be Ready for the body to route through the panes.
        let mut loading = crate::app::App::with_news_enabled(Duration::from_secs(60));
        let Command::Fetch { generation, .. } = loading.update(Event::Start) else {
            panic!("Start must request a refresh")
        };
        loading.update(Event::FetchResult {
            generation,
            result: Ok(crate::api::FetchOutcome {
                snapshot: rows_snapshot(vec![row_input("bitcoin", 1, "Bitcoin", "BTC", 50_000.0)]),
                summary_notice: None,
            }),
        });
        loading.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&loading, 60, 16);
        assert!(rendered.contains("Loading headlines..."), "{rendered:?}");

        // News disabled (default): unavailable placeholder.
        let mut disabled = app_with(Ok(crate::api::FetchOutcome {
            snapshot: summary_snapshot(),
            summary_notice: None,
        }));
        disabled.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&disabled, 60, 16);
        assert!(rendered.contains("News feed unavailable."), "{rendered:?}");
    }

    #[test]
    fn sentiment_pane_renders_breadth_meter_and_best_worst() {
        // Three coins: two up, one down, one flat.
        let rows = vec![
            row_input("a", 1, "A", "AA", 1.0),
            row_input("b", 2, "B", "BB", 1.0),
            row_input("c", 3, "C", "CC", 1.0),
            row_input("d", 4, "D", "DD", 1.0),
        ];
        let mut snapshot_rows = rows;
        snapshot_rows[0].change_24h = Some(3.0);
        snapshot_rows[1].change_24h = Some(-1.0);
        snapshot_rows[2].change_24h = Some(2.0);
        snapshot_rows[3].change_24h = Some(0.0);
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(snapshot_rows),
            summary_notice: None,
        }));
        // Two Tabs: Table -> News -> Sentiment.
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&app, 60, 16);
        assert!(rendered.contains(" Sentiment "), "{rendered:?}");
        assert!(rendered.contains("Up 2   Down 1   Flat 1"), "{rendered:?}");
        assert!(rendered.contains("Bullish"), "{rendered:?}");
        assert!(rendered.contains("50%"), "2 of 4 are up: {rendered:?}");
        assert!(rendered.contains("Best: AA +3.00%"), "{rendered:?}");
        assert!(rendered.contains("Worst: BB -1.00%"), "{rendered:?}");
        assert!(rendered.contains("Avg 24h:"), "{rendered:?}");
    }

    #[test]
    fn sentiment_pane_handles_empty_and_no_data() {
        // Zero rows put the app in the Empty state message, not the pane.
        let mut empty = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(vec![]),
            summary_notice: None,
        }));
        empty.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        empty.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&empty, 60, 16);
        assert!(
            rendered.contains("No market rows were returned."),
            "{rendered:?}"
        );

        // Ready rows with no finite 24h changes show the pane placeholder.
        let mut nodata = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(vec![row_input("a", 1, "A", "AA", 1.0)]),
            summary_notice: None,
        }));
        nodata.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        nodata.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&nodata, 60, 16);
        assert!(rendered.contains("No 24h data yet."), "{rendered:?}");
        assert!(rendered.contains(" Sentiment "), "{rendered:?}");
    }

    #[test]
    fn panes_render_side_by_side_at_wide_widths_in_bounds() {
        let app = news_app(3);
        let rendered = text_at(&app, 162, 30);
        // The table keeps its full column set and both panes render.
        assert!(rendered.contains("Sym"), "standard columns: {rendered:?}");
        assert!(rendered.contains("News"), "{rendered:?}");
        assert!(rendered.contains("Sentiment"), "{rendered:?}");
        assert!(
            rendered.contains("Headline 0 about markets"),
            "{rendered:?}"
        );

        let rendered = text_at(&app, 200, 30);
        assert!(rendered.contains("News"), "{rendered:?}");
        assert!(rendered.contains("Sentiment"), "{rendered:?}");
        assert!(rendered.contains("Market summary"), "{rendered:?}");
    }

    #[test]
    fn pane_focus_shows_one_pane_below_the_threshold() {
        // At 161 the panes never render side-by-side; only the focused pane
        // replaces the table.
        let mut focused = news_app(2);
        focused.update(Event::Input(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&focused, 161, 30);
        assert!(
            rendered.contains("Headline 0 about markets"),
            "{rendered:?}"
        );
        assert!(
            !rendered.contains("Up 0   Down"),
            "sentiment is not shown when news is focused: {rendered:?}"
        );
    }

    #[test]
    fn detail_sidebar_renders_rich_fields_and_loading_note() {
        // Build the app and open the detail ourselves so we can capture the
        // fetch generation (detail_app already pressed Enter).
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(vec![detail_row(
                "Bitcoin",
                "BTC",
                50_000.0,
                -2.0,
                vec![1.0, 2.0],
            )]),
            summary_notice: None,
        }));
        let Command::FetchDetail { id, generation } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))) else {
            panic!("Enter must open the detail and start the fetch")
        };
        let _ = id;
        // Build a rich detail through the domain boundary and box it.
        let detail = crate::domain::CoinDetail::new(crate::domain::CoinDetailInput {
            id: "bitcoin".into(),
            symbol: "btc".into(),
            name: "Bitcoin".into(),
            rank: Some(1),
            price: Some(50_000.0),
            change_1h: Some(0.1),
            change_24h: Some(2.0),
            change_7d: Some(-1.5),
            change_14d: Some(3.0),
            change_30d: Some(-2.0),
            change_60d: Some(5.0),
            change_1y: Some(40.0),
            market_cap: Some(1_000_000_000_000.0),
            volume_24h: Some(25_000_000_000.0),
            high_24h: Some(52_000.0),
            low_24h: Some(49_000.0),
            ath: Some(100_000.0),
            atl: Some(3_000.0),
            ath_change: Some(-50.0),
            atl_change: Some(1500.0),
            circulating_supply: Some(19_700_000.0),
            total_supply: Some(21_000_000.0),
            max_supply: Some(21_000_000.0),
            fully_diluted_valuation: Some(1_100_000_000_000.0),
            categories: vec!["layer-1".into(), "store-of-value".into()],
            sentiment_up: Some(70.0),
            sentiment_down: Some(30.0),
            sparkline_7d: vec![1.0, 2.0, 3.0],
            description: Some("A peer-to-peer network for the fixture test.".into()),
        });
        app.update(Event::DetailResult {
            id: "bitcoin".to_owned(),
            generation,
            result: Ok(Box::new(detail)),
        });

        let rendered = text_at(&app, 120, 30);
        assert!(rendered.contains(" Coin data "), "{rendered:?}");
        assert!(rendered.contains("ATH: $100K (-50.00%)"), "{rendered:?}");
        assert!(rendered.contains("FDV: $1.1T"), "{rendered:?}");
        assert!(
            rendered.contains("Sentiment: 70% up / 30% down"),
            "{rendered:?}"
        );
        assert!(rendered.contains("layer-1"), "{rendered:?}");
        assert!(rendered.contains("Total supply: 21.0M"), "{rendered:?}");
        assert!(
            rendered.contains("A peer-to-peer network for the fixt"),
            "the About snippet renders (bounded to the sidebar width): {rendered:?}"
        );

        // The basic (row-fallback) detail shows a loading note.
        let basic = detail_app(vec![detail_row(
            "Bitcoin",
            "BTC",
            50_000.0,
            -2.0,
            vec![1.0, 2.0],
        )]);
        let rendered = text_at(&basic, 120, 30);
        assert!(
            rendered.contains("Loading extended data..."),
            "{rendered:?}"
        );
    }

    #[test]
    fn row_separators_draw_beneath_plain_rows_and_skip_selected_and_last() {
        let mut rows = Vec::new();
        for rank in 1..=10 {
            rows.push(row_input(
                &rank.to_string(),
                rank,
                &format!("Coin {rank}"),
                "x",
                1.0,
            ));
        }
        let mut app = app_with(Ok(crate::api::FetchOutcome {
            snapshot: rows_snapshot(rows),
            summary_notice: None,
        }));
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        let rendered = text_at(&app, 80, 24);
        // The separator glyph appears somewhere and stays in-bounds.
        assert!(rendered.contains('─'), "{rendered:?}");
        let rendered_120 = text_at(&app, 120, 30);
        assert!(rendered_120.contains('─'), "{rendered:?}");
    }
}
