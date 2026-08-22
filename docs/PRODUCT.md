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
- keyboard navigation, stable sorting, search, manual refresh, news and sentiment panes, and responsive layouts;
- explicit loading, live, refreshing, stale, empty, rate-limited, offline, and fatal states;
- USD as the only quote currency.

## Non-Goals

The first release excludes:

- trading, wallets, accounts, portfolios, and financial calculations;
- alerts and prediction markets (beyond the computed 24-hour breadth sentiment);
- watchlist persistence, alternate currencies, and providers;
- token image downloads and a mouse-first experience;
- an HTTP server, browser UI, remote daemon, and plugin API;
- full feature or visual parity with CoinMarketCap.

## Product Surface

The current wide coin detail layout places the `Coin data` sidebar on the right at a fixed 150 terminal-cell width. The chart and main content use the remaining space; narrow terminals keep the stacked layout.

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
| `80..119` columns | Standard table: name and symbol, price, 1-hour, 24-hour, 7-day, market cap, and 24-hour volume. |
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
| `Tab`, `Shift-Tab` | Move pane focus forward or backward between the market table, news, and sentiment panes. |
| `Up`, `Down`, `PageUp`, `PageDown`, `Home`, `End` | Scroll the news pane when it is focused. |
| `?` | Toggle keybinding help. |

## Coin Detail

`Enter` on a selected row opens a read-only detail screen for that coin. The screen keeps the market summary for context and replaces the table with a CoinMarketCap-shaped layout in a left-aligned, width-limited content column: an identity header (rank, name, symbol), the price with its color-coded 24-hour change, the 1h/24h/7d change strip, the candlestick chart with real price labels, and a market-stats grid. `Esc` returns to the table with the selection and viewport unchanged.

The detail chart opens instantly from the snapshot's 7-day series and, when the provider supports it, upgrades to a 30-day price history fetched on demand (`/coins/{id}/market_chart`), stretching the full content column with up to thirty daily open/high/low/close candles; when that fetch is unavailable or fails, it stays on the 7-day candles and behaves the same when stale, offline, or offline-fixture-driven. `[` and `]` switch the visible chart range between `1 day`, `7 days`, and `30 days`; the selected range filters the available points locally and appears in the caption. The chart draws a library-rendered autoscaling price axis and time axis, gain/loss coloring for up and down candles (the wick follows the 7-day trend), and a range caption below showing the plotted price range. A flat or hostile series renders as a bounded mid-line, an all-missing series shows a placeholder message, and the caption always shows the plotted price range. Color only reinforces the sign of each change; every value remains sign-prefixed text. While the detail screen is open, navigation, search, and sort keys are ignored; `r`, `?`, `q`, `Esc`, `[` and `]` stay active.

At wide panes the detail screen adds a left-hand `Coin data` column occupying about 30% of the detail width, with the chart and main content using the remaining 70%. It is fed by the rich `/coins/{id}` endpoint: 24-hour high/low, ATH and ATL with their change, circulating/total/max supply, fully diluted valuation, 14d/30d/60d/1y changes (where the provider supplies them), community sentiment votes, categories, and a bounded About snippet from the provider description. Long values and About text wrap inside the sidebar instead of running into its edge, and About appears once as a dedicated subsection. The pane opens instantly from the snapshot row and upgrades when the fetch lands; if the fetch fails or the provider does not support detail, it stays on the row-derived fallback with an "extended data unavailable" note. At narrow panes the same extended fields render as two compact stat lines under the chart instead of a sidebar.

## News And Sentiment Panes

`Tab` and `Shift-Tab` move focus between three panes: the market table, a news wire, and a market-breadth sentiment pane. On terminals at least 162 columns wide the news and sentiment panes render beside the table in a right column (the body splits 70/30 — table 70%, panes 30% — and the right column divides into two equal rows, news on top, sentiment below), and focus highlights the active pane's border and title; on narrower terminals one focused pane replaces the table at a time so the table keeps its full column set. Focus is keyboard-only and modal: pane keys are swallowed while searching, while help is open, or on the coin detail screen.

The news pane shows the latest headlines from the configured RSS feed (`--news-url`, default CoinDesk). Each headline renders the bounded title first, wraps it to the pane width, then shows a `· time · category` metadata line such as `· 2h · Markets`; URLs are not rendered. News and sentiment content have one terminal-cell of inner padding, about 8px in a typical terminal font. When focused, the pane scrolls with directional and page keys; `Home` and `End` move to the first and last content. Before the first result the pane shows a loading placeholder, after a failed refresh it keeps the last headlines and appends a one-line failure notice, and with the feed disabled it says the feed is unavailable.

The sentiment pane computes 24-hour market breadth from the current snapshot: up/down/flat counts, a bullish-share meter, the average 24-hour change, and the best and worst mover. It also appends the optional keyless Alternative.me Fear & Greed Index value and classification. A provider failure keeps market breadth visible and omits the external index. News and sentiment are informational.

## Themes

`t` and `Shift-T` cycle between built-in themes without restarting: `Default`, `Nord`, `Tokyo Night`, and `Monochrome`. The status line names the active theme whenever it is not the startup default. A theme recolors the summary line's labels (with the market-cap change by sign), the table header, the selected-row highlight, the change cells, the 7-day trend sparkline, the detail change strip, the detail chart's candle and wick colors, the block titles, and the help overlay, resize message, and no-results notices. Table rows carry one blank breathing line of vertical padding so the trend charts have room. Color only reinforces text, never carries meaning alone, and no layout, column, or state decision depends on the active theme, so every theme renders at every supported width and stays readable without color (including with `NO_COLOR=1` and the `Monochrome` theme).

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
