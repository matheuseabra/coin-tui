# 🪙 Coin TUI

A cryptocurrency market dashboard for the terminal, built with Rust, Ratatui, Tokio, and Reqwest.

> Coin TUI uses CoinGecko's Demo API over HTTPS. Set `COIN_TUI_API_KEY` when the endpoint requires a demo key. `COIN_TUI_BASE_URL` overrides the endpoint for local loopback mocks or compatible HTTPS hosts.

## pre-requisites
- rust
- cargo

## quickstart

```sh
$ git clone https://github.com/matheuseabra/coin-tui
$ cd coin-tui
```
## run

```sh
$ cargo run --locked
```

## controls

- `/`: filter by name
- `r`: refresh market data
- `s` or `Ctrl-S`: cycle sort order
- `q` or `Ctrl-q`: quit
- `?`: helper commands

## tech stack
- rust
- ratatui
- tokio
- reqwest
- crossterm

## docs

- [`docs/PRODUCT.md`](docs/PRODUCT.md): users, first-release scope, layouts, controls, and user-facing states.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): runtime, modules, domain model, provider, dependencies, and security.
- [`docs/ROADMAP.md`](docs/ROADMAP.md): phased tasks, dependencies, acceptance criteria, and quality gates.
- [`docs/WORKFLOW.md`](docs/WORKFLOW.md): planner, Luna Max execution, and independent Terra High verification and adversarial review.
- [`docs/TESTING.md`](docs/TESTING.md): automated checks, fixtures, manual scenarios, and release verification.

## roadmap

Implementation is in progress under the phased roadmap. The repository contains the product and engineering specifications:
- Milestones M0 and M1 are complete. The current vertical slice loads up to 100 market rows, combines global summary data, remains responsive during refreshes, and preserves stale data when refresh fails. Continue with M2 in [`docs/ROADMAP.md`](docs/ROADMAP.md).
