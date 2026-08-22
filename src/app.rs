use std::{
    cmp::Ordering,
    future::Future,
    io,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use crossterm::event::{Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind};
use futures_util::{Stream, StreamExt};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use crate::news::NoNewsProvider;
use crate::{
    api::{ApiError, CoinGeckoClient, FetchOutcome, MarketData},
    config::Config,
    domain::{CoinDetail, CoinMarket, MarketSnapshot, PricePoint},
    log::FileLog,
    news::{NewsItem, NewsProvider, RssNewsClient},
    sentiment::{AlternativeMeClient, FearGreedIndex, FearGreedProvider},
    theme::{Theme, THEMES},
    tui, ui,
};
use crate::{detail, input, pane, refresh};

pub enum Event {
    Start,
    Tick,
    Input(KeyEvent),
    Resize {
        height: u16,
    },
    FetchResult {
        generation: u64,
        result: Result<FetchOutcome, ApiError>,
    },
    DetailResult {
        id: String,
        generation: u64,
        result: Result<Box<CoinDetail>, ApiError>,
    },
    ChartResult {
        id: String,
        generation: u64,
        result: Result<Vec<PricePoint>, ApiError>,
    },
    NewsResult {
        generation: u64,
        result: Result<Vec<NewsItem>, ApiError>,
    },
    FearGreedResult {
        result: Result<FearGreedIndex, ApiError>,
    },
}

pub enum Command {
    Quit,
    Render,
    Fetch {
        generation: u64,
        news_generation: Option<u64>,
    },
    FetchDetail {
        id: String,
        generation: u64,
    },
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DataState {
    Initial,
    Loading,
    Ready {
        snapshot: MarketSnapshot,
        refreshed_at: Instant,
        notice: Option<ApiError>,
    },
    Empty {
        snapshot: MarketSnapshot,
        refreshed_at: Instant,
        notice: Option<ApiError>,
    },
    Stale {
        snapshot: MarketSnapshot,
        refreshed_at: Instant,
        error: ApiError,
        notice: Option<ApiError>,
    },
    Fatal(ApiError),
}

/// The coin detail screen. `Enter` always opens with the snapshot row
/// (`Basic`) so the pane renders instantly offline; an optional
/// `/coins/{id}` fetch upgrades it to `Ready`, and a failed fetch keeps it
/// on the row-derived fallback. `Ready` also carries the optional 30-day
/// price series for the candlestick chart when the market-chart fetch lands.
#[derive(Clone, Debug, PartialEq)]
pub enum DetailState {
    Basic(CoinMarket),
    Loading {
        base: CoinMarket,
        chart_30d: Vec<PricePoint>,
    },
    Ready {
        base: CoinMarket,
        detail: Box<CoinDetail>,
        chart_30d: Vec<PricePoint>,
    },
}

impl DetailState {
    pub fn base(&self) -> &CoinMarket {
        match self {
            DetailState::Basic(base)
            | DetailState::Loading { base, .. }
            | DetailState::Ready { base, .. } => base,
        }
    }

    fn update_base(&mut self, base: CoinMarket) {
        match self {
            DetailState::Basic(current)
            | DetailState::Loading { base: current, .. }
            | DetailState::Ready { base: current, .. } => *current = base,
        }
    }

    /// The optional 30-day price series for the candlestick chart, empty when
    /// the market-chart fetch has not landed or is unsupported.
    pub fn chart_30d(&self) -> &[PricePoint] {
        match self {
            DetailState::Loading { chart_30d, .. } | DetailState::Ready { chart_30d, .. } => {
                chart_30d
            }
            DetailState::Basic(_) => &[],
        }
    }
}

/// The newest headline feed plus the notice when the last fetch failed; the
/// previous items are preserved so the pane shows stale news instead of blank.
#[derive(Clone, Debug, PartialEq)]
pub struct NewsFeed {
    pub items: Vec<NewsItem>,
    pub notice: Option<ApiError>,
}

/// Which pane the main view shows. At compact widths only the focused pane is
/// visible (`Tab`/`Shift-Tab` cycle it); at wide widths the focused pane's
/// title is highlighted but the market table, news, and sentiment all render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainPane {
    Table,
    News,
    Sentiment,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartRange {
    Day,
    #[default]
    Week,
    Month,
}

impl ChartRange {
    pub fn label(self) -> &'static str {
        match self {
            Self::Day => "1 day",
            Self::Week => "7 days",
            Self::Month => "30 days",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Day => Self::Week,
            Self::Week => Self::Month,
            Self::Month => Self::Day,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Day => Self::Month,
            Self::Week => Self::Day,
            Self::Month => Self::Week,
        }
    }
}

pub struct App {
    state: DataState,
    generation: u64,
    fetching: bool,
    selected: usize,
    viewport_rows: usize,
    search: SearchState,
    sort: SortState,
    help_open: bool,
    detail: Option<DetailState>,
    detail_fetching: bool,
    chart_fetching: bool,
    detail_generation: u64,
    detail_id: Option<String>,
    detail_range: ChartRange,
    news: Option<NewsFeed>,
    fear_greed: Option<FearGreedIndex>,
    news_fetching: bool,
    news_generation: u64,
    news_enabled: bool,
    news_scroll: u16,
    pane_focus: MainPane,
    theme_index: usize,
    schedule: RefreshScheduler,
}

/// Editing and committed state for the `/` search feature. Typing fills
/// `buffer`; `Enter` commits it to `query` and `Esc` cancels without touching
/// the committed filter. An empty query means no filter.
#[derive(Clone, Debug, Default, PartialEq)]
struct SearchState {
    typing: bool,
    buffer: String,
    query: String,
}

/// Numeric columns that can be sorted. The default state (rank, ascending) is
/// the natural market-cap order, so it sorts nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Rank,
    Price,
    Change1h,
    Change24h,
    Change7d,
    Cap,
    Volume,
    Supply,
}

impl SortKey {
    /// Cycle order for `s`/`Shift-S`. Each key appears in both directions.
    const CYCLE: [SortKey; 8] = [
        SortKey::Rank,
        SortKey::Price,
        SortKey::Change1h,
        SortKey::Change24h,
        SortKey::Change7d,
        SortKey::Cap,
        SortKey::Volume,
        SortKey::Supply,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Rank => "rank",
            SortKey::Price => "price",
            SortKey::Change1h => "1h",
            SortKey::Change24h => "24h",
            SortKey::Change7d => "7d",
            SortKey::Cap => "cap",
            SortKey::Volume => "volume",
            SortKey::Supply => "supply",
        }
    }
}

/// The active sort key and direction. The default (rank ascending) is the
/// provider order and shows no sort indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortState {
    key: SortKey,
    ascending: bool,
}

impl Default for SortState {
    fn default() -> Self {
        Self {
            key: SortKey::Rank,
            ascending: true,
        }
    }
}

impl SortState {
    pub fn key(self) -> SortKey {
        self.key
    }
    pub fn ascending(self) -> bool {
        self.ascending
    }
    fn position(self) -> usize {
        let key_index = SortKey::CYCLE
            .iter()
            .position(|key| *key == self.key)
            .unwrap_or(0);
        key_index * 2 + usize::from(!self.ascending)
    }
    fn at(position: usize) -> Self {
        let key_index = position / 2;
        Self {
            key: SortKey::CYCLE[key_index],
            ascending: position.is_multiple_of(2),
        }
    }
    fn advance(self, forward: bool) -> Self {
        let total = SortKey::CYCLE.len() * 2;
        let next = if forward {
            self.position() + 1
        } else {
            self.position() + total - 1
        };
        Self::at(next % total)
    }
}

/// Cap the search buffer so a long typed query cannot overflow the layout.
const MAX_SEARCH_CHARS: usize = 64;

/// Capped jittered backoff base and ceiling for transient failures.
const BACKOFF_BASE_MS: u64 = 2_000;
const BACKOFF_MAX_MS: u64 = 60_000;
/// Cooldown applied to non-transient failures (4xx, malformed responses) so
/// automatic retries stay on a steady cadence without hammering the provider.
const RETRY_GATE: Duration = Duration::from_secs(60);

/// When the next refresh attempt is allowed. A success restarts the normal
/// cadence (`next_auto_at = now + interval`); a failure opens a cooldown window
/// during which neither automatic nor manual refresh may start.
struct RefreshScheduler {
    interval: Duration,
    failures: u32,
    next_auto_at: tokio::time::Instant,
    cooling_down: bool,
}

impl RefreshScheduler {
    fn new(now: tokio::time::Instant, interval: Duration) -> Self {
        Self {
            interval,
            failures: 0,
            next_auto_at: now + interval,
            cooling_down: false,
        }
    }
    /// A manual refresh may start unless a failure cooldown is still active.
    fn allow_manual(&self, now: tokio::time::Instant) -> bool {
        !self.cooling_down || now >= self.next_auto_at
    }
    /// An automatic refresh may start once the current window has elapsed.
    fn due(&self, now: tokio::time::Instant, fetching: bool) -> bool {
        !fetching && now >= self.next_auto_at
    }
    fn mark_success(&mut self, now: tokio::time::Instant) {
        self.failures = 0;
        self.cooling_down = false;
        self.next_auto_at = now + self.interval;
    }
    fn mark_failure(&mut self, now: tokio::time::Instant, delay: Duration) {
        self.failures = self.failures.saturating_add(1);
        self.cooling_down = true;
        self.next_auto_at = now + delay;
    }
    /// Remaining cooldown window, if one is still open.
    fn cooldown_remaining(&self, now: tokio::time::Instant) -> Option<Duration> {
        if self.cooling_down {
            let remaining = self.next_auto_at.saturating_duration_since(now);
            if !remaining.is_zero() {
                return Some(remaining);
            }
        }
        None
    }
}

/// How long to wait before the next attempt after a failed refresh. A `429`
/// honors the provider's `Retry-After` (floored at one second); transient
/// failures (transport, timeout, `5xx`, or a bare `429`) use capped jittered
/// backoff; other errors retry on the steady cadence.
fn failure_retry_delay(error: &ApiError, failures: u32) -> Duration {
    match error {
        ApiError::RateLimited {
            retry_after: Some(delay),
        } => (*delay).max(Duration::from_secs(1)),
        ApiError::Timeout | ApiError::Transport => jittered_backoff(failures),
        ApiError::HttpStatus { status } if (500..600).contains(status) => {
            jittered_backoff(failures)
        }
        ApiError::RateLimited { retry_after: None } => jittered_backoff(failures),
        _ => RETRY_GATE,
    }
}

/// Capped exponential backoff with equal jitter: the delay lands in
/// `[scaled / 2, scaled]` where `scaled` grows by a factor of two per failure
/// up to `BACKOFF_MAX_MS`.
fn jittered_backoff(failures: u32) -> Duration {
    let scaled = BACKOFF_BASE_MS
        .saturating_mul(1_u64 << failures.min(60))
        .min(BACKOFF_MAX_MS);
    let lower = scaled / 2;
    let range = (scaled - lower).max(1);
    Duration::from_millis(lower + pseudo_random(u64::from(failures)) % range)
}

/// Cheap deterministic-mix jitter seed. Time only varies the shift; the bounds
/// come from the deterministic backoff window.
fn pseudo_random(seed: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    let mut value = now ^ seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}

impl App {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_refresh_interval(Duration::from_secs(crate::config::DEFAULT_REFRESH_SECONDS))
    }

    /// An `App` whose automatic refresh cadence comes from validated config.
    /// The news feed stays disabled; the production controller opts in.
    #[cfg(test)]
    pub fn with_refresh_interval(interval: Duration) -> Self {
        Self::with_news_flag(interval, false)
    }

    /// App with the news feed enabled.
    #[cfg(test)]
    pub fn with_news_enabled(interval: Duration) -> Self {
        Self::with_news_flag(interval, true)
    }

    fn with_news_flag(interval: Duration, news_enabled: bool) -> Self {
        Self {
            state: DataState::Initial,
            generation: 0,
            fetching: false,
            selected: 0,
            viewport_rows: 16,
            search: SearchState::default(),
            sort: SortState::default(),
            help_open: false,
            detail: None,
            detail_fetching: false,
            chart_fetching: false,
            detail_generation: 0,
            detail_id: None,
            detail_range: ChartRange::default(),
            news: None,
            fear_greed: None,
            news_fetching: false,
            news_generation: 0,
            news_enabled,
            news_scroll: 0,
            pane_focus: MainPane::Table,
            theme_index: 0,
            schedule: RefreshScheduler::new(tokio::time::Instant::now(), interval),
        }
    }

    /// The only state transition function. It performs no I/O and is deterministic.
    pub fn update(&mut self, event: Event) -> Command {
        match event {
            Event::Start => self.request_refresh(),
            Event::Tick => {
                let now = tokio::time::Instant::now();
                if self.schedule.due(now, self.fetching) {
                    self.begin_fetch()
                } else {
                    Command::Render
                }
            }
            Event::Input(key) if input::should_quit(key, self.search.typing) => Command::Quit,
            Event::Input(key) if self.help_open => {
                if input::is_help_toggle(key) || input::is_esc(key) {
                    self.help_open = false;
                    Command::Render
                } else {
                    Command::None
                }
            }
            Event::Input(key) if self.detail.is_some() => {
                if input::is_esc(key) {
                    self.detail = None;
                    self.cancel_detail_fetch();
                    Command::Render
                } else if input::is_help_toggle(key) {
                    self.help_open = true;
                    Command::Render
                } else if input::is_refresh(key) {
                    self.request_refresh()
                } else if input::is_theme_forward(key) || input::is_theme_backward(key) {
                    self.cycle_theme(input::is_theme_forward(key));
                    Command::Render
                } else if input::is_chart_range_next(key) {
                    self.detail_range = self.detail_range.next();
                    Command::Render
                } else if input::is_chart_range_previous(key) {
                    self.detail_range = self.detail_range.previous();
                    Command::Render
                } else {
                    Command::None
                }
            }
            Event::Input(key) if self.search.typing => {
                if self.search_input(key) {
                    Command::Render
                } else {
                    Command::None
                }
            }
            Event::Input(key)
                if input::clear_active_search(key) && !self.search.query.is_empty() =>
            {
                self.search.query.clear();
                Command::Render
            }
            Event::Input(key) if input::is_search_start(key) => {
                self.search.typing = true;
                self.search.buffer.clear();
                Command::Render
            }
            Event::Input(key) if input::is_help_toggle(key) => {
                self.help_open = true;
                Command::Render
            }
            Event::Input(key) if input::is_sort_forward(key) => {
                self.cycle_sort(true);
                Command::Render
            }
            Event::Input(key) if input::is_sort_backward(key) => {
                self.cycle_sort(false);
                Command::Render
            }
            Event::Input(key) if input::is_refresh(key) => self.request_refresh(),
            Event::Input(key) if input::is_theme_forward(key) || input::is_theme_backward(key) => {
                self.cycle_theme(input::is_theme_forward(key));
                Command::Render
            }
            Event::Input(key) if input::is_pane_forward(key) || input::is_pane_backward(key) => {
                self.cycle_pane(input::is_pane_forward(key));
                Command::Render
            }
            Event::Input(key)
                if self.pane_focus == MainPane::News && input::is_news_scroll(key) =>
            {
                self.news_scroll =
                    input::news_scroll_target(key.code, self.news_scroll, self.viewport_rows);
                Command::Render
            }
            Event::Input(key) if input::is_detail_open(key) && self.row_count() > 0 => {
                if let Some(coin) = self
                    .visible_coins()
                    .get(self.selected)
                    .map(|coin| (*coin).clone())
                {
                    self.detail = Some(DetailState::Loading {
                        base: coin.clone(),
                        chart_30d: Vec::new(),
                    });
                    self.detail_range = ChartRange::default();
                    self.begin_detail_fetch(coin.id())
                } else {
                    Command::None
                }
            }
            Event::Input(key) if input::navigation_key(key.code) => {
                self.navigate(key.code);
                Command::Render
            }
            Event::Resize { height, .. } => {
                self.viewport_rows = input::table_viewport(height);
                Command::Render
            }
            Event::FetchResult { generation, result } => {
                if !self.fetching || generation != self.generation {
                    return Command::None;
                }
                self.fetching = false;
                let refreshed_at = Instant::now();
                let scheduler_now = tokio::time::Instant::now();
                match result {
                    Ok(outcome) => {
                        self.schedule.mark_success(scheduler_now);
                        if let Some(state) = &mut self.detail {
                            if let Some(updated) = outcome
                                .snapshot
                                .coins()
                                .iter()
                                .find(|coin| coin.id() == state.base().id())
                            {
                                state.update_base(updated.clone());
                            }
                        }
                        if outcome.snapshot.coins().is_empty() {
                            self.state = DataState::Empty {
                                snapshot: outcome.snapshot,
                                refreshed_at,
                                notice: outcome.summary_notice,
                            };
                        } else {
                            self.state = DataState::Ready {
                                snapshot: outcome.snapshot,
                                refreshed_at,
                                notice: outcome.summary_notice,
                            };
                        }
                    }
                    Err(error) => {
                        let delay = failure_retry_delay(&error, self.schedule.failures);
                        self.schedule.mark_failure(scheduler_now, delay);
                        self.state = match self.state.clone() {
                            DataState::Ready {
                                snapshot,
                                refreshed_at,
                                notice,
                            }
                            | DataState::Empty {
                                snapshot,
                                refreshed_at,
                                notice,
                            }
                            | DataState::Stale {
                                snapshot,
                                refreshed_at,
                                notice,
                                ..
                            } => DataState::Stale {
                                snapshot,
                                refreshed_at,
                                error,
                                notice,
                            },
                            DataState::Initial | DataState::Loading | DataState::Fatal(_) => {
                                DataState::Fatal(error)
                            }
                        };
                    }
                }
                self.clamp_selection();
                Command::Render
            }
            Event::DetailResult {
                id,
                generation,
                result,
            } => {
                if !self.detail_fetching || generation != self.detail_generation {
                    return Command::None;
                }
                self.detail_fetching = false;
                match result {
                    Ok(detail) => {
                        let upgrade = matches!(
                            &self.detail,
                            Some(DetailState::Loading { base, .. }) if base.id() == id
                        );
                        if upgrade {
                            if let Some(DetailState::Loading { base, chart_30d }) =
                                self.detail.take()
                            {
                                self.detail = Some(DetailState::Ready {
                                    base,
                                    detail,
                                    chart_30d,
                                });
                            }
                        }
                    }
                    Err(_) => {
                        if let Some(DetailState::Loading { base, .. }) = self.detail.take() {
                            self.detail = Some(DetailState::Basic(base));
                        }
                    }
                }
                Command::Render
            }
            Event::ChartResult {
                id,
                generation,
                result,
            } => {
                if !self.chart_fetching || generation != self.detail_generation {
                    return Command::None;
                }
                self.chart_fetching = false;
                self.detail_id = None;
                if let Ok(series) = result {
                    match self.detail.take() {
                        Some(DetailState::Ready { base, detail, .. }) if base.id() == id => {
                            self.detail = Some(DetailState::Ready {
                                base,
                                detail,
                                chart_30d: series,
                            });
                        }
                        Some(DetailState::Loading { base, .. }) if base.id() == id => {
                            self.detail = Some(DetailState::Loading {
                                base,
                                chart_30d: series,
                            });
                        }
                        other => self.detail = other,
                    }
                }
                Command::Render
            }
            Event::NewsResult { generation, result } => {
                if !self.news_fetching || generation != self.news_generation {
                    return Command::None;
                }
                self.news_fetching = false;
                self.news = Some(match result {
                    Ok(items) => NewsFeed {
                        items,
                        notice: None,
                    },
                    Err(error) => {
                        let items = self.news.take().map(|feed| feed.items).unwrap_or_default();
                        NewsFeed {
                            items,
                            notice: Some(error),
                        }
                    }
                });
                Command::Render
            }
            Event::FearGreedResult { result } => {
                if let Ok(index) = result {
                    self.fear_greed = Some(index);
                }
                Command::Render
            }
            Event::Input(_) => Command::None,
        }
    }

    fn navigate(&mut self, code: KeyCode) {
        self.selected =
            input::navigation_target(code, self.selected, self.row_count(), self.viewport_rows);
    }

    /// Manual refresh. It starts a fetch unless one is active or a failure
    /// cooldown is still open (manual refresh never bypasses a cooldown).
    fn request_refresh(&mut self) -> Command {
        if self.fetching {
            return Command::None;
        }
        if !self.schedule.allow_manual(tokio::time::Instant::now()) {
            return Command::None;
        }
        self.begin_fetch()
    }

    fn begin_fetch(&mut self) -> Command {
        self.generation = refresh::next_generation(self.generation);
        self.fetching = true;
        if matches!(self.state, DataState::Initial) {
            self.state = DataState::Loading;
        }
        let news_generation = self.begin_news_fetch();
        Command::Fetch {
            generation: self.generation,
            news_generation,
        }
    }

    /// Start a detail fetch for the coin id, unless one is already in flight.
    fn begin_detail_fetch(&mut self, id: &str) -> Command {
        if self.detail_fetching {
            return Command::None;
        }
        self.detail_fetching = true;
        self.chart_fetching = true;
        self.detail_id = Some(id.to_owned());
        self.detail_generation = detail::next_generation(self.detail_generation);
        Command::FetchDetail {
            id: id.to_owned(),
            generation: self.detail_generation,
        }
    }

    /// Invalidate any in-flight detail fetch (used when the detail closes).
    fn cancel_detail_fetch(&mut self) {
        self.detail_fetching = false;
        self.chart_fetching = false;
        self.detail_id = None;
        self.detail_generation = detail::next_generation(self.detail_generation);
    }

    /// Chain a news fetch onto a market refresh, one in flight at a time.
    fn begin_news_fetch(&mut self) -> Option<u64> {
        if !self.news_enabled || self.news_fetching {
            return None;
        }
        self.news_fetching = true;
        self.news_generation = refresh::next_generation(self.news_generation);
        Some(self.news_generation)
    }

    /// Cycle which pane is focused when the terminal is too narrow for
    /// side-by-side panes; forward `Tab`, backward `Shift-Tab`.
    fn cycle_pane(&mut self, forward: bool) {
        const PANES: [MainPane; 3] = [MainPane::Table, MainPane::News, MainPane::Sentiment];
        let current = PANES
            .iter()
            .position(|pane| *pane == self.pane_focus)
            .unwrap_or(0);
        self.pane_focus = PANES[pane::next_index(current, PANES.len(), forward)];
        if self.pane_focus != MainPane::News {
            self.news_scroll = 0;
        }
    }

    pub fn state_ref(&self) -> &DataState {
        &self.state
    }
    #[cfg(test)]
    pub fn state(&self) -> DataState {
        self.state.clone()
    }
    pub fn fetching(&self) -> bool {
        self.fetching
    }
    pub fn news_scroll(&self) -> u16 {
        self.news_scroll
    }
    pub fn detail_range(&self) -> ChartRange {
        self.detail_range
    }
    pub fn fear_greed(&self) -> Option<&FearGreedIndex> {
        self.fear_greed.as_ref()
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    /// Bound an index to the current row count.
    #[allow(dead_code)]
    pub fn select(&mut self, index: usize) {
        self.selected = clamped_index(index, self.row_count());
    }
    #[cfg(test)]
    pub fn set_sort(&mut self, key: SortKey, ascending: bool) {
        self.sort = SortState { key, ascending };
        self.clamp_selection();
    }
    fn clamp_selection(&mut self) {
        self.selected = clamped_index(self.selected, self.row_count());
    }
    fn row_count(&self) -> usize {
        self.visible_coins().len()
    }
    /// Coins visible under the committed filter and sort, in a deterministic
    /// order. Untrusted remote names and symbols are compared
    /// case-insensitively; missing sort values always land last.
    pub fn visible_coins(&self) -> Vec<&CoinMarket> {
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        let mut coins: Vec<&CoinMarket> = if self.search.query.is_empty() {
            snapshot.coins().iter().collect()
        } else {
            let needle = self.search.query.to_lowercase();
            snapshot
                .coins()
                .iter()
                .filter(|coin| {
                    coin.name().to_lowercase().contains(&needle)
                        || coin.symbol().to_lowercase().contains(&needle)
                })
                .collect()
        };
        if self.sort_active() {
            let key = self.sort.key;
            let ascending = self.sort.ascending;
            coins.sort_by(|a, b| compare_coins(a, b, key, ascending));
        }
        coins
    }
    pub fn searching(&self) -> bool {
        self.search.typing
    }
    pub fn search_buffer(&self) -> &str {
        &self.search.buffer
    }
    pub fn search_query(&self) -> &str {
        &self.search.query
    }
    pub fn has_active_filter(&self) -> bool {
        !self.search.query.is_empty()
    }
    pub fn sort_state(&self) -> SortState {
        self.sort
    }
    /// True when the active sort differs from the natural rank order.
    pub fn sort_active(&self) -> bool {
        self.sort != SortState::default()
    }
    pub fn help_open(&self) -> bool {
        self.help_open
    }
    /// The row-backed coin shown on the detail screen, if one is open.
    /// `Enter` opens it for the selected row and `Esc` closes it without
    /// touching selection. While it is read-only UI: `r`, `?`, `q`, and `Esc`
    /// stay active, and a completed refresh re-syncs the stored coin by ID.
    #[cfg(test)]
    pub fn detail(&self) -> Option<&CoinMarket> {
        self.detail.as_ref().map(DetailState::base)
    }
    /// One of `Basic` (row fallback), `Loading`, or `Ready` (rich detail).
    pub fn detail_state(&self) -> Option<&DetailState> {
        self.detail.as_ref()
    }
    /// True while a `/coins/{id}` detail request is in flight. The UI uses it
    /// to label the sidebar while it upgrades from the row fallback.
    pub fn detail_fetching(&self) -> bool {
        self.detail_fetching
    }
    pub fn detail_open(&self) -> bool {
        self.detail.is_some()
    }
    /// The newest headline feed, or `None` before the first news result.
    pub fn news_feed(&self) -> Option<&NewsFeed> {
        self.news.as_ref()
    }
    pub fn news_enabled(&self) -> bool {
        self.news_enabled
    }
    pub fn pane_focus(&self) -> MainPane {
        self.pane_focus
    }
    /// The active color theme, starting on `THEMES[0]` and cycled at runtime.
    pub fn theme(&self) -> &'static Theme {
        &THEMES[self.theme_index]
    }
    /// Advance or retreat one built-in theme; wraps at both ends.
    pub fn cycle_theme(&mut self, forward: bool) {
        let len = THEMES.len();
        let next = if forward {
            (self.theme_index + 1) % len
        } else {
            (self.theme_index + len - 1) % len
        };
        self.theme_index = next;
    }
    /// Remaining failure cooldown window, if one is open. UI uses it to
    /// replace the "press r to retry" call-to-action with a countdown.
    pub fn refresh_cooldown(&self) -> Option<Duration> {
        self.schedule
            .cooldown_remaining(tokio::time::Instant::now())
    }
    /// Move the sort key (and its direction) one step forward or backward in
    /// the cycle and keep the selection on the same coin id.
    fn cycle_sort(&mut self, forward: bool) {
        let anchor = self
            .visible_coins()
            .get(self.selected)
            .map(|coin| coin.id().to_owned());
        self.sort = self.sort.advance(forward);
        self.reanchor_selection(anchor);
    }
    /// Handle a key while search editing is open. Enter commits the buffer,
    /// Esc discards it, printable characters append (bounded), and Backspace
    /// removes the last scalar.
    fn search_input(&mut self, key: KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return false;
        }
        match key.code {
            KeyCode::Enter => {
                self.search.typing = false;
                let anchor = self
                    .visible_coins()
                    .get(self.selected)
                    .map(|coin| coin.id().to_owned());
                self.search.query = std::mem::take(&mut self.search.buffer);
                self.reanchor_selection(anchor);
                true
            }
            KeyCode::Esc => {
                self.search.typing = false;
                self.search.buffer.clear();
                true
            }
            KeyCode::Backspace => self.search.buffer.pop().is_some(),
            KeyCode::Char(character)
                if !character.is_control()
                    && self.search.buffer.chars().count() < MAX_SEARCH_CHARS =>
            {
                self.search.buffer.push(character);
                true
            }
            _ => false,
        }
    }
    /// Keep the selection on the same coin id captured before the visible set
    /// changed; when it no longer matches, clamp to the visible row count.
    fn reanchor_selection(&mut self, anchor: Option<String>) {
        self.selected = match anchor.as_deref() {
            Some(id) => self
                .visible_coins()
                .iter()
                .position(|coin| coin.id() == id)
                .unwrap_or(self.selected),
            None => self.selected,
        };
        self.clamp_selection();
    }
    pub fn snapshot(&self) -> Option<&MarketSnapshot> {
        match &self.state {
            DataState::Ready { snapshot, .. }
            | DataState::Empty { snapshot, .. }
            | DataState::Stale { snapshot, .. } => Some(snapshot),
            DataState::Initial | DataState::Loading | DataState::Fatal(_) => None,
        }
    }
}

fn clamped_index(index: usize, count: usize) -> usize {
    match count {
        0 => 0,
        _ => index.min(count - 1),
    }
}

pub struct Controller<P: MarketData + ?Sized> {
    provider: Arc<P>,
    news: Arc<dyn NewsProvider>,
    fear_greed: Option<Arc<dyn FearGreedProvider>>,
    events: mpsc::Sender<Event>,
    results: mpsc::Receiver<Event>,
    cancellation: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    refresh_started_at: Option<Instant>,
    tracer: Option<FileLog>,
    pub app: App,
}

impl<P: MarketData + ?Sized + 'static> Controller<P> {
    #[cfg(test)]
    pub fn new(provider: Arc<P>) -> Self {
        Self::with_tracer(provider, None)
    }

    #[cfg(test)]
    pub fn with_tracer(provider: Arc<P>, tracer: Option<FileLog>) -> Self {
        Self::with_interval(
            provider,
            tracer,
            Duration::from_secs(crate::config::DEFAULT_REFRESH_SECONDS),
        )
    }

    #[cfg(test)]
    pub fn with_interval(provider: Arc<P>, tracer: Option<FileLog>, interval: Duration) -> Self {
        Self::with_news_and_flag(
            provider,
            Arc::new(NoNewsProvider),
            tracer,
            interval,
            false,
            None,
        )
    }

    /// Controller with a real news feed; the app enables news fetches.
    pub fn with_news(
        provider: Arc<P>,
        news: Arc<dyn NewsProvider>,
        tracer: Option<FileLog>,
        interval: Duration,
    ) -> Self {
        Self::with_news_and_flag(provider, news, tracer, interval, true, None)
    }

    pub fn with_news_and_sentiment(
        provider: Arc<P>,
        news: Arc<dyn NewsProvider>,
        fear_greed: Arc<dyn FearGreedProvider>,
        tracer: Option<FileLog>,
        interval: Duration,
    ) -> Self {
        Self::with_news_and_flag(provider, news, tracer, interval, true, Some(fear_greed))
    }

    fn with_news_and_flag(
        provider: Arc<P>,
        news: Arc<dyn NewsProvider>,
        tracer: Option<FileLog>,
        interval: Duration,
        news_enabled: bool,
        fear_greed: Option<Arc<dyn FearGreedProvider>>,
    ) -> Self {
        let (events, results) = mpsc::channel(16);
        Self {
            provider,
            news,
            fear_greed,
            events,
            results,
            cancellation: CancellationToken::new(),
            tasks: Vec::new(),
            refresh_started_at: None,
            tracer,
            app: App::with_news_flag(interval, news_enabled),
        }
    }

    pub fn start_initial_refresh(&mut self) {
        self.dispatch(Event::Start);
    }

    fn dispatch(&mut self, event: Event) {
        let command = self.app.update(event);
        self.route(command);
    }

    fn trace(&self, message: String) {
        if let Some(log) = &self.tracer {
            log.info(&message);
        }
    }

    fn route(&mut self, command: Command) {
        match command {
            Command::Fetch {
                generation,
                news_generation,
            } => self.start_fetch(generation, news_generation),
            Command::FetchDetail { id, generation } => self.start_detail_fetch(id, generation),
            _ => {}
        }
    }

    fn spawn(&mut self, task: JoinHandle<()>) {
        self.tasks.push(task);
    }

    fn spawn_operation<F, T, M>(&mut self, operation: F, map_event: M)
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
        M: FnOnce(T) -> Event + Send + 'static,
    {
        let sender = self.events.clone();
        let cancelled = self.cancellation.clone();
        self.spawn(tokio::spawn(async move {
            tokio::select! {
                result = operation => { let _ = sender.send(map_event(result)).await; }
                _ = cancelled.cancelled() => {}
            }
        }));
    }

    fn start_fetch(&mut self, generation: u64, news_generation: Option<u64>) {
        self.refresh_started_at = Some(Instant::now());
        self.trace(format!("refresh start generation={generation}"));
        let provider = Arc::clone(&self.provider);
        self.spawn_operation(
            async move { provider.fetch_snapshot().await },
            move |result| Event::FetchResult { generation, result },
        );
        if let Some(news_generation) = news_generation {
            let news = Arc::clone(&self.news);
            self.spawn_operation(async move { news.fetch_headlines().await }, move |result| {
                Event::NewsResult {
                    generation: news_generation,
                    result,
                }
            });
        }
        if let Some(provider) = self.fear_greed.as_ref().map(Arc::clone) {
            self.spawn_operation(async move { provider.fetch().await }, |result| {
                Event::FearGreedResult { result }
            });
        }
    }

    fn start_detail_fetch(&mut self, id: String, generation: u64) {
        self.trace(format!(
            "detail fetch start id={id} generation={generation}"
        ));
        let provider = Arc::clone(&self.provider);
        let chart_id = id.clone();
        let detail_id = id.clone();
        self.spawn_operation(
            async move { provider.fetch_coin_detail(&id).await },
            move |result| Event::DetailResult {
                id: detail_id,
                generation,
                result: result.map(Box::new),
            },
        );
        // The 30-day price series for the candlestick chart is a separate
        // provider call, chained onto the same detail fetch so the chart
        // stretches once it lands (and falls back to the 7-day series when it
        // is unsupported or fails).
        let provider = Arc::clone(&self.provider);
        let chart_event_id = chart_id.clone();
        self.spawn_operation(
            async move { provider.fetch_market_chart(&chart_id).await },
            move |result| Event::ChartResult {
                id: chart_event_id,
                generation,
                result,
            },
        );
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        self.results.recv().await
    }

    pub async fn handle(&mut self, event: Event) -> Command {
        let fetch_result = match &event {
            Event::FetchResult { generation, result } => Some((*generation, result.clone())),
            _ => None,
        };
        let command = self.app.update(event);
        if let Some((generation, result)) = fetch_result {
            if !matches!(command, Command::None) {
                let elapsed_ms = self
                    .refresh_started_at
                    .take()
                    .map(|started| started.elapsed().as_millis())
                    .unwrap_or(0);
                let line = match result {
                    Ok(outcome) => format!(
                        "refresh ok generation={generation} coins={} duration={elapsed_ms}ms",
                        outcome.snapshot.coins().len()
                    ),
                    Err(error) => format!(
                        "refresh failed generation={generation} duration={elapsed_ms}ms error={error}"
                    ),
                };
                self.trace(line);
            }
        }
        match &command {
            Command::Fetch {
                generation,
                news_generation,
            } => self.start_fetch(*generation, *news_generation),
            Command::FetchDetail { id, generation } => {
                self.start_detail_fetch(id.clone(), *generation)
            }
            _ => {}
        }
        command
    }

    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.cancellation.cancel();
        for task in self.tasks.drain(..) {
            task.await.map_err(|error| {
                io::Error::other(format!("background task failed during shutdown: {error}"))
            })?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn has_active_task(&self) -> bool {
        !self.tasks.is_empty()
    }
}

impl<P: MarketData + ?Sized> Drop for Controller<P> {
    fn drop(&mut self) {
        self.cancellation.cancel();
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

pub async fn run(config: Config) -> io::Result<()> {
    let api_key = config.api_key.clone().filter(|key| !key.is_empty());
    debug_assert_eq!(config.currency, "usd");
    let provider = CoinGeckoClient::new(&config.base_url, api_key.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let news = RssNewsClient::new(&config.news_url)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let fear_greed = AlternativeMeClient::new()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let tracer = open_log(&config, api_key)?;
    let mut session = tui::enter()?;
    let result = run_loop(
        session.terminal_mut(),
        Arc::new(provider),
        Arc::new(news),
        Arc::new(fear_greed),
        tracer,
        config.refresh_seconds,
    )
    .await;
    let restore_result = session.restore();
    result.and(restore_result)
}

/// Diagnostics logger from validated config. The API key is registered as a
/// redaction secret so no accidental caption can spill it. Opening the file
/// happens before terminal entry, so a bad path fails early and cleanly.
fn open_log(config: &Config, api_key: Option<String>) -> io::Result<Option<FileLog>> {
    let Some(path) = config.log_file.as_deref() else {
        return Ok(None);
    };
    let secrets = api_key.map(|key| vec![key]).unwrap_or_default();
    FileLog::append_at(Path::new(path), secrets)
        .map(Some)
        .map_err(|error| io::Error::other(format!("cannot open log file {path}: {error}")))
}

async fn run_loop<P: MarketData + 'static>(
    terminal: &mut tui::AppTerminal,
    provider: Arc<P>,
    news: Arc<dyn NewsProvider>,
    fear_greed: Arc<dyn FearGreedProvider>,
    tracer: Option<FileLog>,
    refresh_seconds: u64,
) -> io::Result<()> {
    let mut draw = |app: &App| terminal.draw(|frame| ui::render(frame, app)).map(|_| ());
    run_loop_with_sources_and_tracer(
        provider,
        news,
        Some(fear_greed),
        EventStream::new(),
        &mut draw,
        tracer,
        Duration::from_secs(refresh_seconds),
    )
    .await
}

/// The controller loop is kept independent of the terminal so lifecycle and
/// concurrency behavior can be tested with deterministic event streams.
#[cfg(test)]
async fn run_loop_with_sources<P, S, R>(provider: Arc<P>, input: S, render: R) -> io::Result<()>
where
    P: MarketData + 'static,
    S: Stream<Item = Result<CrosstermEvent, io::Error>> + Unpin,
    R: FnMut(&App) -> io::Result<()>,
{
    run_loop_with_sources_and_tracer(
        provider,
        Arc::new(NoNewsProvider),
        None,
        input,
        render,
        None,
        Duration::from_secs(crate::config::DEFAULT_REFRESH_SECONDS),
    )
    .await
}

async fn run_loop_with_sources_and_tracer<P, S, R>(
    provider: Arc<P>,
    news: Arc<dyn NewsProvider>,
    fear_greed: Option<Arc<dyn FearGreedProvider>>,
    mut input: S,
    mut render: R,
    tracer: Option<FileLog>,
    refresh_interval: Duration,
) -> io::Result<()>
where
    P: MarketData + 'static,
    S: Stream<Item = Result<CrosstermEvent, io::Error>> + Unpin,
    R: FnMut(&App) -> io::Result<()>,
{
    let mut controller = match fear_greed {
        Some(fear_greed) => Controller::with_news_and_sentiment(
            provider,
            news,
            fear_greed,
            tracer,
            refresh_interval,
        ),
        None => Controller::with_news(provider, news, tracer, refresh_interval),
    };
    let session_log = controller.tracer.clone();
    let render_tracer = session_log.clone();
    let loop_result = async {
        controller.start_initial_refresh();
        if let Some(log) = &session_log {
            log.trace("info", "session start");
        }
        let mut draw = move |app: &App| {
            let started = std::time::Instant::now();
            let result = render(app);
            if let Some(log) = &render_tracer {
                if result.is_ok() {
                    log.info(&format!(
                        "render ok duration={}ms",
                        started.elapsed().as_millis()
                    ));
                }
            }
            result
        };
        draw(&controller.app)?;
        // Low-frequency tick: refreshes relative timestamps, re-renders, and
        // lets the refresh scheduler start automatic refreshes. The first
        // tick is delayed so startup is not double-rendered.
        let mut ticker = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(1),
            Duration::from_secs(1),
        );
        loop {
            let event = tokio::select! {
                event = input.next() => match event {
                    Some(Ok(CrosstermEvent::Key(key))) => Event::Input(key),
                    Some(Ok(CrosstermEvent::Resize(_, height))) => Event::Resize { height },
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => return Err(error),
                    None => return Ok(()),
                },
                event = controller.next_event() => match event {
                    Some(event) => event,
                    None => return Ok(()),
                },
                _ = ticker.tick() => Event::Tick,
            };
            match controller.handle(event).await {
                Command::Quit => {
                    return Ok(());
                }
                Command::Render => {
                    draw(&controller.app)?;
                }
                Command::Fetch { .. } => {
                    draw(&controller.app)?;
                }
                Command::FetchDetail { .. } => {
                    draw(&controller.app)?;
                }
                Command::None => {}
            }
        }
    };
    let loop_result = loop_result.await;
    if let Some(log) = &session_log {
        log.trace(
            "info",
            &format!("loop stopped success={}", loop_result.is_ok()),
        );
    }
    let shutdown_result = controller.shutdown().await;
    match loop_result {
        Err(error) => Err(error),
        Ok(()) => shutdown_result,
    }
}

/// Compare two coins by a numeric column. Missing values sort last in both
/// directions; equal finite values keep the snapshot order (stable tie).
fn compare_coins(a: &CoinMarket, b: &CoinMarket, key: SortKey, ascending: bool) -> Ordering {
    let (left, right): (Option<f64>, Option<f64>) = match key {
        SortKey::Rank => (a.rank().map(f64::from), b.rank().map(f64::from)),
        SortKey::Price => (a.price(), b.price()),
        SortKey::Change1h => (a.change_1h(), b.change_1h()),
        SortKey::Change24h => (a.change_24h(), b.change_24h()),
        SortKey::Change7d => (a.change_7d(), b.change_7d()),
        SortKey::Cap => (a.market_cap(), b.market_cap()),
        SortKey::Volume => (a.volume_24h(), b.volume_24h()),
        SortKey::Supply => (a.circulating_supply(), b.circulating_supply()),
    };
    compare_missing_last(left, right, ascending)
}

fn compare_missing_last(left: Option<f64>, right: Option<f64>, ascending: bool) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => {
            let ordering = left.partial_cmp(&right).unwrap_or(Ordering::Equal);
            if ascending {
                ordering
            } else {
                ordering.reverse()
            }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoinMarketInput, MarketSummaryInput};
    use crate::input;
    use crossterm::event::KeyModifiers;
    use futures_util::stream;
    use ratatui::{backend::TestBackend, Terminal};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Notify;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn snapshot() -> MarketSnapshot {
        MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![CoinMarketInput {
                id: "x".into(),
                rank: None,
                name: "X".into(),
                symbol: "x".into(),
                price: None,
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

    fn outcome(snapshot: MarketSnapshot) -> FetchOutcome {
        FetchOutcome {
            snapshot,
            summary_notice: None,
        }
    }

    struct PendingProvider {
        calls: AtomicUsize,
        dropped: Arc<std::sync::atomic::AtomicBool>,
        first_polled: Arc<Notify>,
    }

    struct PendingFetch {
        dropped: Arc<std::sync::atomic::AtomicBool>,
        first_polled: Arc<Notify>,
        polled: bool,
    }

    impl Future for PendingFetch {
        type Output = Result<FetchOutcome, ApiError>;

        fn poll(
            mut self: Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            if !self.polled {
                self.as_mut().get_mut().polled = true;
                self.first_polled.notify_one();
            }
            std::task::Poll::Pending
        }
    }

    impl Drop for PendingFetch {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }

    impl MarketData for PendingProvider {
        fn fetch_snapshot<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, ApiError>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(PendingFetch {
                dropped: Arc::clone(&self.dropped),
                first_polled: Arc::clone(&self.first_polled),
                polled: false,
            })
        }
    }

    /// A provider that counts calls and completes each fetch with an empty
    /// snapshot, used to observe automatic refreshes through the run loop.
    struct CountingProvider {
        calls: Arc<AtomicUsize>,
    }

    impl MarketData for CountingProvider {
        fn fetch_snapshot<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, ApiError>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Ok(outcome(snapshot())) })
        }
    }

    /// A provider whose refresh always fails with a transport error.
    struct FailingProvider;

    impl MarketData for FailingProvider {
        fn fetch_snapshot<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, ApiError>> + Send + 'a>> {
            Box::pin(async { Err(ApiError::Transport) })
        }
    }

    fn render_text(app: &App) -> String {
        render_text_at(app, 80, 24)
    }

    fn render_text_at(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// An input stream that sleeps once, then signals end-of-input so a
    /// running loop reaches a controlled, success=true shutdown.
    fn eof_after(
        delay: Duration,
    ) -> Pin<Box<dyn Stream<Item = Result<CrosstermEvent, io::Error>> + Send>> {
        Box::pin(stream::unfold(delay, |delay| async move {
            tokio::time::sleep(delay).await;
            None::<(Result<CrosstermEvent, io::Error>, Duration)>
        }))
    }

    #[test]
    fn pure_update_suppresses_duplicate_refresh_and_stale_results() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        assert!(matches!(app.update(Event::Input(key('r'))), Command::None));
        assert!(matches!(
            app.update(Event::Resize { height: 24 }),
            Command::Render
        ));
        assert!(matches!(
            app.update(Event::FetchResult {
                generation: generation + 1,
                result: Ok(outcome(snapshot()))
            }),
            Command::None
        ));
        assert!(matches!(
            app.update(Event::FetchResult {
                generation,
                result: Ok(outcome(snapshot()))
            }),
            Command::Render
        ));
        assert!(matches!(app.state(), DataState::Ready { .. }));
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        assert!(matches!(
            app.update(Event::FetchResult {
                generation,
                result: Err(ApiError::Transport)
            }),
            Command::Render
        ));
        assert!(matches!(app.state(), DataState::Stale { .. }));
        assert!(app.snapshot().is_some());
        assert!(matches!(app.update(Event::Input(key('x'))), Command::None));
        assert!(matches!(app.update(Event::Input(key('q'))), Command::Quit));
    }

    #[test]
    fn successful_empty_and_startup_failure_have_distinct_states() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        let empty = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![],
            None,
        );
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(empty)),
        });
        assert!(matches!(app.state(), DataState::Empty { .. }));

        let mut failed = App::new();
        let Command::Fetch { generation, .. } = failed.update(Event::Start) else {
            panic!()
        };
        failed.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        assert!(matches!(
            failed.state(),
            DataState::Fatal(ApiError::Transport)
        ));
        assert!(failed.snapshot().is_none());
    }

    #[test]
    fn selection_clamps_to_rows_and_survives_empty_snapshots() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert_eq!(app.selected(), 0);
        app.select(500);
        assert_eq!(app.selected(), 0);

        let base = CoinMarketInput {
            id: "x".into(),
            rank: None,
            name: "X".into(),
            symbol: "x".into(),
            price: None,
            change_1h: None,
            change_24h: None,
            change_7d: None,
            market_cap: None,
            volume_24h: None,
            circulating_supply: None,
            sparkline_7d: vec![],
        };
        let many = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            (0..5)
                .map(|rank| CoinMarketInput {
                    id: rank.to_string(),
                    rank: Some(rank),
                    name: format!("Coin {rank}"),
                    ..base.clone()
                })
                .collect(),
            None,
        );
        let mut many_app = App::new();
        let Command::Fetch { generation, .. } = many_app.update(Event::Start) else {
            panic!()
        };
        many_app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(many)),
        });
        many_app.select(4);
        assert_eq!(many_app.selected(), 4);
        let Command::Fetch { generation, .. } = many_app.update(Event::Input(key('r'))) else {
            panic!()
        };
        many_app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert_eq!(many_app.selected(), 0);
    }

    fn ready_app(count: usize) -> App {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        let rows = (0..count)
            .map(|rank| CoinMarketInput {
                id: rank.to_string(),
                rank: Some(rank as u32),
                name: format!("Coin {rank}"),
                symbol: "x".into(),
                price: None,
                change_1h: None,
                change_24h: None,
                change_7d: None,
                market_cap: None,
                volume_24h: None,
                circulating_supply: None,
                sparkline_7d: vec![],
            })
            .collect();
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(MarketSnapshot::new(
                MarketSummaryInput {
                    total_market_cap: None,
                    total_volume_24h: None,
                    btc_dominance: None,
                    market_cap_change_24h: None,
                },
                rows,
                None,
            ))),
        });
        app
    }

    fn nav(app: &mut App, code: KeyCode) -> Command {
        app.update(Event::Input(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    fn is_render(command: Command) -> bool {
        matches!(command, Command::Render)
    }

    #[test]
    fn navigation_keys_cover_boundaries_and_empty_lists() {
        let mut app = ready_app(100);
        assert_eq!(app.selected(), 0);
        assert!(is_render(nav(&mut app, KeyCode::Down)));
        assert_eq!(app.selected(), 1);
        assert!(is_render(nav(&mut app, KeyCode::Char('j'))));
        assert_eq!(app.selected(), 2);
        assert!(is_render(nav(&mut app, KeyCode::End)));
        assert_eq!(app.selected(), 99, "End lands on the last row");
        assert!(is_render(nav(&mut app, KeyCode::Down)));
        assert_eq!(app.selected(), 99, "next stays clamped at the last row");
        assert!(is_render(nav(&mut app, KeyCode::Char('G'))));
        assert_eq!(app.selected(), 99);
        assert!(is_render(nav(&mut app, KeyCode::Char('j'))));
        assert_eq!(app.selected(), 99, "j stays clamped at the last row");

        assert!(is_render(nav(&mut app, KeyCode::Home)));
        assert_eq!(app.selected(), 0, "Home lands on the first row");
        assert!(is_render(nav(&mut app, KeyCode::Up)));
        assert_eq!(app.selected(), 0, "previous stays clamped at the first row");
        assert!(is_render(nav(&mut app, KeyCode::Char('g'))));
        assert_eq!(app.selected(), 0);
        assert!(is_render(nav(&mut app, KeyCode::Char('k'))));
        assert_eq!(app.selected(), 0, "k stays clamped at the first row");
    }

    #[test]
    fn page_keys_move_by_the_resize_viewport_and_clamp() {
        let mut app = ready_app(100);
        app.update(Event::Resize { height: 24 });
        let viewport = input::table_viewport(24);
        assert_eq!(viewport, 9, "24-row terminal shows 9 table rows");
        assert!(is_render(nav(&mut app, KeyCode::PageDown)));
        assert_eq!(app.selected(), viewport, "PageDown advances one viewport");
        assert!(is_render(nav(&mut app, KeyCode::PageDown)));
        assert_eq!(app.selected(), viewport * 2);
        assert!(is_render(nav(&mut app, KeyCode::End)));
        assert!(is_render(nav(&mut app, KeyCode::PageDown)));
        assert_eq!(app.selected(), 99, "PageDown clamps at the last row");
        assert!(is_render(nav(&mut app, KeyCode::PageUp)));
        assert_eq!(
            app.selected(),
            99 - viewport,
            "PageUp steps back one viewport"
        );
        assert!(is_render(nav(&mut app, KeyCode::Home)));
        assert!(is_render(nav(&mut app, KeyCode::PageUp)));
        assert_eq!(app.selected(), 0, "PageUp clamps at the first row");
    }

    #[test]
    fn navigation_is_a_noop_when_there_are_no_rows() {
        let mut app = ready_app(0);
        app.update(Event::Resize { height: 24 });
        for code in [
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('g'),
            KeyCode::Char('G'),
        ] {
            assert!(is_render(nav(&mut app, code)), "{code:?}");
            assert_eq!(app.selected(), 0, "{code:?} leaves selection at zero");
        }
    }

    #[test]
    fn viewport_has_a_floor_and_resize_updates_it() {
        assert_eq!(input::table_viewport(0), 1);
        assert_eq!(input::table_viewport(5), 1);
        assert_eq!(input::table_viewport(60), 27);
        let mut app = ready_app(10);
        app.update(Event::Resize { height: 30 });
        assert_eq!(input::table_viewport(30), 12);
        assert!(is_render(nav(&mut app, KeyCode::PageDown)));
        assert_eq!(app.selected(), 9, "PageDown clamps to the row count");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn accepted_refresh_starts_fetch_and_redraws_before_completion() {
        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let mut controller = Controller::new(provider.clone());
        controller.start_initial_refresh();
        tokio::task::yield_now().await;
        assert!(matches!(
            controller
                .handle(Event::FetchResult {
                    generation: 1,
                    result: Ok(outcome(snapshot())),
                })
                .await,
            Command::Render
        ));
        let renders = Arc::new(AtomicUsize::new(0));
        renders.fetch_add(1, Ordering::Relaxed);
        assert!(matches!(
            controller.handle(Event::Input(key('r'))).await,
            Command::Fetch { .. }
        ));
        renders.fetch_add(1, Ordering::Relaxed);
        assert!(controller.app.fetching());
        assert!(matches!(
            controller.handle(Event::Input(key('r'))).await,
            Command::None
        ));
        assert_eq!(renders.load(Ordering::Relaxed), 2);
        tokio::task::yield_now().await;
        assert_eq!(provider.calls.load(Ordering::Relaxed), 2);
        controller.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn eof_and_render_errors_run_bounded_shutdown_and_preserve_loop_error() {
        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let eof = run_loop_with_sources(provider, stream::empty(), |_| Ok(())).await;
        assert!(eof.is_ok());

        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let error = run_loop_with_sources(provider, stream::empty(), |_| {
            Err(io::Error::other("draw failed"))
        })
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "draw failed");

        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let input_error = run_loop_with_sources(
            provider,
            stream::iter([Err(io::Error::other("input failed"))]),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
        assert_eq!(input_error.to_string(), "input failed");

        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let later_draw = run_loop_with_sources(
            provider,
            stream::iter([Ok(CrosstermEvent::Resize(80, 24))]),
            |_| Err(io::Error::other("later draw failed")),
        )
        .await
        .unwrap_err();
        assert_eq!(later_draw.to_string(), "later draw failed");
        // The initial draw fails before the event loop can poll the initial
        // operation, so no provider future exists to observe a drop here.
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_joins_the_active_provider_task() {
        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let dropped = Arc::clone(&provider.dropped);
        let mut controller = Controller::new(provider);
        controller.start_initial_refresh();
        tokio::task::yield_now().await;
        assert!(controller.has_active_task());
        controller.shutdown().await.unwrap();
        assert!(!controller.has_active_task());
        assert!(dropped.load(Ordering::Acquire));
    }

    #[test]
    fn state_table_preserves_snapshot_and_freshness_on_refresh_failure() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        let DataState::Ready {
            snapshot: ready_snapshot,
            refreshed_at,
            ..
        } = app.state()
        else {
            panic!()
        };
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        assert!(matches!(
            app.state(),
            DataState::Stale {
                snapshot,
                refreshed_at: same_time,
                error: ApiError::Transport,
                ..
            } if snapshot == ready_snapshot && same_time == refreshed_at
        ));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn summary_notice_keeps_rows_and_clears_on_next_clean_success() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(FetchOutcome {
                snapshot: snapshot(),
                summary_notice: Some(ApiError::RateLimited {
                    retry_after: Some(Duration::from_secs(7)),
                }),
            }),
        });
        assert!(matches!(
            app.state(),
            DataState::Ready {
                snapshot,
                notice: Some(ApiError::RateLimited {
                    retry_after: Some(delay),
                }),
                ..
            } if snapshot.coins().len() == 1 && delay == Duration::from_secs(7)
        ));

        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        assert!(matches!(
            app.state(),
            DataState::Stale {
                notice: Some(ApiError::RateLimited {
                    retry_after: Some(delay),
                }),
                ..
            } if delay == Duration::from_secs(7)
        ));
        assert!(
            app.refresh_cooldown().is_some(),
            "transport failure opens a cooldown"
        );
        assert!(
            matches!(app.update(Event::Input(key('r'))), Command::None),
            "manual refresh is blocked while the cooldown is open"
        );

        tokio::time::advance(Duration::from_secs(3)).await;
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!("a manual refresh is allowed once the cooldown passes")
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert!(matches!(app.state(), DataState::Ready { notice: None, .. }));
        assert!(
            app.refresh_cooldown().is_none(),
            "success clears the cooldown"
        );
    }

    #[test]
    fn empty_success_then_failure_becomes_stale_with_empty_snapshot() {
        let empty = MarketSnapshot::new(
            MarketSummaryInput {
                total_market_cap: None,
                total_volume_24h: None,
                btc_dominance: None,
                market_cap_change_24h: None,
            },
            vec![],
            None,
        );
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(empty.clone())),
        });
        let DataState::Empty { refreshed_at, .. } = app.state() else {
            panic!()
        };
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        assert!(matches!(
            app.state(),
            DataState::Stale { snapshot, refreshed_at: same_time, .. }
                if snapshot == empty && same_time == refreshed_at
        ));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn success_resets_the_refresh_cadence() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert!(app.refresh_cooldown().is_none());

        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(
            is_render(app.update(Event::Tick)),
            "no automatic refresh before the interval elapses"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        let Command::Fetch { generation, .. } = app.update(Event::Tick) else {
            panic!("automatic refresh fires once the interval elapses")
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });

        tokio::time::advance(Duration::from_secs(59)).await;
        assert!(
            is_render(app.update(Event::Tick)),
            "a success resets the cadence to a full interval"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(
            matches!(app.update(Event::Tick), Command::Fetch { .. }),
            "the reset cadence fires again on schedule"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn retry_after_opens_a_cooldown_that_blocks_manual_refresh() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
            }),
        });
        assert_eq!(
            app.refresh_cooldown(),
            Some(Duration::from_secs(30)),
            "Retry-After opens an exact cooldown window"
        );

        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(
            matches!(app.update(Event::Input(key('r'))), Command::None),
            "manual refresh is blocked while the cooldown is open"
        );
        tokio::time::advance(Duration::from_secs(19)).await;
        assert!(
            is_render(app.update(Event::Tick)),
            "no automatic retry before Retry-After elapses"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(
            matches!(app.update(Event::Tick), Command::Fetch { .. }),
            "the automatic retry fires after the Retry-After window"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn backoff_window_is_capped_after_repeated_failures() {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::Transport),
        });
        let first = app.refresh_cooldown().expect("failure opens a cooldown");
        assert!(
            first >= Duration::from_secs(1) && first <= Duration::from_secs(2),
            "first backoff sits in the base window: {first:?}"
        );

        for failure in 0..10 {
            let remaining = app.refresh_cooldown().expect("still cooling down");
            tokio::time::advance(remaining + Duration::from_millis(1)).await;
            let Command::Fetch { generation, .. } = app.update(Event::Tick) else {
                panic!("automatic retry fires after the cooldown window")
            };
            app.update(Event::FetchResult {
                generation,
                result: Err(ApiError::Transport),
            });
            let next = app.refresh_cooldown().unwrap();
            assert!(
                next <= Duration::from_secs(60),
                "the backoff window never exceeds the cap: {next:?}"
            );
            if failure >= 5 {
                assert!(
                    next >= Duration::from_secs(30),
                    "a deeply-exponentiated window sits at the cap floor: {next:?}"
                );
            }
        }
    }

    #[test]
    fn jittered_backoff_stays_within_its_scaled_window() {
        for failures in 0..20 {
            for _ in 0..64 {
                let delay = jittered_backoff(failures);
                let scaled = BACKOFF_BASE_MS
                    .saturating_mul(1_u64 << failures.min(60))
                    .min(BACKOFF_MAX_MS);
                let milliseconds = delay.as_millis() as u64;
                let lower = scaled / 2;
                assert!(
                    (lower..=scaled).contains(&milliseconds),
                    "failures={failures} delay {delay:?} outside [{lower}, {scaled}]"
                );
            }
        }
    }

    #[test]
    fn failure_retry_delay_honors_retry_after_and_classifies_errors() {
        assert_eq!(
            failure_retry_delay(
                &ApiError::RateLimited {
                    retry_after: Some(Duration::from_secs(42)),
                },
                7,
            ),
            Duration::from_secs(42),
            "Retry-After is honored exactly"
        );
        assert_eq!(
            failure_retry_delay(
                &ApiError::RateLimited {
                    retry_after: Some(Duration::ZERO),
                },
                0,
            ),
            Duration::from_secs(1),
            "a zero Retry-After is floored to one second"
        );
        let transport = failure_retry_delay(&ApiError::Transport, 0);
        assert!(
            transport >= Duration::from_secs(1) && transport <= Duration::from_secs(2),
            "transport backoff stays in the base window: {transport:?}"
        );
        assert!(
            failure_retry_delay(&ApiError::HttpStatus { status: 503 }, 20)
                <= Duration::from_secs(60),
            "a 5xx failure uses capped backoff"
        );
        assert_eq!(
            failure_retry_delay(&ApiError::HttpStatus { status: 400 }, 0),
            RETRY_GATE,
            "a client error retries on the steady gate"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn tick_drives_an_automatic_refresh_through_the_loop() {
        let calls = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(run_loop_with_sources(
            Arc::new(CountingProvider {
                calls: Arc::clone(&calls),
            }),
            stream::pending::<Result<CrosstermEvent, io::Error>>(),
            |_| Ok(()),
        ));
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "only the initial refresh runs before the interval elapses"
        );
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "the low-frequency tick started an automatic refresh"
        );
        task.abort();
        task.await.unwrap_err();
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn idle_rendering_is_event_driven_and_steady() {
        let renders = Arc::new(AtomicUsize::new(0));
        let rendered = Arc::clone(&renders);
        let task = tokio::spawn(run_loop_with_sources(
            Arc::new(CountingProvider {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            stream::pending::<Result<CrosstermEvent, io::Error>>(),
            move |_| {
                rendered.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        ));
        // Let startup settle: the initial render, the completed fetch result
        // render, and the first tick. Nothing idle should render between
        // events except the low-frequency tick.
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        let settled = renders.load(Ordering::Relaxed);

        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        let first_window = renders.load(Ordering::Relaxed).saturating_sub(settled);

        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        let second_window = renders
            .load(Ordering::Relaxed)
            .saturating_sub(settled + first_window);

        // A 1 Hz idle tick means ~30 renders per 30-second window, not tens
        // of thousands per second. A hot loop would blow these bounds.
        assert!(
            (26..=36).contains(&first_window),
            "first 30s idle rendered {first_window} times"
        );
        assert!(
            (26..=36).contains(&second_window),
            "second 30s idle rendered {second_window} times"
        );
        assert!(
            first_window.abs_diff(second_window) <= 10,
            "idle rendering stays steady: {first_window} vs {second_window}"
        );
        assert!(
            renders.load(Ordering::Relaxed) - settled <= 72,
            "60s idle rendered far more than the tick cadence"
        );
        task.abort();
        task.await.unwrap_err();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn refresh_duration_is_recorded_in_the_trace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("timing.log");
        let tracer = Some(FileLog::append_at(&path, Vec::new()).unwrap());
        run_loop_with_sources_and_tracer(
            Arc::new(CountingProvider {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoNewsProvider),
            None,
            eof_after(Duration::from_millis(50)),
            |_| Ok(()),
            tracer,
            Duration::from_secs(crate::config::DEFAULT_REFRESH_SECONDS),
        )
        .await
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("refresh ok generation=1 coins=1"),
            "{content}"
        );
        let has_duration = content
            .lines()
            .any(|line| line.contains("refresh ok") && line.contains("duration="));
        assert!(has_duration, "refresh ok lines carry a duration: {content}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hostile_provider_fixtures_never_corrupt_the_terminal_or_leak_the_key() {
        let key = "super-secret-api-key";
        let hostile_body = format!(
            r#"[{{"id":"bitcoin","name":"{name}","symbol":"{symbol}","market_cap_rank":1,"current_price":50000,"market_cap":1000000,"sparkline_in_7d":{{"price":[1]}}}}]"#,
            name = "\\u001b[31mBitcoin\\u200e\\u0000\\u0007\\u007f",
            symbol = "\\u001b[1mBTC\\u200b",
        );
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/coins/markets"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(hostile_body.as_bytes(), "application/json"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/global"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"data":{"total_market_cap":{"usd":1},"total_volume":{"usd":2},"market_cap_percentage":{"btc":3},"market_cap_change_percentage_24h_usd":4}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let provider = Arc::new(
            CoinGeckoClient::with_timeouts(
                &server.uri(),
                Some(key.into()),
                Duration::from_millis(100),
                Duration::from_secs(1),
            )
            .unwrap(),
        );
        let mut controller = Controller::with_tracer(provider, None);
        controller.start_initial_refresh();
        let event = controller.next_event().await.expect("one refresh result");
        controller.handle(event).await;
        controller.shutdown().await.unwrap();

        assert!(matches!(controller.app.state(), DataState::Ready { .. }));
        let rendered = render_text(&controller.app);
        assert!(
            rendered.chars().all(|character| !character.is_control()),
            "terminal text contains control characters: {rendered:?}"
        );
        assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
        assert!(!rendered.contains('\u{200e}') && !rendered.contains('\u{200b}'));
        assert!(
            rendered.contains("Bitcoin") && rendered.contains("BTC"),
            "{rendered:?}"
        );
        assert!(
            !rendered.contains(key),
            "key leaked into the screen: {rendered:?}"
        );

        let requests = server.received_requests().await.unwrap();
        let markets = requests
            .iter()
            .find(|request| request.url.path() == "/api/v3/coins/markets")
            .unwrap();
        assert_eq!(
            markets.headers.get("x-cg-demo-api-key").unwrap(),
            key,
            "the key must be sent as the provider header"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_session_logs_redact_the_key_and_record_the_refresh_timeline() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.log");
        let key = "plumbus-secret-key";
        let tracer = Some(FileLog::append_at(&path, vec![key.into()]).unwrap());
        let result = run_loop_with_sources_and_tracer(
            Arc::new(CountingProvider {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            Arc::new(NoNewsProvider),
            None,
            eof_after(Duration::from_millis(50)),
            |_| Ok(()),
            tracer,
            Duration::from_secs(crate::config::DEFAULT_REFRESH_SECONDS),
        )
        .await;
        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        for expected in [
            "session start",
            "refresh start generation=1",
            "refresh ok generation=1 coins=1",
            "loop stopped success=true",
        ] {
            assert!(
                content.contains(expected),
                "missing {expected:?} in {content}"
            );
        }
        assert!(
            !content.contains(key),
            "key leaked into the trace log: {content}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn traced_failed_refresh_logs_the_error_without_the_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("failure.log");
        let key = "another-secret-key";
        let tracer = Some(FileLog::append_at(&path, vec![key.into()]).unwrap());
        let result = run_loop_with_sources_and_tracer(
            Arc::new(FailingProvider),
            Arc::new(NoNewsProvider),
            None,
            eof_after(Duration::from_millis(50)),
            |_| Ok(()),
            tracer,
            Duration::from_secs(crate::config::DEFAULT_REFRESH_SECONDS),
        )
        .await;
        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("refresh failed generation=1"), "{content}");
        assert!(content.contains("error=API transport failed"), "{content}");
        assert!(
            content.contains("duration="),
            "refresh timing is recorded in the trace: {content}"
        );
        assert!(
            !content.contains(key),
            "key leaked into the trace log: {content}"
        );
    }

    #[test]
    fn malformed_response_keeps_last_good_rows_and_renders_cleanly() {
        let mut app = loaded_app(snapshot());
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Err(ApiError::MalformedResponse),
        });
        assert!(matches!(app.state(), DataState::Stale { .. }));
        let rendered = render_text(&app);
        assert!(rendered.contains("STALE"), "{rendered:?}");
        assert!(
            rendered.chars().all(|character| !character.is_control()),
            "oversized-as-malformed responses must not corrupt the terminal: {rendered:?}"
        );
        assert!(
            rendered.contains("STALE") && rendered.contains("X"),
            "{rendered:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn parent_cancellation_drops_pending_provider_future() {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::clone(&dropped),
            first_polled: Arc::new(Notify::new()),
        });
        let task = tokio::spawn(run_loop_with_sources(
            provider,
            stream::pending::<Result<CrosstermEvent, io::Error>>(),
            |_| Ok(()),
        ));
        tokio::task::yield_now().await;
        task.abort();
        task.await.unwrap_err();
        assert!(dropped.load(Ordering::Acquire));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resize_is_rendered() {
        let renders = Arc::new(AtomicUsize::new(0));
        let rendered = Arc::clone(&renders);
        let result = run_loop_with_sources(
            Arc::new(PendingProvider {
                calls: AtomicUsize::new(0),
                dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                first_polled: Arc::new(Notify::new()),
            }),
            stream::iter([
                Ok(CrosstermEvent::Resize(80, 24)),
                Ok(CrosstermEvent::Key(key('q'))),
            ]),
            move |_| {
                rendered.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(renders.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delayed_real_provider_refresh_stays_single_and_quits_promptly() {
        let server = MockServer::start().await;
        let delayed = ResponseTemplate::new(200)
            .set_body_raw(
                br#"[{"id":"bitcoin","name":"Bitcoin","symbol":"btc","market_cap_rank":1,"current_price":50000}]"#,
                "application/json",
            )
            .set_delay(Duration::from_secs(30));
        Mock::given(method("GET"))
            .and(path("/api/v3/coins/markets"))
            .respond_with(delayed.clone())
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v3/global"))
            .respond_with(
                delayed
                    .clone()
                    .set_body_raw(
                        br#"{"data":{"total_market_cap":{"usd":1},"total_volume":{"usd":2},"market_cap_percentage":{"btc":3},"market_cap_change_percentage_24h_usd":4}}"#,
                        "application/json",
                    ),
            )
            .mount(&server)
            .await;

        let provider = Arc::new(
            CoinGeckoClient::with_timeouts(
                &server.uri(),
                None,
                Duration::from_secs(1),
                Duration::from_secs(60),
            )
            .unwrap(),
        );
        let mut controller = Controller::new(provider);
        controller.start_initial_refresh();

        let requests = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let requests = server.received_requests().await.unwrap_or_default();
                if requests.len() == 2 {
                    break requests;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both delayed requests should arrive");
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/api/v3/coins/markets")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/api/v3/global")
                .count(),
            1
        );

        for _ in 0..20 {
            assert!(matches!(
                controller.handle(Event::Input(key('r'))).await,
                Command::None
            ));
        }
        assert!(matches!(
            controller.handle(Event::Input(key('q'))).await,
            Command::Quit
        ));
        let started_shutdown = tokio::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(2), controller.shutdown())
            .await
            .expect("quit should cancel delayed provider work")
            .unwrap();
        assert!(started_shutdown.elapsed() < Duration::from_secs(2));
        assert!(!controller.has_active_task());
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    fn coin_row(id: &str, rank: u32, name: &str, symbol: &str) -> CoinMarketInput {
        CoinMarketInput {
            id: id.into(),
            rank: Some(rank),
            name: name.into(),
            symbol: symbol.into(),
            price: Some(1.0),
            change_1h: None,
            change_24h: None,
            change_7d: None,
            market_cap: None,
            volume_24h: None,
            circulating_supply: None,
            sparkline_7d: vec![],
        }
    }

    fn mixed_snapshot(rows: Vec<CoinMarketInput>) -> MarketSnapshot {
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

    fn loaded_app(snapshot: MarketSnapshot) -> App {
        let mut app = App::new();
        let Command::Fetch { generation, .. } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot)),
        });
        app
    }

    fn type_query(app: &mut App, query: &str) {
        assert!(is_render(app.update(Event::Input(key('/')))));
        for character in query.chars() {
            assert!(is_render(app.update(Event::Input(key(character)))));
        }
    }

    /// `Enter` commits a search (Render) or opens the detail pane and starts
    /// the rich-detail fetch (FetchDetail); both are the "did something" forms.
    fn enter(app: &mut App) {
        let command = app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(
            matches!(command, Command::Render | Command::FetchDetail { .. }),
            "Enter must render or open detail"
        );
    }

    #[test]
    fn typing_never_triggers_global_shortcuts_and_ctrl_c_still_quits() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("bitcoin", 1, "Bitcoin", "BTC"),
            coin_row("ether", 2, "Ethereum", "ETH"),
        ]));
        assert!(is_render(app.update(Event::Input(key('/')))));
        assert!(app.searching());
        assert_eq!(app.selected(), 0);
        for character in ['r', 'j', 'q', 'g', 'k', 'G'] {
            assert!(is_render(app.update(Event::Input(key(character)))));
        }
        assert_eq!(app.search_buffer(), "rjqgkG");
        assert!(!app.fetching(), "r is typed, not a refresh");
        assert_eq!(app.selected(), 0, "navigation keys are typed, not moves");
        assert!(is_render(app.update(Event::Input(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE
        )))));
        assert_eq!(app.search_buffer(), "rjqgk");
        assert!(
            matches!(
                app.update(Event::Input(KeyEvent::new(
                    KeyCode::Char('c'),
                    KeyModifiers::CONTROL
                ))),
                Command::Quit
            ),
            "Ctrl-C quits even while typing a query"
        );
    }

    #[test]
    fn enter_applies_buffer_and_esc_cancels_without_touching_the_filter() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("abt", 1, "Adventure Blockchain Token", "ABT"),
            coin_row("bitcoin", 2, "Bitcoin", "BTC"),
            coin_row("litecoin", 3, "Litecoin", "LTC"),
        ]));
        type_query(&mut app, "eth");
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!app.searching());
        assert!(app.search_buffer().is_empty());
        assert_eq!(app.search_query(), "");
        assert_eq!(
            app.visible_coins().len(),
            3,
            "cancelled search filters nothing"
        );

        type_query(&mut app, "btc");
        enter(&mut app);
        assert!(!app.searching());
        assert_eq!(app.search_query(), "btc");
        let ids: Vec<&str> = app.visible_coins().iter().map(|coin| coin.id()).collect();
        assert_eq!(ids, vec!["bitcoin"], "entered search filters to one match");

        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert_eq!(
            app.search_query(),
            "btc",
            "applied filter survives a refresh"
        );
    }

    #[test]
    fn filter_matches_names_and_symbols_case_insensitively() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("litecoin", 1, "Litecoin", "LTC"),
            coin_row("abt", 2, "Adventure", "ABT"),
            coin_row("bitcoin", 3, "Bitcoin", "BTC"),
        ]));
        type_query(&mut app, "BT");
        enter(&mut app);
        assert_eq!(app.search_query(), "BT");
        let ids: Vec<&str> = app.visible_coins().iter().map(|coin| coin.id()).collect();
        assert_eq!(
            ids,
            vec!["abt", "bitcoin"],
            "uppercase query matches a lowercased symbol and name"
        );
        assert!(!app
            .visible_coins()
            .iter()
            .any(|coin| coin.id() == "litecoin"));
    }

    #[test]
    fn selection_stays_on_the_same_coin_id_after_filter_and_clamps_otherwise() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("litecoin", 1, "Litecoin", "LTC"),
            coin_row("bitbo", 2, "Bitbo", "BBO"),
            coin_row("bitcoin", 3, "Bitcoin", "BTC"),
            coin_row("ether", 4, "Ethereum", "ETH"),
        ]));
        app.select(2);
        assert_eq!(app.visible_coins()[app.selected()].id(), "bitcoin");
        type_query(&mut app, "bit");
        enter(&mut app);
        let visible: Vec<&str> = app.visible_coins().iter().map(|coin| coin.id()).collect();
        assert_eq!(visible, vec!["bitbo", "bitcoin"]);
        assert_eq!(
            app.visible_coins()[app.selected()].id(),
            "bitcoin",
            "selection moves to the same coin id in the filtered set"
        );

        app.select(0);
        type_query(&mut app, "eth");
        enter(&mut app);
        let visible: Vec<&str> = app.visible_coins().iter().map(|coin| coin.id()).collect();
        assert_eq!(visible, vec!["ether"]);
        assert_eq!(
            app.visible_coins()[app.selected()].id(),
            "ether",
            "a dropped anchor clamps the selection into the filtered set"
        );
    }

    #[test]
    fn no_results_is_empty_and_esc_clears_the_committed_filter() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("bitcoin", 1, "Bitcoin", "BTC"),
            coin_row("litecoin", 2, "Litecoin", "LTC"),
        ]));
        type_query(&mut app, "zzz");
        enter(&mut app);
        assert!(app.has_active_filter());
        assert_eq!(app.visible_coins().len(), 0);
        assert_eq!(app.row_count(), 0);
        assert!(is_render(nav(&mut app, KeyCode::Down)));
        assert_eq!(app.selected(), 0, "navigation no-ops with no visible rows");

        assert!(is_render(app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )))));
        assert!(!app.has_active_filter());
        assert_eq!(app.visible_coins().len(), 2, "idle Esc clears the filter");
    }

    #[test]
    fn unicode_search_matches_and_backspace_edits_by_scalar() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("cn", 1, "比特币", "CNB"),
            coin_row("x", 2, "Other", "OTH"),
        ]));
        type_query(&mut app, "比特币");
        assert_eq!(app.search_buffer(), "比特币");
        enter(&mut app);
        assert_eq!(app.search_query(), "比特币");
        let ids: Vec<&str> = app.visible_coins().iter().map(|coin| coin.id()).collect();
        assert_eq!(ids, vec!["cn"]);

        type_query(&mut app, "比特");
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Backspace,
            KeyModifiers::NONE,
        )));
        assert_eq!(app.search_buffer(), "比");
        app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE,
        )));
        assert!(!app.searching());
    }

    #[test]
    fn search_buffer_is_bounded_to_prevent_overflow() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("x", 1, "X", "X")]));
        type_query(&mut app, "");
        for _ in 0..(MAX_SEARCH_CHARS + 10) {
            app.update(Event::Input(key('a')));
        }
        assert_eq!(app.search_buffer().chars().count(), MAX_SEARCH_CHARS);
    }

    #[test]
    fn s_cycles_all_keys_and_directions_and_shift_s_walks_back() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("x", 1, "X", "X")]));
        assert_eq!(app.sort_state(), SortState::default());
        let expected: [(SortKey, bool); 15] = [
            (SortKey::Rank, false),
            (SortKey::Price, true),
            (SortKey::Price, false),
            (SortKey::Change1h, true),
            (SortKey::Change1h, false),
            (SortKey::Change24h, true),
            (SortKey::Change24h, false),
            (SortKey::Change7d, true),
            (SortKey::Change7d, false),
            (SortKey::Cap, true),
            (SortKey::Cap, false),
            (SortKey::Volume, true),
            (SortKey::Volume, false),
            (SortKey::Supply, true),
            (SortKey::Supply, false),
        ];
        for (index, (sort_key, ascending)) in expected.iter().enumerate() {
            assert!(is_render(app.update(Event::Input(key('s')))));
            let sort = app.sort_state();
            assert_eq!(sort.key(), *sort_key, "forward step {index}");
            assert_eq!(sort.ascending(), *ascending, "forward step {index}");
        }
        assert!(is_render(app.update(Event::Input(key('s')))));
        assert_eq!(
            app.sort_state(),
            SortState::default(),
            "s wraps back to rank order"
        );

        let shift_s = KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT);
        assert!(is_render(app.update(Event::Input(shift_s))));
        let sort = app.sort_state();
        assert_eq!(sort.key(), SortKey::Supply, "Shift-S walks back one step");
        assert!(!sort.ascending());
        assert!(is_render(app.update(Event::Input(shift_s))));
        let sort = app.sort_state();
        assert_eq!(sort.key(), SortKey::Supply);
        assert!(sort.ascending());
    }

    fn with_numbers(
        row: CoinMarketInput,
        values: (f64, f64, f64, f64, f64, f64, f64),
    ) -> CoinMarketInput {
        let (price, change_1h, change_24h, change_7d, market_cap, volume_24h, circulating_supply) =
            values;
        let mut row = row;
        row.price = Some(price);
        row.change_1h = Some(change_1h);
        row.change_24h = Some(change_24h);
        row.change_7d = Some(change_7d);
        row.market_cap = Some(market_cap);
        row.volume_24h = Some(volume_24h);
        row.circulating_supply = Some(circulating_supply);
        row
    }

    fn tie_row(id: &str, name: &str) -> CoinMarketInput {
        let mut row = coin_row(id, 1, name, "TT");
        row.rank = None;
        row.price = Some(60.0);
        row.change_24h = Some(0.0);
        row
    }

    fn missing_row(id: &str, name: &str) -> CoinMarketInput {
        let mut row = coin_row(id, 1, name, "M");
        row.rank = None;
        row.price = None;
        row
    }

    fn assert_order(app: &App, key: SortKey, ascending: bool, expected: &[&str]) {
        let ids: Vec<&str> = app.visible_coins().iter().map(|coin| coin.id()).collect();
        assert_eq!(
            ids,
            expected,
            "{:?} {}",
            key,
            if ascending { "asc" } else { "desc" }
        );
    }

    #[test]
    fn every_numeric_column_sorts_both_directions_with_missing_last_and_stable_ties() {
        let rows = vec![
            with_numbers(
                coin_row("a", 1, "A", "A"),
                (100.0, 1.0, 2.0, 3.0, 1000.0, 10.0, 5.0),
            ),
            with_numbers(
                coin_row("b", 2, "B", "B"),
                (50.0, 0.5, 1.0, 2.0, 2000.0, 20.0, 8.0),
            ),
            with_numbers(
                coin_row("c", 3, "C", "C"),
                (200.0, -1.0, -2.0, -3.0, 3000.0, 30.0, 2.0),
            ),
            missing_row("m", "Missing"),
            tie_row("t1", "T1"),
            tie_row("t2", "T2"),
        ];
        let mut app = loaded_app(mixed_snapshot(rows));
        let cases: [(SortKey, &[&str], &[&str]); 8] = [
            (
                SortKey::Rank,
                &["a", "b", "c", "m", "t1", "t2"],
                &["c", "b", "a", "m", "t1", "t2"],
            ),
            (
                SortKey::Price,
                &["b", "t1", "t2", "a", "c", "m"],
                &["c", "a", "t1", "t2", "b", "m"],
            ),
            (
                SortKey::Change1h,
                &["c", "b", "a", "m", "t1", "t2"],
                &["a", "b", "c", "m", "t1", "t2"],
            ),
            (
                SortKey::Change24h,
                &["c", "t1", "t2", "b", "a", "m"],
                &["a", "b", "t1", "t2", "c", "m"],
            ),
            (
                SortKey::Change7d,
                &["c", "b", "a", "m", "t1", "t2"],
                &["a", "b", "c", "m", "t1", "t2"],
            ),
            (
                SortKey::Cap,
                &["a", "b", "c", "m", "t1", "t2"],
                &["c", "b", "a", "m", "t1", "t2"],
            ),
            (
                SortKey::Volume,
                &["a", "b", "c", "m", "t1", "t2"],
                &["c", "b", "a", "m", "t1", "t2"],
            ),
            (
                SortKey::Supply,
                &["c", "a", "b", "m", "t1", "t2"],
                &["b", "a", "c", "m", "t1", "t2"],
            ),
        ];
        for (key, asc, desc) in cases {
            app.set_sort(key, true);
            assert_order(&app, key, true, asc);
            app.set_sort(key, false);
            assert_order(&app, key, false, desc);
        }
        app.set_sort(SortKey::Rank, true);
        assert!(!app.sort_active());
        assert_order(&app, SortKey::Rank, true, &["a", "b", "c", "m", "t1", "t2"]);
    }

    #[test]
    fn sort_moves_selection_with_its_coin_id_and_composes_with_filter() {
        let rows = vec![
            with_numbers(
                coin_row("a", 1, "Alpha", "A"),
                (100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            ),
            with_numbers(
                coin_row("b", 2, "Beta", "B"),
                (50.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            ),
            with_numbers(
                coin_row("c", 3, "Gamma", "C"),
                (200.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            ),
        ];
        let mut app = loaded_app(mixed_snapshot(rows));
        app.select(0);
        assert_eq!(app.visible_coins()[app.selected()].id(), "a");
        for _ in 0..3 {
            app.update(Event::Input(key('s')));
        }
        let sort = app.sort_state();
        assert_eq!(sort.key(), SortKey::Price);
        assert!(!sort.ascending());
        assert_eq!(
            app.visible_coins()[app.selected()].id(),
            "a",
            "selection follows the anchored coin after sorting"
        );
        assert_order(&app, SortKey::Price, false, &["c", "a", "b"]);

        type_query(&mut app, "Gamma");
        enter(&mut app);
        assert_eq!(app.visible_coins().len(), 1);
        assert_eq!(app.visible_coins()[app.selected()].id(), "c");
        app.update(Event::Input(key('s')));
        assert_eq!(
            app.sort_state(),
            SortState {
                key: SortKey::Change1h,
                ascending: true
            },
            "sort cycle advances past a filter"
        );
    }

    #[test]
    fn sort_keys_are_typed_not_triggered_while_searching() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("a", 1, "A", "A"),
            coin_row("b", 2, "B", "B"),
        ]));
        type_query(&mut app, "sS");
        assert_eq!(app.search_buffer(), "sS");
        assert!(!app.sort_active(), "s and S are query text while searching");
        enter(&mut app);
        assert_eq!(app.visible_coins().len(), 0);
    }

    #[test]
    fn search_retention_keeps_anchor_when_filtered_index_overlaps() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("litecoin", 1, "Litecoin", "LTC"),
            coin_row("bitbo", 2, "Bitbo", "BBO"),
            coin_row("bitcoin", 3, "Bitcoin", "BTC"),
        ]));
        app.select(1);
        assert_eq!(app.visible_coins()[app.selected()].id(), "bitbo");
        type_query(&mut app, "bit");
        enter(&mut app);
        assert_eq!(
            app.visible_coins()[app.selected()].id(),
            "bitbo",
            "the anchored coin is not replaced by the coin that fills its old index"
        );
    }

    #[test]
    fn help_toggles_with_question_mark_and_closes_with_question_or_esc() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        assert!(!app.help_open());
        assert!(is_render(app.update(Event::Input(key('?')))));
        assert!(app.help_open());
        assert!(is_render(app.update(Event::Input(key('?')))));
        assert!(!app.help_open(), "second ? closes help");

        assert!(is_render(app.update(Event::Input(key('?')))));
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(is_render(app.update(Event::Input(esc))));
        assert!(!app.help_open(), "Esc closes help");

        let shift_question = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT);
        assert!(is_render(app.update(Event::Input(shift_question))));
        assert!(app.help_open(), "shifted ? also opens help");
    }

    #[test]
    fn help_is_modal_blocks_shortcuts_and_keeps_quit() {
        let mut app = ready_app(5);
        app.update(Event::Input(key('?')));
        assert!(app.help_open());
        let selected = app.selected();
        for route in ['j', 's', 'S', 'r', '/', 'g', 'G', 'k'] {
            let command = app.update(Event::Input(key(route)));
            assert!(
                matches!(command, Command::None),
                "{route} is swallowed while help is open"
            );
        }
        assert!(app.help_open());
        assert_eq!(app.selected(), selected, "navigation is blocked");
        assert!(!app.sort_active(), "sort is blocked");
        assert!(!app.fetching(), "refresh is blocked");
        assert!(!app.searching(), "search is blocked");

        let quit = app.update(Event::Input(key('q')));
        assert!(matches!(quit, Command::Quit), "q still quits over help");
    }

    #[test]
    fn question_mark_is_query_text_while_searching() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        type_query(&mut app, "btc?eth");
        assert_eq!(app.search_buffer(), "btc?eth");
        assert!(!app.help_open(), "? while typing never opens help");
    }

    #[test]
    fn enter_opens_detail_for_the_selected_coin_and_esc_returns_preserving_selection() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("bitcoin", 1, "Bitcoin", "BTC"),
            coin_row("litecoin", 2, "Litecoin", "LTC"),
        ]));
        app.select(1);
        assert!(
            matches!(
                app.update(Event::Input(KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE
                ))),
                Command::FetchDetail { .. }
            ),
            "Enter starts the rich-detail fetch"
        );
        assert!(app.detail_open());
        assert!(app.detail_fetching(), "a detail request is in flight");
        assert_eq!(app.detail().unwrap().id(), "litecoin");
        assert_eq!(
            app.selected(),
            1,
            "selection is untouched while detail is open"
        );
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(is_render(app.update(Event::Input(esc))));
        assert!(!app.detail_open());
        assert_eq!(app.selected(), 1, "Esc returns with selection preserved");
        assert_eq!(app.visible_coins()[app.selected()].id(), "litecoin");
    }

    #[test]
    fn detail_is_modal_but_keeps_esc_help_refresh_and_quit() {
        let mut app = loaded_app(mixed_snapshot(vec![
            coin_row("bitcoin", 1, "Bitcoin", "BTC"),
            coin_row("litecoin", 2, "Litecoin", "LTC"),
        ]));
        enter(&mut app);
        assert!(app.detail_open());
        assert_eq!(app.detail_range(), ChartRange::Week);
        assert!(is_render(app.update(Event::Input(key(']')))));
        assert_eq!(app.detail_range(), ChartRange::Month);
        assert!(is_render(app.update(Event::Input(key('[')))));
        assert_eq!(app.detail_range(), ChartRange::Week);
        assert!(is_render(app.update(Event::Input(key('/')))));
        assert_eq!(app.detail_range(), ChartRange::Month);
        let selected = app.selected();
        for route in ['j', 'k', 'g', 'G', 's', 'S'] {
            assert!(
                matches!(app.update(Event::Input(key(route))), Command::None),
                "{route} is swallowed while detail is open"
            );
        }
        assert_eq!(app.selected(), selected, "navigation is blocked in detail");
        assert!(!app.sort_active(), "sort is blocked in detail");
        assert!(!app.searching(), "search is blocked in detail");
        assert!(!app.fetching(), "no refresh started by swallowed keys");

        assert!(is_render(app.update(Event::Input(key('?')))));
        assert!(
            app.help_open() && app.detail_open(),
            "? opens help over detail"
        );
        assert!(is_render(app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )))));
        assert!(
            !app.help_open() && app.detail_open(),
            "Esc closes help before the detail pane"
        );
        assert!(is_render(app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )))));
        assert!(!app.detail_open(), "a second Esc returns to the table");

        enter(&mut app);
        assert!(app.detail_open());
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!("r still refreshes while detail is open")
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert!(
            app.detail_open(),
            "a completed refresh keeps the detail pane open"
        );
        assert!(matches!(app.update(Event::Input(key('q'))), Command::Quit));
    }

    #[test]
    fn enter_on_empty_rows_does_not_open_detail() {
        let mut fresh = App::new();
        assert!(matches!(
            fresh.update(Event::Input(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Command::None
        ));
        assert!(!fresh.detail_open());

        let mut empty = loaded_app(mixed_snapshot(vec![]));
        assert!(matches!(
            empty.update(Event::Input(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))),
            Command::None
        ));
        assert!(!empty.detail_open());
    }

    #[test]
    fn detail_coin_tracks_a_refreshed_snapshot_by_id() {
        let row = |price| {
            with_numbers(
                coin_row("bitcoin", 1, "Bitcoin", "BTC"),
                (price, 1.0, 2.0, 3.0, 1000.0, 10.0, 5.0),
            )
        };
        let mut app = loaded_app(mixed_snapshot(vec![row(100.0)]));
        enter(&mut app);
        assert_eq!(app.detail().unwrap().price(), Some(100.0));
        let Command::Fetch { generation, .. } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(mixed_snapshot(vec![row(200.0)]))),
        });
        assert_eq!(
            app.detail().unwrap().price(),
            Some(200.0),
            "the detail pane refreshes its coin when the snapshot is replaced"
        );
    }

    #[test]
    fn detail_upgrades_from_loading_to_ready_when_the_fetch_returns() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row(
            "bitcoin", 1, "Bitcoin", "BTC",
        )]));
        let Command::FetchDetail { id, generation } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))) else {
            panic!("Enter must open the detail and start the rich fetch")
        };
        assert_eq!(id, "bitcoin");
        assert!(matches!(
            app.detail_state(),
            Some(DetailState::Loading { .. })
        ));

        app.update(Event::DetailResult {
            id: "bitcoin".to_owned(),
            generation,
            result: Ok(Box::new(rich_detail())),
        });
        assert!(
            matches!(
                app.detail_state(),
                Some(DetailState::Ready { detail, .. }) if detail.ath() == Some(100_000.0)
            ),
            "a completed detail fetch upgrades the pane to Ready"
        );
        let rendered = render_text_at(&app, 200, 30);
        assert!(
            rendered.contains("ATH: $100K") && rendered.contains("Categories: layer-1"),
            "the rich detail fields render: {rendered:?}"
        );

        let stale = app.update(Event::DetailResult {
            id: "bitcoin".to_owned(),
            generation: generation.wrapping_add(1),
            result: Err(ApiError::Transport),
        });
        assert!(
            matches!(stale, Command::None),
            "a stale-generation detail result is ignored"
        );
        assert!(
            matches!(app.detail_state(), Some(DetailState::Ready { .. })),
            "a stale result never downgrades a Ready pane"
        );
    }

    #[test]
    fn chart_result_upgrades_the_30_day_series_in_any_arrival_order() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row(
            "bitcoin", 1, "Bitcoin", "BTC",
        )]));
        let Command::FetchDetail { id, generation } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))) else {
            panic!("Enter must open the detail and start the rich fetch")
        };
        assert_eq!(id, "bitcoin");

        // The chart series lands before the rich detail: it is carried on the
        // Loading state, then preserved when the detail upgrades to Ready.
        app.update(Event::ChartResult {
            id: "bitcoin".to_owned(),
            generation,
            result: Ok((0..720)
                .map(|i| PricePoint {
                    timestamp: i as f64 * 3_600_000.0,
                    price: 1000.0 + i as f64,
                })
                .collect()),
        });
        assert_eq!(
            app.detail_state().unwrap().chart_30d().len(),
            720,
            "chart series stored on Loading"
        );
        app.update(Event::DetailResult {
            id: "bitcoin".to_owned(),
            generation,
            result: Ok(Box::new(rich_detail())),
        });
        let state = app.detail_state().unwrap();
        assert_eq!(
            state.chart_30d().len(),
            720,
            "chart series survives the upgrade"
        );

        // A failed chart fetch leaves the pane on the 7-day fallback.
        let mut app = loaded_app(mixed_snapshot(vec![coin_row(
            "bitcoin", 1, "Bitcoin", "BTC",
        )]));
        let Command::FetchDetail { generation, .. } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))) else {
            panic!()
        };
        app.update(Event::DetailResult {
            id: "bitcoin".to_owned(),
            generation,
            result: Ok(Box::new(rich_detail())),
        });
        app.update(Event::ChartResult {
            id: "bitcoin".to_owned(),
            generation,
            result: Err(ApiError::Transport),
        });
        assert!(
            app.detail_state().unwrap().chart_30d().is_empty(),
            "failed chart fetch keeps the 7-day fallback"
        );
    }

    fn rich_detail() -> CoinDetail {
        CoinDetail::new(crate::domain::CoinDetailInput {
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
            market_cap: Some(1_000_000.0),
            volume_24h: Some(25_000.0),
            high_24h: Some(52_000.0),
            low_24h: Some(49_000.0),
            ath: Some(100_000.0),
            atl: Some(3_000.0),
            ath_change: Some(-50.0),
            atl_change: Some(1500.0),
            circulating_supply: Some(19.0),
            total_supply: Some(21.0),
            max_supply: Some(21.0),
            fully_diluted_valuation: Some(1_000_000.0),
            categories: vec!["layer-1".into(), "store-of-value".into()],
            sentiment_up: Some(70.0),
            sentiment_down: Some(30.0),
            sparkline_7d: vec![1.0, 2.0, 3.0],
            description: Some("A peer-to-peer network peak store of value.".into()),
        })
    }

    #[test]
    fn theme_cycles_forward_and_backward_and_wraps() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        assert_eq!(app.theme().name, "Default");
        for expected in ["Nord", "Tokyo Night", "Monochrome"] {
            assert!(is_render(app.update(Event::Input(key('t')))));
            assert_eq!(app.theme().name, expected);
        }
        assert!(is_render(app.update(Event::Input(key('t')))));
        assert_eq!(app.theme().name, "Default", "t wraps forward");
        let shift_t = KeyEvent::new(KeyCode::Char('T'), KeyModifiers::SHIFT);
        assert!(is_render(app.update(Event::Input(shift_t))));
        assert_eq!(app.theme().name, "Monochrome", "Shift-T steps backward");
        assert!(is_render(app.update(Event::Input(shift_t))));
        assert_eq!(app.theme().name, "Tokyo Night");
        assert!(is_render(app.update(Event::Input(shift_t))));
        assert_eq!(app.theme().name, "Nord");
        assert!(is_render(app.update(Event::Input(shift_t))));
        assert_eq!(app.theme().name, "Default", "Shift-T wraps backward");
    }

    #[test]
    fn theme_key_is_typed_text_while_searching() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        type_query(&mut app, "t");
        assert_eq!(app.search_buffer(), "t");
        assert_eq!(app.theme().name, "Default", "no theme change while typing");
    }

    #[test]
    fn theme_still_cycles_while_detail_is_open() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        enter(&mut app);
        assert!(app.detail_open());
        assert!(is_render(app.update(Event::Input(key('t')))));
        assert_eq!(app.theme().name, "Nord");
        assert!(
            app.detail_open(),
            "cycling a theme keeps the detail pane open"
        );
    }

    #[test]
    fn news_result_stores_items_and_failure_keeps_stale_items_with_notice() {
        let mut app = App::with_news_enabled(Duration::from_secs(60));
        // A stale generation (no fetch in flight) is ignored.
        assert!(matches!(
            app.update(Event::NewsResult {
                generation: 7,
                result: Ok(vec![NewsItem::fixture(
                    "headline",
                    "Wire",
                    "https://x.test/1"
                )]),
            }),
            Command::None
        ));
        assert!(app.news_feed().is_none());

        // Start a news fetch via a refresh, then deliver a success.
        let Command::Fetch {
            generation,
            news_generation,
        } = app.update(Event::Start)
        else {
            panic!("Start must request a refresh")
        };
        let news_generation = news_generation.expect("news is chained when enabled");
        assert!(is_render(app.update(Event::NewsResult {
            generation: news_generation,
            result: Ok(vec![NewsItem::fixture(
                "Bitcoin rises",
                "Wire",
                "https://x.test/1"
            )]),
        })));
        let feed = app.news_feed().unwrap();
        assert_eq!(feed.items.len(), 1);
        assert_eq!(feed.items[0].title(), "Bitcoin rises");
        assert!(feed.notice.is_none());

        // Complete the market refresh so a manual refresh is allowed, then
        // drive the news fetch into a failure.
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]))),
        });
        let Command::Fetch {
            news_generation, ..
        } = app.update(Event::Input(key('r')))
        else {
            panic!("manual refresh is allowed once the market fetch completes")
        };
        let news_generation = news_generation.expect("news is chained when enabled");
        assert!(is_render(app.update(Event::NewsResult {
            generation: news_generation,
            result: Err(ApiError::HttpStatus { status: 503 }),
        })));
        let feed = app.news_feed().unwrap();
        assert_eq!(feed.items.len(), 1, "stale headlines survive a failure");
        assert_eq!(feed.notice, Some(ApiError::HttpStatus { status: 503 }));
    }

    #[test]
    fn pane_keys_cycle_table_news_sentiment_and_are_swallowed_by_modals() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        assert_eq!(app.pane_focus(), MainPane::Table);
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(is_render(app.update(Event::Input(tab))));
        assert_eq!(app.pane_focus(), MainPane::News);
        assert!(is_render(app.update(Event::Input(tab))));
        assert_eq!(app.pane_focus(), MainPane::Sentiment);
        assert!(is_render(app.update(Event::Input(tab))));
        assert_eq!(app.pane_focus(), MainPane::Table, "Tab wraps forward");
        let shift_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
        assert!(is_render(app.update(Event::Input(shift_tab))));
        assert_eq!(
            app.pane_focus(),
            MainPane::Sentiment,
            "Shift-Tab wraps back"
        );
        let shift_tab_as_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert!(is_render(app.update(Event::Input(shift_tab_as_tab))));
        assert_eq!(app.pane_focus(), MainPane::News);

        // Tab is swallowed while searching, help is open, or detail is open.
        let mut searching = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        type_query(&mut searching, "a");
        assert!(matches!(searching.update(Event::Input(tab)), Command::None));
        assert_eq!(searching.pane_focus(), MainPane::Table);

        let mut help = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        help.update(Event::Input(key('?')));
        assert!(help.help_open());
        assert!(matches!(help.update(Event::Input(tab)), Command::None));

        let mut detail = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        enter(&mut detail);
        assert!(detail.detail_open());
        assert!(matches!(detail.update(Event::Input(tab)), Command::None));
    }

    #[test]
    fn closing_detail_invalidates_an_in_flight_fetch() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row(
            "bitcoin", 1, "Bitcoin", "BTC",
        )]));
        let Command::FetchDetail { id, generation } = app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))) else {
            panic!("Enter must open the detail and start the rich fetch")
        };
        assert_eq!(id, "bitcoin");
        assert!(app.detail_fetching());

        // Esc closes the pane and invalidates the fetch generation.
        assert!(is_render(app.update(Event::Input(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )))));
        assert!(!app.detail_open());
        assert!(!app.detail_fetching());

        // The stale result for the closed pane is ignored.
        assert!(matches!(
            app.update(Event::DetailResult {
                id: "bitcoin".to_owned(),
                generation,
                result: Ok(Box::new(rich_detail())),
            }),
            Command::None
        ));
        assert!(!app.detail_open());
    }

    #[test]
    fn news_fetch_chains_to_refresh_once_and_stays_disabled_by_default() {
        let mut app = loaded_app(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]));
        // News is disabled by default: no news generation is chained.
        let Command::Fetch {
            news_generation, ..
        } = app.update(Event::Start)
        else {
            panic!("Start must request a refresh")
        };
        assert!(news_generation.is_none(), "news stays disabled by default");
        assert!(matches!(app.update(Event::Input(key('r'))), Command::None));

        // With news enabled, a refresh chains one news fetch, and a second
        // refresh while it is in flight does not start a second news fetch.
        let mut enabled = App::with_news_enabled(Duration::from_secs(60));
        let Command::Fetch {
            generation,
            news_generation,
        } = enabled.update(Event::Start)
        else {
            panic!("Start must request a refresh")
        };
        let first_news = news_generation.expect("news is chained when enabled");
        assert!(
            matches!(enabled.update(Event::Input(key('r'))), Command::None),
            "a refresh is already in flight"
        );

        // Completing the market refresh while the news fetch is still in
        // flight: a manual refresh chains no second news fetch (one in flight).
        enabled.update(Event::FetchResult {
            generation,
            result: Ok(outcome(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]))),
        });
        let Command::Fetch {
            news_generation, ..
        } = enabled.update(Event::Input(key('r')))
        else {
            panic!("manual refresh is allowed once the fetch completes")
        };
        assert!(
            news_generation.is_none(),
            "the chained news fetch is still in flight, so no second news fetch starts"
        );

        // Once the first news result lands, the next refresh chains a fresh
        // generation.
        assert!(is_render(enabled.update(Event::NewsResult {
            generation: first_news,
            result: Ok(vec![NewsItem::fixture(
                "headline",
                "Wire",
                "https://x.test/1"
            )]),
        })));
        enabled.update(Event::FetchResult {
            generation: generation.wrapping_add(1),
            result: Ok(outcome(mixed_snapshot(vec![coin_row("a", 1, "A", "A")]))),
        });
        let Command::Fetch {
            news_generation, ..
        } = enabled.update(Event::Input(key('r')))
        else {
            panic!("manual refresh is allowed once the fetch completes")
        };
        assert!(
            news_generation.is_some_and(|next| next != first_news),
            "each completed news fetch chains a distinct generation"
        );

        // A stale news result (generation mismatch) is ignored.
        assert!(matches!(
            enabled.update(Event::NewsResult {
                generation: first_news,
                result: Ok(vec![NewsItem::fixture("old", "Wire", "https://x.test/1")]),
            }),
            Command::None
        ));
        assert_eq!(
            enabled.news_feed().unwrap().items[0].title(),
            "headline",
            "a stale news result never replaces the current feed"
        );
    }
}
