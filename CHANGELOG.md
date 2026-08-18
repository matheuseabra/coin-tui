# Changelog

All notable changes to `coin-tui` are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
for [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

First release candidate. The application surface is complete; this section will
become the `0.1.0` changelog on the first tagged release.

### Added

- Compact global market summary: market cap, 24-hour volume, BTC dominance, and 24-hour change.
- Up to 100 market-cap-ranked coins with price, 1h/24h/7d change, market cap, 24-hour volume, circulating supply, and a 7-day sparkline.
- Keyboard-only navigation: row/viewport movement, case-insensitive search, stable sort cycling, manual refresh, and a help overlay.
- A read-only coin detail screen in the CoinMarketCap shape: left-aligned, width-limited content column with identity header, price with 24-hour change, 1h/24h/7d strip, a 7-day gradient area chart with real price labels, and a market-stats grid; at wide panes a `Coin data` sidebar adds 24h high/low, ATH/ATL, supplies, fully diluted valuation, longer-period changes, sentiment votes, categories, and an About snippet from the rich `/coins/{id}` endpoint, falling back to the snapshot row when the fetch fails.
- News and sentiment panes cycled with `Tab`/`Shift-Tab`: a bounded RSS headline feed (source, age, title, URL) from `--news-url`/`COIN_TUI_NEWS_URL`, and a 24-hour market-breadth pane (up/down/flat counts, bullish meter, average, best/worst mover); the panes render side-by-side at 162+ columns and one-at-a-time below.
- Four built-in color themes (`Default`, `Nord`, `Tokyo Night`, `Monochrome`) cycled live with `t`/`Shift-T`; the summary line, table header and selection, change cells, sparkline, detail pane, block titles, and the detail chart's gradient fill are all themed, table rows carry breathing padding, and every theme stays readable without color.
- Responsive compact, standard, and full layouts with live/refreshing/stale/empty/rate-limited/offline/fatal states.
- CoinGecko Demo API over HTTPS with an optional `x-cg-demo-api-key` header.
- Automatic refresh cadence with `Retry-After` and capped jittered backoff, and redacted file tracing.
- Typed CLI flags and environment-variable configuration with validation before terminal entry.
- Offline fixture server (`scripts/fixture-server.py`) for local runs without an API key.