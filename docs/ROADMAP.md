# Roadmap

## Tracking Rules

- `[ ]` means ready or blocked, `[~]` means active, and `[x]` means accepted.
- Only one task is active per executor unless the planner records independent file ownership in the task evidence.
- A task line stays concise. Put completion evidence directly below it as `Evidence: ...` after acceptance.
- Complete dependencies before starting a task. Resolve milestone gates in order.
- A milestone closes only when every task and its quality gate are accepted.

## M0: Reproducible Skeleton

Goal: a cleanly exiting application with deterministic local checks.

- [x] `M0-01` Initialize one binary crate and pin the stable Rust toolchain.
  Acceptance: `cargo run` opens an alternate screen, renders the application shell, and both `q` and `Ctrl-C` restore the terminal.
  Evidence: Rust 1.97.1 is pinned; format, Clippy, 3 tests, locked build, and 80x24 PTY `q`/`Ctrl-C` restoration passed; Terra verification passed and adversarial review found no defects.
- [x] `M0-02` Add the minimal dependencies and module skeleton from `docs/ARCHITECTURE.md`.
  Depends on: `M0-01`.
  Acceptance: every production dependency has a used purpose; `cargo tree -d` has been reviewed for avoidable duplicate major versions.
  Evidence: The six-module skeleton uses only live Crossterm and Ratatui dependencies; format, Clippy, 4 tests, locked builds, dependency-tree review, and PTY quit restoration passed; Terra verification passed and adversarial review found no defects.
- [x] `M0-03` Add formatting, lint, and test commands to CI.
  Depends on: `M0-01`.
  Acceptance: CI runs the baseline checks from `docs/TESTING.md` on a clean checkout.
  Evidence: CI runs all baseline checks with Rust 1.97.1 and locked dependencies; checkout is SHA-pinned with credentials disabled; clean-copy checks and Terra verification passed; adversarial review found no defects.
- [x] `M0-04` Implement panic-safe terminal enter and restore.
  Depends on: `M0-02`.
  Acceptance: focused tests cover setup failure paths where practical; manual panic and normal-exit checks leave echo, cursor, and canonical input usable.
  Evidence: RAII cleanup, single ownership, retryable restoration, prior-hook restoration, and process-fatal panic handling have 8 passing tests; 80x24 PTY normal, Ctrl-C, runtime-error, main-panic, and background-panic checks restored terminal state; Terra verification passed and final review found no defects.

Quality gate `G0`:

- [x] All M0 tasks are accepted.
- [x] The baseline checks in `docs/TESTING.md` pass.
  Evidence: `cargo fmt --all -- --check`, locked Clippy with denied warnings, 8 locked tests, and locked debug build passed; dependency duplicates are transitive Ratatui requirements.

## M1: Market Data Vertical Slice

Goal: fetch, normalize, and display deterministic top-market data without blocking input.

- [x] `M1-01` Define provider-independent domain types and finite-value normalization.
  Depends on: `G0`.
  Acceptance: fixture tests cover complete, missing optional, empty, and non-finite-equivalent inputs accepted by JSON.
  Evidence: Private provider-independent types normalize every scalar and sparkline; complete, omitted, empty, null, malformed, overflow, timestamp, and direct non-finite tests pass; 11 locked tests, Terra verification, and adversarial review passed.
- [x] `M1-02` Implement the CoinGecko client and DTO conversion for coin markets.
  Depends on: `M1-01`.
  Acceptance: local mock tests verify request parameters, API-key header, success conversion, malformed JSON, timeout, `429`, and `5xx`; no test calls the internet.
  Evidence: A rustls client enforces exact requests, redirect/key safety, loopback-only HTTP, streaming bounds, JSON responses, typed errors, redaction, Retry-After, DTO conversion, and oldest-source timestamps; 19 offline provider and 30 total locked tests, Terra verification, and adversarial review passed.
- [x] `M1-03` Fetch global summary metrics and combine endpoint results.
  Depends on: `M1-02`.
  Acceptance: coin rows remain usable when only summary retrieval fails; a coin retrieval failure fails the refresh.
  Evidence: Concurrent coin/global fetches fail promptly on coin errors, preserve rows and typed summary notices on summary failures, retain oldest-source freshness, and pass deterministic delayed-body tests; Terra verification and adversarial review passed.
- [x] `M1-04` Implement the async event and command loop with one cancellable refresh.
  Depends on: `M1-02`.
  Acceptance: input remains responsive during a delayed mock response; duplicate refresh requests do not start duplicate HTTP calls; shutdown joins tasks.
  Evidence: Current-thread typed event/update/command flow permits one generation-aware refresh, suppresses repeated `r`, and cancel-joins every controlled exit; gated fake and delayed Wiremock tests prove responsive input, one HTTP pair, joined shutdown, and owned state transitions; Terra verification and review passed with emergency abort-on-drop documented.
- [x] `M1-05` Render loading, ready, empty, stale, rate-limited, offline, and fatal states.
  Depends on: `M1-03`, `M1-04`.
  Acceptance: state-transition tests prove that a failed refresh preserves the last successful snapshot.
  Evidence: Owned state transitions and 60x16 TestBackend coverage render every M1 state, immediate refresh progress, summary notices, bounded hostile remote text, and cached rows with unchanged freshness after failure; local-mock PTY verification and adversarial review passed.

Quality gate `G1`:

- [x] All M1 tasks are accepted and required checks pass.
  Evidence: Locked offline format, Clippy with denied warnings, 55 tests, and debug build passed on Rust 1.97.1.
- [x] A local mock demo shows startup loading, successful data, and stale fallback.
  Evidence: A 60x16 PTY using loopback fixture responses showed Loading, Live Bitcoin data with age, immediate Refreshing, then Stale with the cached Bitcoin row after a delayed `500`; `q` restored the terminal.
- [x] Logs and displayed errors contain no configured API key.
  Evidence: Redaction tests cover malformed, transport, `429`, and `5xx` errors; the local-mock PTY capture contained neither the configured key nor a secret response body.

## M2: Reference-Inspired Dashboard

Goal: deliver the image reference's scanability in compact, standard, and full terminal widths.

- [x] `M2-01` Implement deterministic formatters for prices, percentages, compact money, supply, age, and missing values.
  Depends on: `G1`.
  Acceptance: table-driven tests cover zero, signs, sub-cent prices, trillions, rounding boundaries, and missing values.
  Evidence: Locale-independent bounded formatters cover prices, signed percentages, compact USD/supply, age, missing/non-finite values, unit rollover, negative zero, tiny prices, and extremes; 59 locked offline tests, Terra verification, and adversarial review passed.
- [x] `M2-02` Build the summary row and status line.
  Depends on: `M2-01`.
  Acceptance: summary metrics retain labels without relying on color and degrade to one line below 80 columns.
  Evidence: Labeled cap/volume/dominance/change metrics render with `-` for missing and bounded `$999T+`/`>999K%` extremes; compact form is a single ≤58-cell line below 80 columns; status line names every state with age, notice, and controls, keeps `q quit | r refresh` visible at 60 columns by dropping the degraded marker and age detail in order; width-appropriate placeholders for Initial/Loading/Fatal; 76 locked offline tests (46 unit + 30 provider), fmt, Clippy with denied warnings, and locked build pass; Terra-independent verification passed twice and adversarial review found no critical or high findings (two low findings repaired with regression tests).
- [x] `M2-03` Build the ranked table, selection style, scrolling, and positive/negative styling.
  Depends on: `M2-01`.
  Acceptance: rows align for mixed symbol/name lengths; missing values cannot shift columns; selected row remains visible.
  Evidence: A real ratatui `Table` with fixed-width columns per mode renders rank, name/symbol, price, 1h/24h/7d, cap, and full-mode volume/supply; missing values render `-` without shifting columns; reversed selection auto-scrolls into view; sign-colored changes keep explicit `+`/`-` text; wrapped info lines reserve word-boundary-exact rows at 60x16; 73 locked offline tests (43 unit + 30 provider), fmt, Clippy with denied warnings, and locked build pass; Terra-independent verification passed three rounds and adversarial review found no critical or high findings (three low findings repaired with regression tests, latent estimator and overflow defects hardened).
- [x] `M2-04` Render normalized 7-day sparklines in full mode.
  Depends on: `M2-03`.
  Acceptance: flat, rising, falling, one-point, missing, and non-finite source series render without panic.
  Evidence: Whole-series downsampled min-max normalization maps buckets into 8 block glyphs; flat renders at mid level, empty and all-non-finite render `-`, overflowed ranges fall back to flat, and 168-point series keep direction after downsampling to the 10-cell Trend column; a full-mode 120x30 render shows the actual `▁▃▆█▆▃▁` series and the missing case keeps the header; 80 locked offline tests (50 unit + 30 provider), fmt, Clippy with denied warnings, and locked debug and release builds pass.
- [x] `M2-05` Implement responsive layouts and resize handling.
  Depends on: `M2-02`, `M2-03`, `M2-04`.
  Acceptance: `TestBackend` checks at 60x16, 79x20, 80x24, 119x30, and 120x30 confirm the documented mode and no out-of-bounds rendering.
  Evidence: `render` guards below the 60x16 minimum with a centered resize message that keeps `q quits` visible at 59x15, 20x5, 80x10, and 60x8 without a panic; the five required sizes render the correct column set (compact Symbol/Price/24h without Trend/Supply at 60x16 and 79x20; standard Sym/1h/7d/Cap without Trend/Supply at 80x24 and 119x30; full Trend/Vol/Supply/7d at 120x30) and every draw completes in-bounds; 82 locked offline tests (52 unit + 30 provider), fmt, Clippy with denied warnings, and locked debug and release builds pass.

Quality gate `G2`:

- [x] All M2 tasks are accepted and required checks pass.
  Evidence: `M2-01` through `M2-05` are accepted with their acceptance criteria, and formatted both ways, Clippy with denied warnings, 82 locked offline (52 unit + 30 provider) tests, and locked debug and release builds pass on Rust 1.97.1.
- [x] Manual review confirms readable output in a truecolor terminal and with `NO_COLOR=1`.
  Evidence: Release binary against a loopback fixture server in tmux 120x30 shows a labeled summary, the full column set, sign-colored changes, and 10-glyph sparklines; `capture-pane -e` shows truecolor `38;2`/`48;2` codes without NO_COLOR and only bold/reset codes (no color SGR) with `NO_COLOR=1`, with sparklines and sign text still readable. Quit restores the shell prompt each time.
- [x] Manual resize across all breakpoints shows no panic, stale artifacts, or lost selection.
  Evidence: tmux resizes to 59x15 (centered `Terminal too small` message plus `q quits`, quit still restores), 79x20 (compact Symbol/Price/24h), 80x24 and 119x30 (standard Coin/Sym/1h/24h/7d/Cap), and 120x30 (full with Vol/Supply/Trend sparkline); every pane has intact borders with no leftover cells and the first row persists across mode switches.

## M3: Navigation And Discovery

Goal: make a 100-row market list fast to inspect without a mouse.

- [x] `M3-01` Implement row, viewport, first, and last navigation.
  Depends on: `G2`.
  Acceptance: boundary and empty-list tests cover every key in the `docs/PRODUCT.md` interaction contract.
  Evidence: `j`/`Down`, `k`/`Up`, `g`/`Home`, `G`/`End`, and `PageUp`/`PageDown` route through one `navigate` transition that clamps at the first and last row, pages by the resize-derived viewport (`height - 7` table chrome, floored at 1), and no-ops on an empty list; resize dimensions now feed the viewport through `Event::Resize { height }`; 86 locked offline tests (56 unit + 30 provider) plus fmt, Clippy with denied warnings, and locked debug and release builds pass; a live 80x24 loopback run confirmed selection follows `jjj`, `G` scrolls to row 100, `kk` and page keys keep the reversed highlight in view, and `q` restores the shell prompt.
- [x] `M3-02` Implement search mode over names and symbols.
  Depends on: `M3-01`.
  Acceptance: typing does not trigger global shortcuts; apply, cancel, no-results, Unicode input, and selection retention are tested.
  Evidence: `/` opens an editing prompt (`search:<buffer>_ | Esc cancel | Enter apply`) where printable chars/Backspace fill a bounded 64-scalar buffer without triggering global shortcuts (`r`/`j`/`k`/`q`/`g`/`G` are typed, hard `Ctrl-C` still quits); `Enter` commits and `Esc` cancels editing or clears a committed filter; the filter matches a coin when its lowercased name or symbol contains the lowercased query and selection re-anchors to the same coin id when it still matches, clamping otherwise; the table renders only visible rows and shows a centered no-results message with edit/clear hints at zero matches; `filter: <query> (<count>)` appears in the status line; 12 new tests cover shortcuts, apply, cancel, no-results, Unicode, backspace-by-scalar, buffer bound, and retention; 97 locked offline tests (67 unit + 30 provider) plus fmt, Clippy with denied warnings, and locked build pass; a live 80x24 loopback run showed `/bit` narrowing to Bitcoin+Bitbo with `filter: bit (2)`, the typing prompt, `No coins match "zz"` after `Enter`, `Esc` restoring all rows, and `q` restoring the shell prompt.
- [x] `M3-03` Implement stable sort cycling with missing values last.
  Depends on: `M3-01`.
  Acceptance: every visible numeric column sorts in both directions with deterministic ties.
  Evidence: `s` advances and `Shift-S` steps backward through a fixed 16-step cycle (rank, price, 1h, 24h, 7d, cap, volume, supply × ascending/descending) that wraps at the default rank order; sorting runs on the already-filtered visible set, missing values sort last in both directions, equal finite values keep snapshot order (stable `sort_by` with a non-`NaN`-leaking comparator), and selection re-anchors to the same coin id after reordering; the status line shows `sort: <key> ↑/↓` and drops it in the default rank order; 104 locked offline tests (74 unit + 30 provider) plus fmt, Clippy with denied warnings, and locked debug and release builds pass; a live 80x24 loopback run confirmed `sss` → `sort: price ↓` order (Bitcoin, Ethereum, Solana, Litecoin, Bitbo, Cardano), `Shift-S` → `price ↑`, two more `Shift-S` → `rank ↓` then back to default rank order with the indicator gone, and `q` restored the shell prompt. A latent M3-02 selection-retention bug (the anchor was captured after the query changed) was fixed by capturing the coin id before the visible set changes, with a new regression test.
- [ ] `M3-04` Add a contextual help overlay.
  Depends on: `M3-01`, `M3-02`, `M3-03`.
  Acceptance: help lists only implemented bindings, fits the minimum supported size, and closes with `?` or `Esc`.

Quality gate `G3`:

- [ ] All M3 tasks are accepted and required checks pass.
- [ ] A keyboard-only manual scenario can find Bitcoin, sort by 7-day change, return to rank order, refresh, and quit.

## M4: Operational Hardening

Goal: behave predictably under real network, terminal, and provider failures.

- [ ] `M4-01` Implement refresh scheduling, `Retry-After`, and capped jittered backoff.
  Depends on: `G3`.
  Acceptance: paused-time Tokio tests cover success reset, cooldown, cap, and manual refresh during cooldown.
- [ ] `M4-02` Add bounded response handling, remote-text sanitization, and redacted file tracing.
  Depends on: `G3`.
  Acceptance: oversized, control-character, and secret-bearing fixtures cannot corrupt the terminal or leak the key.
- [ ] `M4-03` Validate CLI and environment configuration.
  Depends on: `G3`.
  Acceptance: help is complete; invalid currency, interval, base URL, and conflicting options fail before terminal entry with actionable messages.
- [ ] `M4-04` Measure idle and refresh behavior and remove observed hot loops.
  Depends on: `M4-01`.
  Acceptance: idle rendering is event-driven, idle CPU is recorded for a 60-second release run, and refresh timing is recorded against a local delayed mock.

Quality gate `G4`:

- [ ] All M4 tasks are accepted and required checks pass.
- [ ] Ten consecutive start/refresh/quit cycles restore the terminal.
- [ ] Offline startup, DNS failure, timeout, malformed response, `429`, and `500` scenarios show the documented state.
- [ ] No secrets appear in screen output, test artifacts, or trace logs.

## M5: Release Candidate

Goal: produce a documented, reproducible first release.

- [ ] `M5-01` Complete the user README with installation, API key setup, controls, screenshots, limitations, and troubleshooting.
  Depends on: `G4`.
  Acceptance: a clean-shell walkthrough follows only README instructions and reaches live or fixture-backed data.
- [ ] `M5-02` Add license, changelog, release profile, and package metadata.
  Depends on: `G4`.
  Acceptance: `cargo package --list` contains only intended files and metadata has no placeholders.
- [ ] `M5-03` Add platform release builds for macOS and Linux.
  Depends on: `M5-02`.
  Acceptance: CI builds release artifacts and records checksums; publishing remains manual.
- [ ] `M5-04` Run the release matrix and resolve all release-blocking defects.
  Depends on: `M5-01`, `M5-03`.
  Acceptance: results are recorded for macOS Terminal, iTerm2, and one Linux terminal at compact, standard, and full widths; unsupported environments are documented.

Quality gate `G5`:

- [ ] All M5 tasks are accepted.
- [ ] The release suite in `docs/TESTING.md` passes on a clean checkout.
- [ ] The release matrix passes with no critical or high-severity known defect.
- [ ] A human approves the release; no agent publishes artifacts without explicit approval.

## Deferred Tracks

These items require a new milestone and acceptance criteria. They are not implicit MVP work:

- persistent watchlists and local configuration files;
- alternate quote currencies and providers;
- coin detail and historical chart screens;
- configurable columns and themes;
- Windows release support;
- news, sentiment, alerts, trading, accounts, or AI features;
- an Axum service, browser UI, remote daemon, or plugin API.
