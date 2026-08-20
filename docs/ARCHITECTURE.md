# Architecture

`PRODUCT.md` owns product scope and observable behavior. This document owns the technical design used to deliver it.

## Runtime Model

Use a single state owner and message passing:

```text
Crossterm input --+
Tick / resize -----+--> Event channel --> update(App, Event) --> Ratatui view
Fetch task --------+                              |
                                                  +--> Command::Fetch
                                                           |
                                                           +--> Reqwest provider
```

The main loop uses `tokio::select!` or a unified Tokio channel to receive input, timer, resize, cancellation, and fetch-result events. It renders after state-changing events, not at a fixed high frame rate. A low-frequency tick updates relative timestamps and schedules refreshes.

There is at most one active market refresh. `Command::Fetch` starts a cancellable task and returns results through the event channel. Every controlled exit (quit, input closure, input error, or render error) cancels and joins the refresh task before terminal teardown. The top-level application future is not externally aborted in production; an abort-on-drop guard is emergency containment, not a joined shutdown path.

### State Machine

```rust
enum DataState {
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

struct FetchOutcome {
    snapshot: MarketSnapshot,
    summary_notice: Option<ApiError>,
}
```

Refreshing a ready, empty, or stale state keeps the snapshot visible and sets `fetching: true` separately. A transient fetch error after any successful response produces `Stale`; it does not erase the last snapshot, its freshness, or an existing optional-summary notice. A clean later summary replaces or clears that notice. Only failures with no successful snapshot can produce `Fatal`.

### Coin Detail Screen

`Enter` on a selected row stores a clone of the selected `CoinMarket` in `App.detail` as `DetailState::Basic`; `Esc` clears it. Opening and closing never touch `selected`, so the table's selection and viewport are preserved across the transition. While `detail` is set the input update is modal: navigation, search, and sort keys fall through to `Command::None`, while `r`, `?`, `q`, and `Esc` remain active. A successful refresh replaces the stored coin's base row with the matching-ID row from the new snapshot, so the pane stays fresh without holding a second query to the provider.

The detail screen follows the CoinMarketCap coin-detail shape: identity header, price with its 24-hour change, the 1h/24h/7d change strip, a candlestick chart, and a market-stats grid — all inside a left-aligned content column capped at `DETAIL_CONTENT_WIDTH = 56`, so the page never stretches with the terminal and the chart hugs the pane's left border. The chart is a `chandelier::CandlestickChart` (`render_detail_chart` in `ui.rs`) drawn from daily OHLC candles derived by `daily_candles` in `src/domain.rs`. The preferred series is the 30-day price history fetched on demand from `GET /coins/{id}/market_chart` (`fetch_market_chart` in `api.rs`), which yields up to `MAX_DAILY_CANDLES = 30` candles so the chart stretches the full content column; when that fetch is unsupported or fails, the chart falls back to the already-normalized hourly `sparkline_7d` series bucketed one candle per 24 hourly points. Hostile, empty, overflowing, or gigantic series stay bounded and cannot panic or overflow the renderer; a flat or tiny series collapses to a single flat candle that renders as a mid-line, and an all-missing series shows the placeholder message. The chart's bull/bear/wick styles use the `theme.gain`/`theme.loss`/`trend_color` roles so it recolors with every theme and stays readable without color (`Monochrome` maps all roles to `Color::Reset`). The chart draws its own autoscaling price axis and time axis, right-aligning the newest candles; a `7 days` or `30 days: low → high` caption line below uses the real series low/high and the summary accent.

#### Rich Detail Sidebar

On top of the row-backed base, `Enter` starts an on-demand `GET /coins/{id}` fetch (`CoinGeckoClient::fetch_coin_detail` in `api.rs`, id percent-encoded via `coin_detail_url` so a hostile id cannot smuggle path segments). The `DetailState` transitions `Basic → Loading → Ready`, where `Loading` carries the base row and `Ready` carries the base row plus the rich `Box<CoinDetail>` (`Event::DetailResult` boxes the detail the same way to keep the event enum small). A failed fetch or a provider without detail support leaves the pane on `Basic`; a stale generation is ignored. Wide detail panes (`DETAIL_SIDEBAR_MIN_WIDTH = 78` inner columns) split into the main column plus a `Coin data` sidebar (`DETAIL_SIDEBAR_WIDTH = 36`) with ATH/ATL plus change, supplies, fully diluted valuation, longer-period changes, sentiment votes, categories, and a bounded About snippet; narrow panes stack the same values as two compact stat lines under the chart. The chart prefers the on-demand 30-day `market_chart` series (`Event::ChartResult`, carried through `Loading`/`Ready` so either fetch may land first), then the rich detail's dense hourly `sparkline_7d` series, and finally the row series.

#### News And Pane Layout

`Tab`/`Shift-Tab` cycle `MainPane` (`Table`, `News`, `Sentiment`). Below `PANE_MIN_WIDTH = 162` the focused pane renders alone in the body so the table keeps its full column set; at 162+ the body splits horizontally 70/30 (table 70%, a right column 30%) and the right column splits vertically into two equal rows holding the news pane (top) and the sentiment pane (bottom), with focus only emphasizing the active pane's title. Pane keys are swallowed while searching, while help is open, or on the detail screen.

The news feed is a separate `NewsProvider` boundary (`src/news.rs`): `RssNewsClient` fetches the configured RSS URL with the same URL validation, no-redirect client, bounded 1 MiB body, and timeout rules as the market provider, and `parse_rss` normalizes items into bounded `NewsItem` values (title ≤ 220 scalars, source ≤ 28, url ≤ 300, control characters stripped, RFC-2822 dates parsed to UTC). An HTML error page cannot masquerade as an empty feed: a missing `<rss>`/`<feed>` root is `MalformedResponse`. The news fetch is chained onto a market refresh (`Command::Fetch { news_generation }`), one in flight at a time, generation-guarded like the market fetch, and spawned as its own cancellable task so a slow feed never blocks input, the market refresh, or shutdown. A failed news refresh preserves the last headlines and records the notice; the `NewsFeed` state is the newest items plus an optional `ApiError`.

The sentiment pane is a pure render of the current snapshot: up/down/flat counts and a bullish meter over the finite 24-hour changes, plus average, best, and worst mover. It adds no provider call.

### Themes

`Theme` (`theme.rs`) defines a small set of semantic color roles (`summary`, `notice`, `gain`, `loss`, `neutral`) plus a display name. `THEMES` lists the built-ins in cycle order (`Default`, `Nord`, `Tokyo Night`, `Monochrome`); the first entry is the startup default. `App` owns only a `theme_index: usize`, exposed as `App::theme` and mutated by `App::cycle_theme` (`t` and `Shift-T` guards in `update`, active on the table and on the detail screen and swallowed while typing or when help is open). Every style site in `ui.rs` reads a role from the active theme and nothing else, so switching themes is a pure re-render. The theme surfaces the roles across: the summary line's labels (summary accent, with the market-cap change colored by sign in `summary_line`), the table header and the selected-row highlight (summary role, applied as the reversed foreground so the selection background carries the role), the detail pane (notice-colored title, summary-colored rank chip and chart labels), the message/no-results/help screens (notice role, with themed block titles), and the detail candlestick chart's bull/bear/wick colors. The trend surfaces reuse the gain/loss/neutral roles: the table's 7-day sparkline cell (`CellKind::Trend`) is tinted by the 7-day change sign via `cell_style`, and the detail chart's candles use `trend_color` for the wick, so the chart carries the change color with no second palette. Table rows are rendered at `Row::height(2)`, so each row's blank breathing line is part of the selected-row highlight block (the highlight fills both lines edge to edge). Layout, columns, and state transitions never read the theme: `Monochrome` maps every role to `Color::Reset`, and the rendering tests prove each theme draws at every supported width with no out-of-bounds output and no dependence on color for meaning.

## Module Map

Start with this structure and split further only when a file becomes difficult to navigate:

```text
src/
|-- main.rs          # process setup, error reporting, terminal lifetime
|-- app.rs           # App, Event, Command, update, selection, sorting, panes
|-- tui.rs           # terminal enter/restore and Crossterm event stream
|-- api.rs           # MarketData trait, CoinGecko client and DTO conversion
|-- domain.rs        # provider-independent market and detail types
|-- news.rs          # NewsProvider trait, RSS client and bounded parsing
|-- format.rs        # deterministic money, percentage and supply formatting
|-- log.rs           # redacted file tracing
|-- theme.rs         # built-in color themes as semantic roles
`-- ui.rs            # responsive layout and Ratatui rendering
tests/
|-- fixtures/        # small, sanitized provider responses
`-- provider.rs      # HTTP-boundary tests with a local mock server
```

Test placement and verification policy are defined in `TESTING.md`.

## Domain Model

```rust
struct MarketSnapshot {
    summary: MarketSummary,
    coins: Vec<CoinMarket>,
    provider_updated_at: Option<DateTime<Utc>>,
}

struct MarketSummary {
    total_market_cap: Option<f64>,
    total_volume_24h: Option<f64>,
    btc_dominance: Option<f64>,
    market_cap_change_24h: Option<f64>,
}

struct CoinMarket {
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
```

Use `f64` only for display and sorting of provider-supplied market statistics. Normalize non-finite values to `None` at the boundary. If the product later performs accounting or order calculations, introduce a decimal type for that separate domain.

## Data Provider

Use CoinGecko's Demo API as the first provider:

- `GET /api/v3/coins/markets`
- `vs_currency=usd`
- `order=market_cap_desc`
- `per_page=100`
- `page=1`
- `sparkline=true`
- `price_change_percentage=1h,24h,7d`

Use the global endpoint only when needed for summary metrics. Fetch independent endpoints concurrently, then publish one complete snapshot. `MarketData::fetch_snapshot` returns `Result<FetchOutcome, ApiError>`: a successful coin request publishes usable rows even when the optional summary request fails, with that `ApiError` in `summary_notice`; a clean summary has no notice. A failed coin request remains the outer error and fails the refresh. The application stores the optional notice in `Ready` or `Empty` and clears it on the next clean success; it does not schedule retries or cooldowns here.

`MarketData::fetch_coin_detail` fetches the rich `/coins/{id}` payload on demand for the detail sidebar. The trait default returns `501 Not Implemented`, so providers that only serve the market snapshot leave the detail pane on its row-backed fallback; `CoinGeckoClient` overrides it. Detail values are normalized like market values at the boundary, and out-of-range or non-JSON responses are rejected as `MalformedResponse`.

Configuration comes from environment variables and CLI flags:

| Setting | Default | Purpose |
| --- | --- | --- |
| `COIN_TUI_API_KEY` | unset | CoinGecko Demo API key, sent as `x-cg-demo-api-key`. |
| `COIN_TUI_BASE_URL` | CoinGecko Demo URL | Test or alternate compatible endpoint. |
| `COIN_TUI_NEWS_URL` | CoinDesk RSS | News headline feed; same HTTPS/loopback rules as the base URL. |
| `COIN_TUI_LOG_FILE` | unset | Append redacted diagnostics to this file; logs stay off the alternate screen. |
| `--refresh-seconds` | `60` | Automatic refresh interval, minimum 15 seconds. |
| `--currency` | `usd` | Quote currency; MVP accepts USD only and validates explicitly. |

Never log the key or include it in an error. Configure one reusable `reqwest::Client` with a user agent, connect timeout, total request timeout, gzip support, and rustls. Respect HTTP `429`; display a concise rate-limit state and delay automatic retries using `Retry-After` when present. Otherwise use capped exponential backoff with jitter for transient transport and `5xx` failures. Manual refresh does not bypass an active cooldown.

### Refresh Scheduling

A `RefreshScheduler` in `App` owns the only refresh cadence. A success marks `next_auto_at = now + interval` (default 60 seconds) and clears cooldown; a failure opens a cooldown window (`next_auto_at = now + delay`) during which neither the automatic tick nor a manual `r` may start a refresh. The delay is the exact `Retry-After` (floored at one second) when a `429` provides one, capped equal-jittered exponential backoff (`[scaled/2, scaled]`, `scaled = min(2s · 2^failures, 60s)`) for transient transport, timeout, `5xx`, and bare-`429` failures, and the steady 60-second gate for other errors. A manual refresh is allowed again as soon as the window passes; a success resets failures, cooldown, and the interval. The main loop emits a low-frequency `Tick` (one second, first tick delayed) that re-renders relative timestamps and lets the scheduler start automatic refreshes; the scheduler never starts a second fetch while one is in flight.


### Redacted File Tracing

When `COIN_TUI_LOG_FILE` is set, a shared `FileLog` (`src/log.rs`) appends timestamped diagnostic lines to that file. It is written outside the alternate screen and is always best-effort: a poison-ed lock or failed write never takes the application down. The API key is registered as a redaction secret, and every line is scrubbed before a single byte reaches disk, so even an accidental caption cannot spill it. The refresh lifecycle emits compact events: `session start`, `refresh start generation=N`, `refresh ok generation=N coins=M duration=NNNms`, `refresh failed generation=N duration=NNNms error=<redacted>`, a `render ok duration=NNNms` line per draw, and `loop stopped success=true`. Durations make idle and refresh behavior observable from the trace, so the trace doubles as the performance record for `M4-04`. Errors use `ApiError`'s redacting `Display`, which never echoes a response body, so hostile or oversized bodies cannot leak their contents into the file.

### Performance Measurement

The idle loop is event-driven: it parks on `tokio::select!` over input, fetch results, and a one-second tick, and re-renders only when an event arrives; it never draws at a fixed high frame rate. `scripts/fixture-server.py` is a loopback CoinGecko-compatible mock serving small sanitized JSON for the market and rich-detail endpoints plus an RSS feed for offline manual runs, and `scripts/measure-idle.sh` samples the idle CPU of a release run and reports the traced render/refresh cadence. Nothing in `scripts/` is part of the application build; it is a measurement harness only. `TESTING.md` defines the measurement procedure.

Provider verification and fixture rules are defined in `TESTING.md`.

## Dependencies

Current production dependencies:

| Crate | Purpose |
| --- | --- |
| `ratatui` | Immediate-mode terminal rendering and `TestBackend` (v0.30, with the `crossterm_0_29` feature so its re-exported Crossterm matches our direct dependency). |
| `crossterm` | Portable terminal setup and input events (v0.29, `event-stream`). |
| `chandelier` | Ratatui candlestick chart widget for the coin-detail chart; renders OHLC bars with an autoscaling price axis and a time axis. |
| `tokio` | Async runtime, channels, timers, tasks, and cancellation. |
| `tokio-util` | `CancellationToken` for coordinated shutdown. |
| `futures-util` | Async Crossterm event-stream polling. |
| `reqwest` | HTTPS client with JSON and rustls features. |
| `serde`, `serde_json` | Provider response decoding and fixtures. |
| `chrono` | Provider timestamps and display formatting. |
| `httpdate` | HTTP-date form of provider `Retry-After` headers. |
| `url` | Parsed host validation for HTTPS and loopback-only HTTP base URLs. |
| `unicode-width` | Measure terminal display-cell width when sanitizing and bounding remote text and compact rows. |
| `clap` | Typed CLI flags and environment integration. |
| `quick-xml` | Streaming RSS 2.0 feed parsing in `src/news.rs` (default features off, no network I/O). |

Planned production dependencies are added only with the roadmap task that uses them:

| Crate | Planned purpose |
| --- | --- |
| `color-eyre` | Error reports while preserving terminal cleanup. |

Redacted file tracing intentionally uses no `tracing`/`tracing-subscriber`; the self-contained `FileLog` in `src/log.rs` keeps the dependency set smallest while meeting the tracing acceptance.

Expected development dependencies are `wiremock` for HTTP-boundary tests and `tempfile` for isolated files; both are in use. The `tokio` `test-util` feature (dev-only) enables paused-time tests for refresh scheduling. Add `insta`, `proptest`, Axum, or a database only when a roadmap task demonstrates a concrete need.

Pin the Rust toolchain in `rust-toolchain.toml`. Let `Cargo.lock` pin crate versions for reproducible application builds; do not hard-code versions in this specification.

## Resilience And Security

- Bound response bodies and accept only JSON from successful HTTP responses.
- Clamp visible remote text to available cell width and strip control characters.
- Use HTTPS by default. Permit an HTTP base URL only for localhost tests.
- Avoid shell execution, HTML rendering, and opening remote URLs.
- Redact headers and query values from diagnostics.
- Keep the last successful in-memory snapshot only. Disk caching is a later decision with an explicit freshness and privacy policy.
- Use saturating index arithmetic and handle zero rows without panics.
- Install a panic hook that restores the previous hook after terminal teardown.

## Observability

Tracing records startup, refresh duration, result class, row count, retry delay, and shutdown. It excludes API keys, complete payloads, and normal keystrokes. `PRODUCT.md` defines user-visible status behavior.

## References

- CoinGecko coins market endpoint: <https://docs.coingecko.com/reference/coins-markets>
- CoinGecko API introduction and authentication: <https://docs.coingecko.com/>
- Ratatui async event stream: <https://ratatui.rs/tutorials/counter-async-app/async-event-stream/>
- Ratatui full async events: <https://ratatui.rs/tutorials/counter-async-app/full-async-events/>
