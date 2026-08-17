use std::{cmp::Ordering, io, sync::Arc, time::Instant};

use crossterm::event::{
    Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures_util::{Stream, StreamExt};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    api::{ApiError, CoinGeckoClient, FetchOutcome, MarketData},
    domain::{CoinMarket, MarketSnapshot},
    tui, ui,
};

pub enum Event {
    Start,
    Input(KeyEvent),
    Resize {
        height: u16,
    },
    FetchResult {
        generation: u64,
        result: Result<FetchOutcome, ApiError>,
    },
}

pub enum Command {
    Quit,
    Render,
    Fetch { generation: u64 },
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

pub struct App {
    state: DataState,
    generation: u64,
    fetching: bool,
    selected: usize,
    viewport_rows: usize,
    search: SearchState,
    sort: SortState,
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

impl App {
    pub fn new() -> Self {
        Self {
            state: DataState::Initial,
            generation: 0,
            fetching: false,
            selected: 0,
            viewport_rows: 16,
            search: SearchState::default(),
            sort: SortState::default(),
        }
    }

    /// The only state transition function. It performs no I/O and is deterministic.
    pub fn update(&mut self, event: Event) -> Command {
        match event {
            Event::Start => self.request_refresh(),
            Event::Input(key) if should_quit(key, self.search.typing) => Command::Quit,
            Event::Input(key) if self.search.typing => {
                if self.search_input(key) {
                    Command::Render
                } else {
                    Command::None
                }
            }
            Event::Input(key) if clear_active_search(key) && !self.search.query.is_empty() => {
                self.search.query.clear();
                Command::Render
            }
            Event::Input(key) if is_search_start(key) => {
                self.search.typing = true;
                self.search.buffer.clear();
                Command::Render
            }
            Event::Input(key) if is_sort_forward(key) => {
                self.cycle_sort(true);
                Command::Render
            }
            Event::Input(key) if is_sort_backward(key) => {
                self.cycle_sort(false);
                Command::Render
            }
            Event::Input(key) if is_refresh(key) => self.request_refresh(),
            Event::Input(key) if navigation_key(key.code) => {
                self.navigate(key.code);
                Command::Render
            }
            Event::Resize { height, .. } => {
                self.viewport_rows = table_viewport(height);
                Command::Render
            }
            Event::FetchResult { generation, result } => {
                if !self.fetching || generation != self.generation {
                    return Command::None;
                }
                self.fetching = false;
                let refreshed_at = Instant::now();
                self.state = match result {
                    Ok(outcome) if outcome.snapshot.coins().is_empty() => DataState::Empty {
                        snapshot: outcome.snapshot,
                        refreshed_at,
                        notice: outcome.summary_notice,
                    },
                    Ok(outcome) => DataState::Ready {
                        snapshot: outcome.snapshot,
                        refreshed_at,
                        notice: outcome.summary_notice,
                    },
                    Err(error) => match self.state.clone() {
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
                    },
                };
                self.clamp_selection();
                Command::Render
            }
            Event::Input(_) => Command::None,
        }
    }

    fn navigate(&mut self, code: KeyCode) {
        let count = self.row_count();
        if count == 0 {
            return;
        }
        let last = count - 1;
        self.selected = match code {
            KeyCode::PageDown => self.selected.saturating_add(self.viewport_rows).min(last),
            KeyCode::Down => self.selected.saturating_add(1).min(last),
            KeyCode::PageUp => self.selected.saturating_sub(self.viewport_rows),
            KeyCode::Up => self.selected.saturating_sub(1),
            KeyCode::Home => 0,
            KeyCode::End => last,
            KeyCode::Char('g') => 0,
            KeyCode::Char('G') => last,
            KeyCode::Char('j') => self.selected.saturating_add(1).min(last),
            KeyCode::Char('k') => self.selected.saturating_sub(1),
            _ => self.selected,
        };
    }

    fn request_refresh(&mut self) -> Command {
        if self.fetching {
            return Command::None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.fetching = true;
        if matches!(self.state, DataState::Initial) {
            self.state = DataState::Loading;
        }
        Command::Fetch {
            generation: self.generation,
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
    events: mpsc::Sender<Event>,
    results: mpsc::Receiver<Event>,
    cancellation: CancellationToken,
    active: Option<JoinHandle<()>>,
    pub app: App,
}

impl<P: MarketData + ?Sized + 'static> Controller<P> {
    pub fn new(provider: Arc<P>) -> Self {
        let (events, results) = mpsc::channel(16);
        Self {
            provider,
            events,
            results,
            cancellation: CancellationToken::new(),
            active: None,
            app: App::new(),
        }
    }

    pub fn start_initial_refresh(&mut self) {
        self.dispatch(Event::Start);
    }

    fn dispatch(&mut self, event: Event) {
        if let Command::Fetch { generation } = self.app.update(event) {
            self.start_fetch(generation);
        }
    }

    fn start_fetch(&mut self, generation: u64) {
        let provider = Arc::clone(&self.provider);
        let sender = self.events.clone();
        let cancelled = self.cancellation.clone();
        self.active = Some(tokio::spawn(async move {
            tokio::select! {
                result = provider.fetch_snapshot() => { let _ = sender.send(Event::FetchResult { generation, result }).await; }
                _ = cancelled.cancelled() => {}
            }
        }));
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        self.results.recv().await
    }

    pub async fn handle(&mut self, event: Event) -> Command {
        let command = self.app.update(event);
        if let Command::Fetch { generation } = command {
            self.start_fetch(generation);
        }
        command
    }

    pub async fn shutdown(&mut self) -> io::Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.active.take() {
            task.await.map_err(|error| {
                io::Error::other(format!("background task failed during shutdown: {error}"))
            })?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn has_active_task(&self) -> bool {
        self.active.is_some()
    }
}

impl<P: MarketData + ?Sized> Drop for Controller<P> {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.active.take() {
            task.abort();
        }
    }
}

pub async fn run() -> io::Result<()> {
    let provider = configured_provider()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut session = tui::enter()?;
    let result = run_loop(session.terminal_mut(), Arc::new(provider)).await;
    let restore_result = session.restore();
    result.and(restore_result)
}

fn configured_provider() -> Result<CoinGeckoClient, String> {
    let base = match std::env::var("COIN_TUI_BASE_URL") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => "https://api.coingecko.com/".to_owned(),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("COIN_TUI_BASE_URL is not valid Unicode".into())
        }
    };
    let key = match std::env::var("COIN_TUI_API_KEY") {
        Ok(value) if !value.is_empty() => Some(value),
        Ok(_) | Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("COIN_TUI_API_KEY is not valid Unicode".into())
        }
    };
    CoinGeckoClient::new(&base, key).map_err(|error| error.to_string())
}

async fn run_loop<P: MarketData + 'static>(
    terminal: &mut tui::AppTerminal,
    provider: Arc<P>,
) -> io::Result<()> {
    let mut draw = |app: &App| terminal.draw(|frame| ui::render(frame, app)).map(|_| ());
    run_loop_with_sources(provider, EventStream::new(), &mut draw).await
}

/// The controller loop is kept independent of the terminal so lifecycle and
/// concurrency behavior can be tested with deterministic event streams.
async fn run_loop_with_sources<P, S, R>(
    provider: Arc<P>,
    mut input: S,
    mut render: R,
) -> io::Result<()>
where
    P: MarketData + 'static,
    S: Stream<Item = Result<CrosstermEvent, io::Error>> + Unpin,
    R: FnMut(&App) -> io::Result<()>,
{
    let mut controller = Controller::new(provider);
    let loop_result = async {
        controller.start_initial_refresh();
        render(&controller.app)?;
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
            };
            match controller.handle(event).await {
                Command::Quit => {
                    return Ok(());
                }
                Command::Render => {
                    render(&controller.app)?;
                }
                Command::Fetch { .. } => {
                    render(&controller.app)?;
                }
                Command::None => {}
            }
        }
    };
    let loop_result = loop_result.await;
    let shutdown_result = controller.shutdown().await;
    match loop_result {
        Err(error) => Err(error),
        Ok(()) => shutdown_result,
    }
}

/// `q` and `Ctrl-C` quit, but while search editing is open `q` is a printable
/// query character. The hard `Ctrl-C` exit always wins so a stuck search can
/// never trap the user.
fn should_quit(key: KeyEvent, typing: bool) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
            || (key.code == KeyCode::Char('q') && !typing))
}

fn is_search_start(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('/') && key.modifiers.is_empty()
}

/// `s` advances the sort cycle; `Shift-S` moves backward. Terminals report the
/// shifted char as `S` (with or without the SHIFT modifier) or as `s` plus the
/// modifier, so both spellings are accepted.
fn is_sort_forward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('s') && key.modifiers.is_empty()
}

fn is_sort_backward(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char('S')
            || (key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::SHIFT)))
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

/// `Esc` cancels search editing (handled by `search_input`) or clears a
/// committed filter when search is idle.
fn clear_active_search(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Esc && key.modifiers.is_empty()
}

fn is_refresh(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && key.code == KeyCode::Char('r') && key.modifiers.is_empty()
}

fn navigation_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Down
            | KeyCode::Up
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Char('j')
            | KeyCode::Char('k')
            | KeyCode::Char('g')
            | KeyCode::Char('G')
    )
}

/// Rows a table can show below its header for a given terminal height: the
/// summary block (3), status line (1), and the bordered table header (2 + 1).
/// PageUp/PageDown move by this viewport.
fn table_viewport(height: u16) -> usize {
    height.saturating_sub(7).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CoinMarketInput, MarketSummaryInput};
    use futures_util::stream;
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

    #[test]
    fn pure_update_suppresses_duplicate_refresh_and_stale_results() {
        let mut app = App::new();
        let Command::Fetch { generation } = app.update(Event::Start) else {
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
        let Command::Fetch { generation } = app.update(Event::Input(key('r'))) else {
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
        let Command::Fetch { generation } = app.update(Event::Start) else {
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
        let Command::Fetch { generation } = failed.update(Event::Start) else {
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
        let Command::Fetch { generation } = app.update(Event::Start) else {
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
        let Command::Fetch { generation } = many_app.update(Event::Start) else {
            panic!()
        };
        many_app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(many)),
        });
        many_app.select(4);
        assert_eq!(many_app.selected(), 4);
        let Command::Fetch { generation } = many_app.update(Event::Input(key('r'))) else {
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
        let Command::Fetch { generation } = app.update(Event::Start) else {
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
        let viewport = table_viewport(24);
        assert_eq!(viewport, 17, "24-row terminal shows 17 table rows");
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
        assert_eq!(table_viewport(0), 1);
        assert_eq!(table_viewport(5), 1);
        assert_eq!(table_viewport(60), 53);
        let mut app = ready_app(10);
        app.update(Event::Resize { height: 30 });
        assert_eq!(table_viewport(30), 23);
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
        let dropped = Arc::clone(&provider.dropped);
        let eof = run_loop_with_sources(provider, stream::empty(), |_| Ok(())).await;
        assert!(eof.is_ok());
        assert!(dropped.load(Ordering::Acquire));

        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let dropped = Arc::clone(&provider.dropped);
        let error = run_loop_with_sources(provider, stream::empty(), |_| {
            Err(io::Error::other("draw failed"))
        })
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "draw failed");
        assert!(dropped.load(Ordering::Acquire));

        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let dropped = Arc::clone(&provider.dropped);
        let input_error = run_loop_with_sources(
            provider,
            stream::iter([Err(io::Error::other("input failed"))]),
            |_| Ok(()),
        )
        .await
        .unwrap_err();
        assert_eq!(input_error.to_string(), "input failed");
        assert!(dropped.load(Ordering::Acquire));

        let provider = Arc::new(PendingProvider {
            calls: AtomicUsize::new(0),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            first_polled: Arc::new(Notify::new()),
        });
        let dropped = Arc::clone(&provider.dropped);
        let later_draw = run_loop_with_sources(
            provider,
            stream::iter([Ok(CrosstermEvent::Resize(80, 24))]),
            |_| Err(io::Error::other("later draw failed")),
        )
        .await
        .unwrap_err();
        assert_eq!(later_draw.to_string(), "later draw failed");
        assert!(dropped.load(Ordering::Acquire));
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
        let Command::Fetch { generation } = app.update(Event::Start) else {
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
        let Command::Fetch { generation } = app.update(Event::Input(key('r'))) else {
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

    #[test]
    fn summary_notice_keeps_rows_and_clears_on_next_clean_success() {
        let mut app = App::new();
        let Command::Fetch { generation } = app.update(Event::Start) else {
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

        let Command::Fetch { generation } = app.update(Event::Input(key('r'))) else {
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

        let Command::Fetch { generation } = app.update(Event::Input(key('r'))) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(snapshot())),
        });
        assert!(matches!(app.state(), DataState::Ready { notice: None, .. }));
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
        let Command::Fetch { generation } = app.update(Event::Start) else {
            panic!()
        };
        app.update(Event::FetchResult {
            generation,
            result: Ok(outcome(empty.clone())),
        });
        let DataState::Empty { refreshed_at, .. } = app.state() else {
            panic!()
        };
        let Command::Fetch { generation } = app.update(Event::Input(key('r'))) else {
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
        let Command::Fetch { generation } = app.update(Event::Start) else {
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

    fn enter(app: &mut App) {
        assert!(is_render(app.update(Event::Input(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )))));
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

        let Command::Fetch { generation } = app.update(Event::Input(key('r'))) else {
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
}
