# Product

## Purpose

Coin TUI is a fast, read-only cryptocurrency market dashboard for the terminal. It helps terminal users scan market direction and compare leading coins without opening a browser. It takes the information hierarchy of the supplied CoinMarketCap reference, not its full feature set or visual density.

## Users And Jobs

The first release serves developers, terminal users, and crypto observers who need to:

- see the broad market state at a glance;
- compare the leading coins across short timeframes;
- find and sort a coin quickly with a keyboard;
- keep useful data visible when the provider or network fails;
- understand data freshness before acting on it.

The product is informational. It does not provide financial advice or execute financial actions.

## First Release

The first release includes:

- a compact global market summary;
- a ranked list of up to 100 coins by market capitalization;
- price, 1-hour, 24-hour, and 7-day changes;
- market cap, 24-hour volume, circulating supply, and a 7-day sparkline;
- keyboard navigation, stable sorting, search, manual refresh, and responsive layouts;
- explicit loading, live, refreshing, stale, empty, rate-limited, offline, and fatal states;
- USD as the only quote currency.

## Non-Goals

The first release excludes:

- trading, wallets, accounts, portfolios, and financial calculations;
- news, AI features, alerts, sentiment, and prediction markets;
- watchlist persistence, alternate currencies, and providers;
- token image downloads and a mouse-first experience;
- an HTTP server, browser UI, remote daemon, and plugin API;
- full feature or visual parity with CoinMarketCap.

## Product Surface

```text
+ Market cap -----+ 24h volume -----+ BTC dominance --+ Updated / status -+
+-------------------------------------------------------------------------+
| #  Coin       Price      1h      24h       7d    Market cap   7d trend |
| 1  Bitcoin    $...      +...     -...      +...   $...         ./^^... |
| 2  Ethereum   $...      -...     +...      -...   $...         ^^\_... |
+-------------------------------------------------------------------------+
| / search  s sort  r refresh  ? help  q quit                LIVE / STALE |
+-------------------------------------------------------------------------+
```

The summary provides context. The table is the primary surface. Color reinforces signs and state but never carries meaning alone.

## Responsive Modes

| Width | Layout |
| --- | --- |
| `< 80` columns | Compact table: rank, symbol, price, and 24-hour change. The summary becomes one status line and the sparkline is hidden. |
| `80..119` columns | Standard table: name and symbol, price, 1-hour, 24-hour, 7-day, and market cap. |
| `>= 120` columns | Full table: standard columns plus volume, supply, and sparkline. |

The minimum supported terminal is 60 columns by 16 rows. Below that size, the product shows a centered resize message and keeps quit handling active.

## Interaction Contract

| Input | Action |
| --- | --- |
| `q`, `Ctrl-C` | Quit and restore the terminal. |
| `j`, `Down` | Select the next visible coin. |
| `k`, `Up` | Select the previous visible coin. |
| `g`, `Home` | Select the first visible coin. |
| `G`, `End` | Select the last visible coin. |
| `PageUp`, `PageDown` | Move by one viewport. |
| `/` | Enter search mode. |
| `Esc` | Cancel search, close help, or return from the coin detail screen. |
| `Enter` | Apply search, or open the selected coin's detail screen. |
| `s`, `Shift-S` | Cycle sort key forward or backward. |
| `r` | Request a refresh unless one is active or cooling down. |
| `t`, `Shift-T` | Cycle the color theme forward or backward. |
| `?` | Toggle keybinding help. |

## Coin Detail

`Enter` on a selected row opens a read-only detail screen for that coin. The screen keeps the market summary for context and replaces the table with the coin's identity, price and size stats, a color-coded 1h/24h/7d change strip, and its 7-day price chart. `Esc` returns to the table with the selection and viewport unchanged.

The detail chart renders the 7-day price series already delivered by the markets snapshot; it makes no extra provider request and behaves the same when stale, offline, or offline-fixture-driven. A flat or hostile series renders as a bounded flat line, an all-missing series shows a placeholder message, and long series are downsampled to a fixed point budget. Color only reinforces the sign of each change; every value remains sign-prefixed text. While the detail screen is open, navigation, search, and sort keys are ignored; `r`, `?`, `q`, and `Esc` stay active.

## Themes

`t` and `Shift-T` cycle between built-in themes without restarting: `Default`, `Nord`, and `Monochrome`. The status line names the active theme whenever it is not the startup default. A theme recolors the summary, table header, change cells, detail change strip, chart line, help overlay, resize message, and no-results notices. Color only reinforces text, never carries meaning alone, and no layout, column, or state decision depends on the active theme, so every theme renders at every supported width and stays readable without color (including with `NO_COLOR=1` and the `Monochrome` theme).

Search is case-insensitive over coin name and symbol. Sorting is stable, missing values sort last, and selection remains on the same coin ID when possible. Each sortable numeric column appears in the cycle in both directions: `s` advances through rank, price, 1h, 24h, 7d, cap, volume, and supply (ascending then descending) and `Shift-S` steps backward. The status line shows the active key and direction; the default rank order shows no indicator.

## User-Facing States

| State | Required behavior |
| --- | --- |
| Loading | Show progress without an empty table that looks final. Keep quit and help active. |
| Live | Show the last successful refresh age. |
| Refreshing | Keep current rows visible and indicate background work. |
| Stale | Keep the last successful snapshot and show its age plus the refresh error. |
| Empty | Explain that the provider returned no market rows and offer refresh. |
| Rate limited | Show cooldown status and prevent refresh attempts that bypass it. |
| Offline | Explain whether stale data remains available and offer refresh when allowed. |
| Fatal | Explain why no usable data is available and keep quit and help active. |

Messages state impact and one action. Example: `Offline: showing data from 4m ago; press r to retry`. During a refresh cooldown the message replaces the retry call-to-action with a countdown, for example `Refresh failed: rate limited. Retrying automatically in 23s.`

Market data refreshes automatically every 60 seconds while the connection is healthy. After a failure the app waits out the provider's `Retry-After` window (rate limited) or a capped jittered backoff, shows the countdown, and blocks both automatic and manual refresh until it passes.

## Product Quality

The release is acceptable when users can complete this keyboard-only path: start the app, identify broad market direction, find Bitcoin, open its detail screen and read its 7-day chart, return to the table, sort by 7-day change, return to rank order, refresh, inspect freshness, and quit with the terminal restored.

The dashboard must remain readable without color, at every supported width, and while showing missing values or stale data. Verification criteria are defined in `TESTING.md`; delivery state is tracked in `ROADMAP.md`.
