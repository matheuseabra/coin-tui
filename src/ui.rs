use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    prelude::Stylize,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, TableState},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_SANITIZED_SCALARS: usize = 256;

use crate::{
    api::ApiError,
    app::{App, DataState},
    domain::CoinMarket,
    format::{
        format_age, format_compact_money, format_compact_supply, format_percentage, format_price,
    },
    theme::{Theme, THEMES},
};

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 16;

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
    if let Some(coin) = app.detail() {
        render_detail(frame, coin, area, app.theme());
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
            table_frame("Market | Live", app, info, frame, area, width);
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
            table_frame("Market | Stale", app, info, frame, area, width);
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
) {
    let theme = app.theme();
    let block = Block::default().borders(Borders::ALL).title(Line::styled(
        format!(" {title} "),
        Style::default().fg(theme.summary),
    ));
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

/// Read-only coin detail pane in a scaled-down CoinMarketCap shape: a left-
/// aligned content column capped at `DETAIL_CONTENT_WIDTH` holds the identity
/// header (rank chip, name, symbol), the price with its 24-hour change, the
/// 1h/24h/7d change strip, a fixed-geometry gradient area chart with real
/// price labels, and the market-stats grid. It renders only from the snapshot
/// row's already-normalized series, so it needs no extra provider call and
/// works offline against the fixture server.
fn render_detail(
    frame: &mut Frame<'_>,
    coin: &CoinMarket,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
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
    let content_width = DETAIL_CONTENT_WIDTH.min(inner.width);
    let [content, _] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(content_width), Constraint::Min(0)])
        .areas(inner);
    let [head, price, changes, chart, stats] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(content);

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
    frame.render_widget(
        Paragraph::new(truncate_line(identity, content_width as usize)),
        head,
    );

    frame.render_widget(
        Paragraph::new(truncate_line(
            price_line(coin, theme),
            content_width as usize,
        )),
        price,
    );
    frame.render_widget(
        Paragraph::new(truncate_line(
            change_line(coin, theme),
            content_width as usize,
        )),
        changes,
    );
    render_price_chart(frame, coin, chart, theme);

    let cap = format_compact_money(coin.market_cap());
    let volume = format_compact_money(coin.volume_24h());
    let supply = format_compact_supply(coin.circulating_supply());
    let stats_line = clean_remote(
        &format!("Mkt cap: {cap} | Vol 24h: {volume} | Supply: {supply}"),
        content_width as usize,
    );
    frame.render_widget(
        Paragraph::new(stats_line).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.notice),
        ),
        stats,
    );
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

/// Maximum number of chart points after downsampling, so a hostile provider
/// series can never unbounded the chart's memory or layout work.
const MAX_CHART_POINTS: usize = 512;

/// Detail content column width: the CMC-style page stays one fixed width no
/// matter the terminal, hugging the pane's left border instead of stretching
/// across the screen.
const DETAIL_CONTENT_WIDTH: u16 = 56;

/// Number of body rows the price chart draws (plus one caption line below).
const CHART_ROWS: u16 = 6;

const MIN_CHART_AREA_WIDTH: u16 = 34;
const MIN_CHART_AREA_HEIGHT: u16 = 4;

/// Number of gradient shades used to fade the area fill away from the line.
const GRADIENT_SHADES: usize = 8;

/// The chart line color and where its gradient starts: gain, loss, or neutral
/// by the sign of the 7-day change.
fn trend_color(coin: &CoinMarket, theme: &Theme) -> Color {
    match finite(coin.change_7d()) {
        Some(value) if value >= 0.0 => theme.gain,
        Some(_) => theme.loss,
        None => theme.neutral,
    }
}

/// A bounded, left-aligned 7-day gradient area chart styled after the
/// CoinMarketCap price chart. The already-filtered series is min-max
/// normalized into `[0.0, 1.0]` (flat or overflow falls back to a mid-line),
/// sampled at half-block resolution, and filled from the line down to the
/// bottom with a gradient that fades away from the line. Real price labels
/// and the range caption use the `accent` color. Returns `None` when the pane
/// is too small or there is no finite series to plot.
fn price_chart_lines(
    series: &[f64],
    area_width: u16,
    area_height: u16,
    trend: Color,
    accent: Color,
) -> Option<Vec<Line<'static>>> {
    if area_width < MIN_CHART_AREA_WIDTH || area_height < MIN_CHART_AREA_HEIGHT {
        return None;
    }
    let (points, low, high) = bounded_chart_points(series);
    if points.is_empty() {
        return None;
    }
    let values = if points.len() == 1 {
        vec![points[0], points[0]]
    } else {
        points
    };
    let rows = CHART_ROWS.min(area_height.saturating_sub(2)).max(2) as usize;

    let mut labels = Vec::with_capacity(rows);
    let mut label_width = 0usize;
    for row in 0..rows {
        let t = if rows == 1 {
            0.0
        } else {
            row as f64 / (rows - 1) as f64
        };
        let price = if high > low {
            high - (high - low) * t
        } else {
            high
        };
        let text = format_price(Some(price));
        label_width = label_width.max(UnicodeWidthStr::width(text.as_str()));
        labels.push(text);
    }
    let graph_width = (area_width as usize).saturating_sub(label_width + 2).max(4);

    let ramp = gradient_ramp(trend, GRADIENT_SHADES);
    let half_rows = rows * 2;
    let mut lines = Vec::with_capacity(rows + 1);
    for (row, label) in labels.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let pad = label_width - UnicodeWidthStr::width(label.as_str());
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled(label.clone(), Style::default().fg(accent)));
        spans.push(Span::raw(" "));
        let mut run: Option<(char, Color)> = None;
        let mut run_len = 0usize;
        for column in 0..graph_width {
            let t = if graph_width == 1 {
                0.0
            } else {
                column as f64 / (graph_width - 1) as f64
            };
            let value = sample_series(&values, t).clamp(0.0, 1.0);
            let line_sub = ((1.0 - value) * (half_rows - 1) as f64).round() as usize;
            let top = row * 2;
            let bottom = top + 1;
            let (glyph, distance) = match (top >= line_sub, bottom >= line_sub) {
                (true, true) => ('█', Some(top - line_sub)),
                (true, false) => ('▀', Some(top - line_sub)),
                (false, true) => ('▄', Some(bottom - line_sub)),
                (false, false) => (' ', None),
            };
            let fill = match distance {
                Some(distance) => ramp[(distance / 2).min(ramp.len() - 1)],
                None => Color::Reset,
            };
            push_run(&mut spans, &mut run, &mut run_len, glyph, fill);
        }
        flush_run(&mut spans, &mut run, &mut run_len);
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        format!(
            "7 days: {} → {}",
            format_price(Some(low)),
            format_price(Some(high))
        ),
        Style::default().fg(accent),
    )));
    Some(lines)
}

/// Sample the normalized series at fraction `t` across `[0.0, 1.0]` of its
/// length, linearly interpolating between points.
fn sample_series(points: &[f64], t: f64) -> f64 {
    let last = (points.len() - 1) as f64;
    let position = (t * last).min(last);
    let left = position.floor() as usize;
    let right = (left + 1).min(points.len() - 1);
    let fraction = position - left as f64;
    points[left] + (points[right] - points[left]) * fraction
}

/// Append `glyph` to the current styled run, merging consecutive cells that
/// share a glyph and color so each chart row stays a small number of spans.
fn push_run(
    spans: &mut Vec<Span<'static>>,
    run: &mut Option<(char, Color)>,
    run_len: &mut usize,
    glyph: char,
    color: Color,
) {
    if *run == Some((glyph, color)) {
        *run_len += 1;
        return;
    }
    flush_run(spans, run, run_len);
    *run = Some((glyph, color));
    *run_len = 1;
}

fn flush_run(spans: &mut Vec<Span<'static>>, run: &mut Option<(char, Color)>, run_len: &mut usize) {
    if let Some((glyph, color)) = run.take() {
        spans.push(Span::styled(
            glyph.to_string().repeat(*run_len),
            Style::default().fg(color),
        ));
    }
}

fn render_price_chart(
    frame: &mut Frame<'_>,
    coin: &CoinMarket,
    area: ratatui::layout::Rect,
    theme: &Theme,
) {
    let Some(lines) = price_chart_lines(
        coin.sparkline_7d(),
        area.width,
        area.height,
        trend_color(coin, theme),
        theme.summary,
    ) else {
        frame.render_widget(Paragraph::new("No 7-day price data available."), area);
        return;
    };
    frame.render_widget(Paragraph::new(lines), area);
}

/// A ramp of `steps` shades from `base` down to a darkened endpoint, so the
/// area fill fades away from the chart line like a CoinMarketCap gradient.
/// Named ANSI colors are mapped to the RGB value typical terminals render so
/// every color theme gets a gradient, while `Color::Reset` (Monochrome) and
/// unknown roles return a solid ramp and stay readable without color.
fn gradient_ramp(base: Color, steps: usize) -> Vec<Color> {
    let steps = steps.max(1);
    let Some((r, g, b)) = color_to_rgb(base) else {
        return vec![base; steps];
    };
    let darken = |channel: u8| (channel as u16 * 35 / 100).min(255) as u8;
    let (er, eg, eb) = (darken(r), darken(g), darken(b));
    (0..steps)
        .map(|i| {
            let t = i as f32 / (steps - 1) as f32;
            Color::Rgb(
                (r as f32 + (er as f32 - r as f32) * t).round() as u8,
                (g as f32 + (eg as f32 - g as f32) * t).round() as u8,
                (b as f32 + (eb as f32 - b as f32) * t).round() as u8,
            )
        })
        .collect()
}

/// Convert a color to RGB for gradient interpolation. Named ANSI colors use
/// the bright xterm-style value typical terminals render; `Reset` and unknown
/// indexed colors return `None` so the ramp stays solid.
fn color_to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    Some(match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (230, 60, 60),
        Color::LightRed => (255, 110, 110),
        Color::Green => (70, 200, 100),
        Color::LightGreen => (120, 230, 150),
        Color::Blue => (70, 130, 235),
        Color::LightBlue => (120, 170, 255),
        Color::Yellow => (235, 190, 60),
        Color::LightYellow => (255, 230, 120),
        Color::Magenta => (200, 90, 190),
        Color::LightMagenta => (240, 140, 230),
        Color::Cyan => (60, 190, 210),
        Color::LightCyan => (120, 220, 235),
        Color::White => (220, 220, 220),
        Color::Gray => (130, 130, 130),
        Color::DarkGray => (85, 85, 85),
        Color::Reset | Color::Indexed(_) => return None,
    })
}

/// Downsample to `MAX_CHART_POINTS` equal buckets, then min-max normalize into
/// `[0.0, 1.0]` so the chart's axis bounds are always finite. Drops non-finite
/// points (matching the domain boundary), and a flat or overflow range renders
/// as a flat mid-line instead of empty bounds. Returns the normalized points
/// and the original low/high prices for the axis label.
fn bounded_chart_points(series: &[f64]) -> (Vec<f64>, f64, f64) {
    let values: Vec<f64> = series
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    if values.is_empty() {
        return (Vec::new(), 0.0, 0.0);
    }
    let downsampled = if values.len() > MAX_CHART_POINTS {
        let bucket_size = values.len().div_ceil(MAX_CHART_POINTS);
        let mut buckets: Vec<f64> = Vec::with_capacity(MAX_CHART_POINTS);
        let mut sum = 0.0;
        let mut count = 0usize;
        for (index, value) in values.iter().copied().enumerate() {
            sum += value;
            count += 1;
            if (index + 1) % bucket_size == 0 || index + 1 == values.len() {
                buckets.push(sum / count as f64);
                sum = 0.0;
                count = 0;
            }
        }
        buckets
    } else {
        values
    };
    let low = downsampled.iter().copied().fold(f64::INFINITY, f64::min);
    let high = downsampled
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let range = high - low;
    if !(range.is_finite() && range != 0.0) {
        return (vec![0.5; downsampled.len()], low, high);
    }
    let scaled = downsampled
        .iter()
        .map(|value| ((value - low) / range).clamp(0.0, 1.0))
        .collect();
    (scaled, low, high)
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
    let rows: Vec<Row<'_>> = coins
        .iter()
        .map(|coin| make_row(coin, columns, theme))
        .collect();
    let widths: Vec<Constraint> = columns
        .iter()
        .map(|column| Constraint::Length(column.width))
        .collect();
    let header = make_header(columns, theme);
    let mut state = TableState::default();
    if !rows.is_empty() {
        state = TableState::new().with_selected(Some(app.selected()));
    }
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .highlight_spacing(HighlightSpacing::Never)
        .row_highlight_style(Style::default().fg(theme.summary).reversed());
    frame.render_stateful_widget(table, area, &mut state);
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

fn make_row(coin: &CoinMarket, columns: &[Column], theme: &Theme) -> Row<'static> {
    Row::new(
        columns
            .iter()
            .map(|column| make_cell(coin, column, theme))
            .collect::<Vec<_>>(),
    )
    .height(2)
}

fn make_cell(coin: &CoinMarket, column: &Column, theme: &Theme) -> Cell<'static> {
    let text = cell_text(coin, column);
    let line = Line::from(text)
        .style(cell_style(coin, column, theme))
        .alignment(column_alignment(column));
    Cell::new(line)
}

fn cell_text(coin: &CoinMarket, column: &Column) -> String {
    let raw = match column.kind {
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
    };
    align_cell(&raw, column.width, column.right)
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

    fn is_chart_row(line: &Line) -> bool {
        let text = line_text(line);
        text.contains('$') && text.contains(['█', '▀', '▄'])
    }

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
        assert!(
            cell_text(&up.coins()[0], trend)
                .chars()
                .any(|ch| SPARKLINE_GLYPHS.contains(&ch)),
            "glyphs remain as text"
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
    fn price_chart_is_left_aligned_bounded_and_gradient_styled() {
        let trend = Color::Rgb(200, 100, 50);
        let accent = Color::Cyan;
        let chart =
            price_chart_lines(&[1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0], 56, 10, trend, accent).unwrap();
        assert_eq!(
            chart.len(),
            CHART_ROWS as usize + 1,
            "CHART_ROWS body rows plus the caption line"
        );
        let body = &chart[..chart.len() - 1];
        assert!(
            body.iter()
                .all(|line| UnicodeWidthStr::width(line_text(line).as_str()) <= 56),
            "every row stays inside the content column"
        );
        assert!(
            body.iter().all(|line| is_chart_row(line)),
            "every chart row has a price label and a fixed-width glyph run"
        );
        assert!(
            body.iter()
                .any(|line| line.spans.iter().any(|span| span.style.fg == Some(trend))),
            "the line and its near fill use the full trend color"
        );
        assert!(
            body.iter()
                .any(|line| line.spans.iter().any(|span| span.style.fg == Some(accent))),
            "the price labels use the accent color"
        );
        assert!(
            line_text(chart.last().unwrap()).contains("7 days:"),
            "caption shows the range: {:?}",
            chart.last().unwrap()
        );

        let flat = price_chart_lines(&[5.0, 5.0, 5.0], 56, 10, trend, accent).unwrap();
        assert!(
            flat.iter()
                .any(|line| line.spans.iter().any(|span| span.style.fg == Some(trend))),
            "flat series renders a mid-line with the trend color"
        );
        assert_eq!(
            price_chart_lines(&[f64::NAN, f64::INFINITY], 56, 10, trend, accent),
            None,
            "all-non-finite series has nothing to plot"
        );
        assert_eq!(
            price_chart_lines(&[1.0, 2.0], 20, 10, trend, accent),
            None,
            "pane below the minimum width is skipped"
        );
    }

    #[test]
    fn gradient_ramp_fades_rgb_and_stays_solid_without_one() {
        let ramp = gradient_ramp(Color::Rgb(200, 100, 50), 5);
        assert_eq!(ramp.len(), 5);
        assert_eq!(ramp[0], Color::Rgb(200, 100, 50));
        assert!(
            ramp.windows(2).all(|stage| match (stage[0], stage[1]) {
                (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) => {
                    [r0 >= r1, g0 >= g1, b0 >= b1]
                        .into_iter()
                        .all(|darker| darker)
                }
                _ => false,
            }),
            "each stage darkens toward the endpoint"
        );
        assert_eq!(
            gradient_ramp(Color::Reset, 4),
            vec![Color::Reset; 4],
            "Monochrome stays solid and colorless"
        );
        assert_eq!(
            gradient_ramp(Color::Green, 3).len(),
            3,
            "named ANSI colors are converted to RGB so they can fade"
        );
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
        let Command::Fetch { generation } = app.update(Event::Input(KeyEvent::new(
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
        let Command::Fetch { generation } = app.update(Event::Input(KeyEvent::new(
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
        let Command::Fetch { generation } = app.update(Event::Input(KeyEvent::new(
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
        let Command::Fetch { generation } = app.update(Event::Input(KeyEvent::new(
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
        let selected_reversed = (0..width).any(|x| {
            buffer
                .cell((x, selected_y))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        });
        assert!(selected_reversed, "scrolled-to selected row is highlighted");
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
                .contains(Modifier::REVERSED)
        });
        assert!(highlighted, "selected row uses a reversed style");
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
        assert!(rendered.contains('█'), "chart area renders: {rendered:?}");
    }

    #[test]
    fn detail_pane_shows_placeholder_for_empty_series() {
        let app = detail_app(vec![detail_row("Bitcoin", "BTC", 1.0, 0.0, vec![])]);
        let rendered = text_at(&app, 60, 16);
        assert!(
            rendered.contains("No 7-day price data available."),
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
            rendered.contains('█'),
            "non-finite points are dropped: {rendered:?}"
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
            rendered.contains('█'),
            "flat series renders a mid-line: {rendered:?}"
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
            rendered.contains('█'),
            "long series is downsampled and renders: {rendered:?}"
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
        assert!(compact.contains('█'), "{compact:?}");
        assert!(
            compact.contains("Esc back | ? help | q quit | r refresh"),
            "{compact:?}"
        );
        let full = text_at(&app, 120, 30);
        assert!(full.contains("#1  Bitcoin (BTC)"), "{full:?}");
        assert!(full.contains('█'), "{full:?}");
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
}
