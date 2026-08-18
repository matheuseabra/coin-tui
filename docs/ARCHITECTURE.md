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

## Module Map

Start with this structure and split further only when a file becomes difficult to navigate:

```text
src/
|-- main.rs          # process setup, error reporting, terminal lifetime
|-- app.rs           # App, Event, Command, update, selection and sorting
|-- tui.rs           # terminal enter/restore and Crossterm event stream
|-- api.rs           # MarketData trait, CoinGecko client and DTO conversion
|-- domain.rs        # provider-independent market types
|-- format.rs        # deterministic money, percentage and supply formatting
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

Configuration comes from environment variables and CLI flags:

| Setting | Default | Purpose |
| --- | --- | --- |
| `COIN_TUI_API_KEY` | unset | CoinGecko Demo API key, sent as `x-cg-demo-api-key`. |
| `COIN_TUI_BASE_URL` | CoinGecko Demo URL | Test or alternate compatible endpoint. |
| `COIN_TUI_LOG_FILE` | unset | Append redacted diagnostics to this file; logs stay off the alternate screen. |
| `--refresh-seconds` | `60` | Automatic refresh interval, minimum 15 seconds. |
| `--currency` | `usd` | Quote currency; MVP accepts USD only and validates explicitly. |

Never log the key or include it in an error. Configure one reusable `reqwest::Client` with a user agent, connect timeout, total request timeout, gzip support, and rustls. Respect HTTP `429`; display a concise rate-limit state and delay automatic retries using `Retry-After` when present. Otherwise use capped exponential backoff with jitter for transient transport and `5xx` failures. Manual refresh does not bypass an active cooldown.

### Refresh Scheduling

A `RefreshScheduler` in `App` owns the only refresh cadence. A success marks `next_auto_at = now + interval` (default 60 seconds) and clears cooldown; a failure opens a cooldown window (`next_auto_at = now + delay`) during which neither the automatic tick nor a manual `r` may start a refresh. The delay is the exact `Retry-After` (floored at one second) when a `429` provides one, capped equal-jittered exponential backoff (`[scaled/2, scaled]`, `scaled = min(2s · 2^failures, 60s)`) for transient transport, timeout, `5xx`, and bare-`429` failures, and the steady 60-second gate for other errors. A manual refresh is allowed again as soon as the window passes; a success resets failures, cooldown, and the interval. The main loop emits a low-frequency `Tick` (one second, first tick delayed) that re-renders relative timestamps and lets the scheduler start automatic refreshes; the scheduler never starts a second fetch while one is in flight.


### Redacted File Tracing

When `COIN_TUI_LOG_FILE` is set, a shared `FileLog` (`src/log.rs`) appends timestamped diagnostic lines to that file. It is written outside the alternate screen and is always best-effort: a poison-ed lock or failed write never takes the application down. The API key is registered as a redaction secret, and every line is scrubbed before a single byte reaches disk, so even an accidental caption cannot spill it. The refresh lifecycle emits compact events: `session start`, `refresh start generation=N`, `refresh ok generation=N coins=M duration=NNNms`, `refresh failed generation=N duration=NNNms error=<redacted>`, a `render ok duration=NNNms` line per draw, and `loop stopped success=true`. Durations make idle and refresh behavior observable from the trace, so the trace doubles as the performance record for `M4-04`. Errors use `ApiError`'s redacting `Display`, which never echoes a response body, so hostile or oversized bodies cannot leak their contents into the file.

### Performance Measurement

The idle loop is event-driven: it parks on `tokio::select!` over input, fetch results, and a one-second tick, and re-renders only when an event arrives; it never draws at a fixed high frame rate. `scripts/fixture-server.py` is a loopback CoinGecko-compatible mock serving small sanitized JSON for offline manual runs, and `scripts/measure-idle.sh` samples the idle CPU of a release run and reports the traced render/refresh cadence. Nothing in `scripts/` is part of the application build; it is a measurement harness only. `TESTING.md` defines the measurement procedure.

Provider verification and fixture rules are defined in `TESTING.md`.

## Dependencies

Current production dependencies:

| Crate | Purpose |
| --- | --- |
| `ratatui` | Immediate-mode terminal rendering and `TestBackend`. |
| `crossterm` | Portable terminal setup and input events. |
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
