# Testing

## Purpose

Tests provide direct evidence for product behavior, architecture boundaries, roadmap acceptance criteria, and safe terminal operation. Automated tests never require internet access or a private API key.

## Baseline Checks

Run these checks for every implementation task after the crate exists:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Run a focused test first while iterating. Run all baseline checks before accepting a roadmap quality gate.

## Test Layers

### Pure Logic

Keep unit tests beside formatting, sorting, filtering, state transition, selection, retry, and sanitization logic. Prefer table-driven cases that name the observable boundary. Include empty input, missing values, ties, extremes, and invalid remote data where applicable.

### Terminal Rendering

Use Ratatui `TestBackend` for critical labels, visible columns, clipping, selection, status, and overlays. Avoid broad golden snapshots that fail on harmless spacing changes.

Required layout sizes:

| Size | Evidence |
| --- | --- |
| `59x15` | Resize message and active quit handling below the supported minimum. |
| `60x16` | Minimum compact layout. |
| `79x20` | Compact upper boundary. |
| `80x24` | Standard lower boundary. |
| `119x30` | Standard upper boundary. |
| `120x30` | Full lower boundary with sparkline. |

Every user-facing state in `PRODUCT.md` needs a rendering or state-transition test. Color assertions must also prove a text or symbol communicates the same meaning.

### HTTP Boundary

Tests must not call the live CoinGecko API. Use a local mock HTTP server and small sanitized JSON fixtures. Verify:

- required path, query parameters, user agent, and API-key header;
- complete, partial, empty, malformed, and oversized responses;
- timeout, connection failure, `429` with and without `Retry-After`, and `5xx`;
- API DTO conversion and finite-value normalization;
- optional summary failure with usable coin rows;
- secret redaction in errors and traces.

Fixtures contain no credentials, personal data, full production payloads, or unnecessary records. Keep one fixture per behavior class unless a smaller inline body is clearer.

### Async Runtime

Use paused Tokio time for refresh scheduling, cooldown, retry, and capped backoff. Prove that delayed HTTP work does not block input, only one refresh runs at a time, stale data survives refresh failure, and cancellation joins background tasks.

### Performance Measurement

Prove idle rendering is event-driven and record the idle CPU of a release run. Over a 60-second idle window the loop re-renders once per second at most (paused-time test `idle_rendering_is_event_driven_and_steady` counts ~30 renders per 30 seconds), and a release run sampled with `ps -o %cpu=` stays near zero. Use `scripts/measure-idle.sh` for the reproducible report: it builds `--release --locked`, runs against the loopback fixture server, samples CPU, and reads the traced `render ok` cadence and `refresh ok ... duration=` lines. Record a delayed-mock refresh timing run (`FIXTURE_DELAY_MS=250`), and record the observed idle CPU and refresh duration in the `ROADMAP.md` evidence as this task does.

### Terminal Lifecycle

Automate terminal setup logic where practical. Manually verify normal exit, `Ctrl-C`, startup error, runtime error, and panic. Each path must restore echo, cursor visibility, canonical input, and the previous screen.

## Manual Product Scenario

At each UI quality gate, run the keyboard-only path from `PRODUCT.md` with fixture-backed or live data. Also verify:

- resize across compact, standard, and full modes;
- readable output with truecolor and `NO_COLOR=1`;
- missing values, a flat sparkline, and long remote names;
- loading, stale, offline, rate-limited, empty, and fatal states;
- ten consecutive start, refresh, and quit cycles before release.

Running the gates is scripted: `scripts/measure-idle.sh` (idle CPU and traced
timing), `scripts/cycle-restore.sh` (ten start/refresh/quit cycles), and
`scripts/scenario-check.sh` (offline, DNS, timeout, malformed, `429`, and `500`
states), all against `scripts/fixture-server.py`. Record the terminal,
dimensions, data source, result, and defects in the relevant `ROADMAP.md`
evidence line.

## Release Suite

Before release acceptance, run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --locked
```

Run the manual product scenario in macOS Terminal, iTerm2, and one Linux terminal at compact, standard, and full widths. Record unsupported environments rather than implying support. A release has no critical or high-severity known defect and requires human approval before publication.

## Evidence

Report only checks that ran. Evidence includes the exact command or manual scenario, its result, and relevant measurements. Record blockers and residual risk when a check cannot run, including missing live-provider credentials.
