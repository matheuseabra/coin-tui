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
- A read-only coin detail screen with identity, price and size stats, a 1h/24h/7d change strip, and a bounded 7-day price chart.
- Three built-in color themes (`Default`, `Nord`, `Monochrome`) cycled live with `t`/`Shift-T`; every theme stays readable without color.
- Responsive compact, standard, and full layouts with live/refreshing/stale/empty/rate-limited/offline/fatal states.
- CoinGecko Demo API over HTTPS with an optional `x-cg-demo-api-key` header.
- Automatic refresh cadence with `Retry-After` and capped jittered backoff, and redacted file tracing.
- Typed CLI flags and environment-variable configuration with validation before terminal entry.
- Offline fixture server (`scripts/fixture-server.py`) for local runs without an API key.