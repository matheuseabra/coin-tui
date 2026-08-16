use std::{io, sync::Arc, time::Instant};

use crossterm::event::{
    Event as CrosstermEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures_util::{Stream, StreamExt};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    api::{ApiError, CoinGeckoClient, FetchOutcome, MarketData},
    domain::MarketSnapshot,
    tui, ui,
};

pub enum Event {
    Start,
    Input(KeyEvent),
    Resize,
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
}

impl App {
    pub fn new() -> Self {
        Self {
            state: DataState::Initial,
            generation: 0,
            fetching: false,
        }
    }

    /// The only state transition function. It performs no I/O and is deterministic.
    pub fn update(&mut self, event: Event) -> Command {
        match event {
            Event::Start => self.request_refresh(),
            Event::Input(key) if should_quit(key) => Command::Quit,
            Event::Input(key)
                if key.kind == KeyEventKind::Press
                    && key.code == KeyCode::Char('r')
                    && key.modifiers.is_empty() =>
            {
                self.request_refresh()
            }
            Event::Resize => Command::Render,
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
                Command::Render
            }
            Event::Input(_) => Command::None,
        }
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
    #[cfg(test)]
    pub fn snapshot(&self) -> Option<&MarketSnapshot> {
        match &self.state {
            DataState::Ready { snapshot, .. }
            | DataState::Empty { snapshot, .. }
            | DataState::Stale { snapshot, .. } => Some(snapshot),
            DataState::Initial | DataState::Loading | DataState::Fatal(_) => None,
        }
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
                    Some(Ok(CrosstermEvent::Resize(_, _))) => Event::Resize,
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

fn should_quit(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (matches!(key.code, KeyCode::Char('q'))
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)))
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
        assert!(matches!(app.update(Event::Resize), Command::Render));
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
}
