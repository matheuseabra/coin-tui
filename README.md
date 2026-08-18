# 🪙 Coin TUI

A fast, read-only cryptocurrency market dashboard for the terminal, built with Rust, Ratatui, Tokio, and Reqwest.

It shows a compact global market summary and up to 100 coins ranked by market capitalization: price, 1-hour / 24-hour / 7-day changes, market cap, 24-hour volume, circulating supply, and a 7-day sparkline. Data comes from CoinGecko's public API over HTTPS.

## Features

- Global market summary: market cap, 24-hour volume, BTC dominance, 24-hour change
- 100 coins by market cap with price, changes, cap, volume, supply, and sparkline
- Keyboard navigation, stable sorting, case-insensitive search, manual refresh
- Responsive layouts (compact, standard, full) and an explicit status for every state
- USD-only quotes; works with or without color and at low resolution
- Configurable via flags or environment variables; zero secrets outside your terminal

## Screenshots

Full layout (120×30), against the offline fixture server so the demo data is deterministic:

```text
┌ Market summary ────────────────────────────────────────────────┐
│Cap: $2T | Vol 24h: $80.0B | BTC dom: 54.00% | Mkt 24h: -0.40%   │
└────────────────────────────────────────────────────────────────┘
┌Market | Live───────────────────────────────────────────────────┐
│  # Coin            Sym          Price       1h      24h       7d│
│  1 Fixture Coin 1  FC1          $100K   +0.10%   -0.05%   +0.01%│
│  2 Fixture Coin 2  FC2         $50.0K   +0.20%   -0.10%   +0.02%│
│  3 Fixture Coin 3  FC3         $33.3K   +0.30%   -0.15%   +0.03%│
│  4 Fixture Coin 4  FC4         $25.0K   +0.40%   -0.20%   +0.04%│
│  5 Fixture Coin 5  FC5         $20.0K   +0.50%   -0.25%   +0.05%│
│  6 Fixture Coin 6  FC6         $16.7K   +0.60%   -0.30%   +0.06%│
│  7 Fixture Coin 7  FC7         $14.3K   +0.70%   -0.35%   +0.07%│
│  8 Fixture Coin 8  FC8         $12.5K   +0.80%   -0.40%   +0.08%│
│  …                                                              │
└────────────────────────────────────────────────────────────────┘
LIVE | age 0s | q quit | r refresh
```

Compact layout (60×16), what you see in a narrow terminal:

```text
┌ Market summary ──────────────────────────────────────────┐
│Cap:$2T Vol24:$80.0B BTCdom:54.00% Mkt24:-0.40%           │
└──────────────────────────────────────────────────────────┘
┌Market | Live─────────────────────────────────────────────┐
│  # Symbol          Price      24h                        │
│  1 FC1             $100K   -0.05%                        │
│  2 FC2            $50.0K   -0.10%                        │
│  3 FC3            $33.3K   -0.15%                        │
│  4 FC4            $25.0K   -0.20%                        │
│  5 FC5            $20.0K   -0.25%                        │
│  6 FC6            $16.7K   -0.30%                        │
│  7 FC7            $14.3K   -0.35%                        │
└──────────────────────────────────────────────────────────┘
LIVE | age 0s | q quit | r refresh
```

## Prerequisites

- Rust (stable; the codebase tracks 1.97.1) and Cargo. Install via [rustup](https://rustup.rs/) if you do not have them.

## Install

```sh
git clone https://github.com/matheuseabra/coin-tui
cd coin-tui
cargo build --release --locked
```

The binary is written to `target/release/coin-tui`. Add that directory to your `PATH` if you want to run it by name.

## Quick start

```sh
cargo run --release --locked
```

CoinGecko's public API may ask for a free demo key. Set it once in your shell profile:

```sh
export COIN_TUI_API_KEY=<your demo key>
```

Get a free demo key from the [CoinGecko API dashboard](https://www.coingecko.com/en/api). The key is sent as the `x-cg-demo-api-key` header and never appears on screen or in logs.

### Run offline with the fixture server (no key needed)

The repo ships a loopback mock of the CoinGecko endpoints, so you can try the dashboard without an API key or internet:

```sh
# terminal 1
python3 scripts/fixture-server.py --port 8137

# terminal 2
./target/release/coin-tui --base-url http://127.0.0.1:8137/
```

Raw HTTP is allowed only for loopback hosts, so the fixture is safe by construction. `q` quits; `r` refreshes whenever you want new fixture data.

## Controls

| Input | Action |
| --- | --- |
| `q`, `Ctrl-C` | Quit and restore the terminal. |
| `j`, `Down` | Select the next visible coin. |
| `k`, `Up` | Select the previous visible coin. |
| `g`, `Home` | Select the first visible coin. |
| `G`, `End` | Select the last visible coin. |
| `PageUp`, `PageDown` | Move by one viewport. |
| `/` | Search by coin name or symbol (case-insensitive). `Enter` applies, `Esc` cancels. |
| `Esc` | Cancel search or close help. |
| `s`, `Shift-S` | Cycle the sort key forward or backward (rank, price, 1h, 24h, 7d, cap, volume, supply). |
| `r` | Refresh now, unless a refresh is already running or cooling down. |
| `?` | Toggle keybinding help. |

Sorting is stable — selection stays on the same coin when possible, and missing values sort last. The status line shows the active sort key and direction; rank order is the default.

## Responsive layouts

| Width | Layout |
| --- | --- |
| `< 80` columns | Compact: rank, symbol, price, and 24-hour change. The summary becomes one status line and the sparkline is hidden. |
| `80..119` columns | Standard: name, symbol, price, 1h, 24h, 7d, and market cap. |
| `>= 120` columns | Full: standard columns plus volume, supply, and the sparkline. |

The minimum supported terminal is 60 columns by 16 rows. Below that, the app shows a centered resize message and keeps `q` working.

## Status states

The footer always tells you what is happening:

| State | What you see |
| --- | --- |
| Loading | Progress while the first snapshot arrives. |
| Live | The age of the last successful refresh. |
| Refreshing | Current rows stay visible while new data loads in the background. |
| Stale | Last good snapshot plus its age and the refresh error. |
| Empty | The provider returned no rows; refresh is offered. |
| Rate limited | Cooldown countdown; refresh is blocked until it passes. |
| Offline | Whether stale data still shows, and when a retry is allowed. |
| Fatal | Why no data is available; quit and help stay active. |

Data refreshes every 60 seconds while healthy. After a failure the app waits out the provider's `Retry-After` window (rate limited) or a capped backoff before retrying automatically.

## Configuration

All options are flags; `--base-url`, `--api-key`, and `--log-file` also read their environment variable, which the flag overrides.

| Flag | Env | Default | Notes |
| --- | --- | --- | --- |
| `--refresh-seconds <N>` | — | `60` | Auto-refresh interval; minimum 15. |
| `--currency <SYM>` | — | `usd` | Quote currency; the MVP supports USD only. |
| `--base-url <URL>` | `COIN_TUI_BASE_URL` | `https://api.coingecko.com/` | Must be HTTPS, or plain HTTP to a loopback host (for mocks). No credentials in the URL. |
| `--api-key <KEY>` | `COIN_TUI_API_KEY` | (none) | Sent as `x-cg-demo-api-key`. |
| `--log-file <FILE>` | `COIN_TUI_LOG_FILE` | (none) | Appends redacted diagnostics to a file, off the screen. |

```sh
# 30-second refresh, with diagnostics to a file
./target/release/coin-tui --refresh-seconds 30 --log-file /tmp/coin-tui.log
```

`coin-tui --help` prints the full option reference.

## Troubleshooting

- **"not a terminal" / blank screen** — the dashboard must run inside a real terminal (it draws an alternate screen). Re-run from a terminal emulator with `$TERM` set.
- **Terminal too small** — a centered resize message appears below 60×16; grow the window or `q` to quit.
- **401 / invalid API key** — the live CoinGecko API rejects requests without a valid demo key. Set `COIN_TUI_API_KEY` (see Quick start) or point `--base-url` at the fixture server.
- **Rate limited** — the dashboard shows a cooldown countdown and blocks refresh until the provider's window passes; manual `r` is ignored during cooldown.
- **Blank or missing values** — missing numbers render as `-`; color never carries meaning alone, so the dashboard stays readable without color.
- **API key on screen or in logs** — it never is. Diagnostics written by `--log-file` are redacted, and the key is covered by tests that assert it never appears in rendered output or trace logs.
- **Build fails** — make sure `rustup`-managed `rustc` and `cargo` are on your `PATH` (`cargo --version`), then retry `cargo build --release --locked`.

## Limitations

The first release is a read-only informational dashboard. It does not provide financial advice, and it deliberately excludes:

- trading, wallets, accounts, portfolios, and financial calculations;
- news, AI features, alerts, sentiment, and token detail pages;
- watchlist persistence, alternate currencies, providers, and themes;
- an HTTP server, browser UI, remote daemon, and plugin API.

The list is capped at 100 coins, quotes are USD only, and there is no mouse interaction by design.

## Development

```sh
cargo fmt --all -- --check      # formatting gate
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features       # unit + provider tests
scripts/fixture-server.py --port 8137            # offline mock
scripts/scenario-check.sh                       # offline/DNS/timeout/malformed/429/500 states
scripts/cycle-restore.sh                        # 10 start/refresh/quit cycles
scripts/measure-idle.sh                         # idle CPU + traced timing (macOS)
```

## Tech stack

Rust, Ratatui, Tokio, Reqwest (rustls), Crossterm, Serde.

## Docs

- [`docs/PRODUCT.md`](docs/PRODUCT.md): users, first-release scope, layouts, controls, and user-facing states.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): runtime, modules, domain model, provider, dependencies, and security.
- [`docs/ROADMAP.md`](docs/ROADMAP.md): phased tasks, dependencies, acceptance criteria, and quality gates.
- [`docs/WORKFLOW.md`](docs/WORKFLOW.md): the planning and execution process this project follows.
- [`docs/TESTING.md`](docs/TESTING.md): automated checks, fixtures, manual scenarios, and release verification.