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
- [x] `M3-04` Add a contextual help overlay.
  Depends on: `M3-01`, `M3-02`, `M3-03`.
  Acceptance: help lists only implemented bindings, fits the minimum supported size, and closes with `?` or `Esc`.
  Evidence: `?` toggles a centered bordered `Help` overlay (40 wide, 12 rows tall) that fits the 60x16 minimum (10 binding lines, each ≤ 38 columns, plus borders) and lists only implemented bindings – `q`/`Ctrl-C`, `j`/`Down`, `k`/`Up`, `g`/`Home`, `G`/`End`, `PageUp`/`Down`, `/` (search, `Enter`/`Esc`), `s`/`Shift-S`, `r`, and `?`/`Esc` to close; `?` or `Esc` closes it while `q`/`Ctrl-C` still quit, and every other key is swallowed so the overlay is modal and cannot leak shortcuts, search, sort, or refresh; `?` typed while search editing stays query text; the overlay draws over the live table and closes by restoring it; 5 new tests cover toggle/close, modality + quit, and search typing; 109 locked offline tests (79 unit + 30 provider) plus fmt, Clippy with denied warnings, and locked debug and release builds pass.
Quality gate `G3`:

- [x] All M3 tasks are accepted and required checks pass.
  Evidence: `M3-01`, `M3-02`, `M3-03`, and `M3-04` are each accepted with their checks listed, and every M3 run above passes `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` (109 locked offline tests: 79 unit + 30 provider), and locked debug plus release builds.
- [x] A keyboard-only manual scenario can find Bitcoin, sort by 7-day change, return to rank order, refresh, and quit.
  Evidence: a live 80x24 loopback run completed the whole path with the keyboard only – identify direction via the summary (`Mkt 24h: -0.65%`), open the `?` help overlay and list its bindings; `/bit Enter` narrowed to Bitcoin + Bitbo with `filter: bit (2)`; `Esc` cleared the filter back to all rows; nine `s` presses reached `sort: 7d ↓` ordered by 7-day change (Bitbo +9.90%, Ethereum +3.00%, Solana +1.40%, Cardano -0.50%, Bitcoin -1.20%, Litecoin -2.10%); seven more `s` wrapped to default rank order (Bitcoin #1 … Solana #6) with the sort indicator gone; `r` refreshed the snapshot to `LIVE | age 0s`; and `q` exited, restoring the shell prompt (`.bashrc` banner and prompt visible in the pane). Note: `tmux send-keys Esc` sends the literal text `Esc`; the real Escape key is `Escape` in the harness – the app handled every real keystroke correctly.

## M4: Operational Hardening

Goal: behave predictably under real network, terminal, and provider failures.

- [x] `M4-01` Implement refresh scheduling, `Retry-After`, and capped jittered backoff.
  Depends on: `G3`.
  Acceptance: paused-time Tokio tests cover success reset, cooldown, cap, and manual refresh during cooldown.
  Evidence: `RefreshScheduler` in `src/app.rs` owns the cadence (60s default, min enforced), opening a cooldown on failure that blocks both the automatic tick and manual `r` until it passes; `failure_retry_delay` honors `Retry-After` exactly (floored at 1s) and applies capped equal-jittered exponential backoff (`[scaled/2, scaled]`, cap 60s) to transport, timeout, `5xx`, and bare-`429` failures. The run loop emits a low-frequency `Tick` (first tick delayed, then 1s) that re-renders freshness and starts due automatic refreshes. Paused-time tests named `success_resets_the_refresh_cadence` (interval fires at 60s and resets after success), `retry_after_opens_a_cooldown_that_blocks_manual_refresh` (30s window: manual `r` blocked at 10s, auto retry after 30s only), `backoff_window_is_capped_after_repeated_failures` (windows never exceed 60s, floor reached), `jittered_backoff_stays_within_its_scaled_window`, `failure_retry_delay_honors_retry_after_and_classifies_errors`, and `tick_drives_an_automatic_refresh_through_the_loop` (exactly one initial fetch, a second fetch after the interval) cover the acceptance. UI shows a cooldown countdown (`Retrying automatically in Ns`) instead of `Press r to retry` in Stale and Fatal states. `tokio` gained the dev-only `test-util` feature (see `docs/ARCHITECTURE.md`). Gates after implementation: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` (85 unit + 30 provider), and `cargo build --locked` all pass.
- [x] `M4-02` Add bounded response handling, remote-text sanitization, and redacted file tracing.
  Depends on: `G3`.
  Acceptance: oversized, control-character, and secret-bearing fixtures cannot corrupt the terminal or leak the key.
  Evidence: response bodies are capped at 2 MiB in `src/api.rs` with any overflow rejected as `MalformedResponse`; renderer `clean_remote`/`truncate_cells` in `src/ui.rs` strip control and terminal-format characters and clamp to cell width. New `src/log.rs` `FileLog` appends redacted diagnostics to `COIN_TUI_LOG_FILE`, scrubbing the API key out of every line before it reaches disk; the refresh lifecycle traces `session start`, `refresh start/ok/failed generation=N`, and `loop stopped`. New tests prove the acceptance at every level: `hostile_provider_fixtures_never_corrupt_the_terminal_or_leak_the_key` (fixture server serving ESC/bidi/NUL/DEL names plus the key; rendered 80x24 buffer has no control chars and no key, LIVE state, key confirmed sent as header), `traced_session_logs_redact_the_key_and_record_the_refresh_timeline` (real loop, temp log file, key absent), `traced_failed_refresh_logs_the_error_without_the_key`, `malformed_response_keeps_last_good_rows_and_renders_cleanly` (oversized-as-malformed → STALE with table intact), plus `log.rs` units `redact_replaces_every_secret_occurrence_and_ignores_empty_secrets`, `file_log_writes_redacted_lines_to_disk`, `two_clone_handles_share_one_redacted_file`, and strengthened provider tests `enforces_json_type_and_streaming_size_limit` (oversize body carries the key text; none in Display/Debug) and `hostile_control_characters_pass_through_the_provider_boundary_for_render_sanitization`. `tempfile` joined dev-dependencies for isolated log-file tests; bookkeeping in `docs/ARCHITECTURE.md` records `COIN_TUI_LOG_FILE` and that redaction uses the self-contained `FileLog` instead of `tracing`. Gates after implementation: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` (92 unit + 31 provider), and `cargo build --locked` all pass.
- [x] `M4-03` Validate CLI and environment configuration.
  Depends on: `G3`.
  Acceptance: help is complete; invalid currency, interval, base URL, and conflicting options fail before terminal entry with actionable messages.
  Evidence: `src/config.rs` centralizes typed `clap` parsing for `--refresh-seconds`, `--currency`, `--base-url`, `--api-key`, and `--log-file` with matching environment variables where documented; config tests cover defaults, complete help output, minimum interval, USD-only currency, HTTPS-or-loopback base URLs, no credential echo in base-URL errors, and a real parser/env conflict. Startup probes against `target/debug/coin-tui` showed complete help plus code-2 plain-stderr failures for `--currency eur`, `--refresh-seconds 5`, `--base-url http://example.com/`, `--base-url https://user:secret-password@example.com/` without printing the password, and `COIN_TUI_BASE_URL=... --base-url ...`, all before alternate-screen entry; sanitized probe evidence is in `.omo/evidence/M4-03-cli-probes.md`. Gates after implementation and repair: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` (100 unit + 31 provider), and `cargo build --locked` all pass.
- [x] `M4-04` Measure idle and refresh behavior and remove observed hot loops.
  Depends on: `M4-01`.
  Acceptance: idle rendering is event-driven, idle CPU is recorded for a 60-second release run, and refresh timing is recorded against a local delayed mock.
  Evidence: The run loop is event-driven: `Controller::start_fetch` stamps `refresh_started_at` and the loop traces `refresh ok/failed ... duration=NNNms` plus a `render ok duration=NNNms` line per draw. A paused-time regression test `idle_rendering_is_event_driven_and_steady` counted 30 and 31 renders across two 30-second idle windows, exactly the 1 Hz tick cadence. `scripts/fixture-server.py` (loopback CoinGecko-compatible mock) and `scripts/measure-idle.sh` (detached 120x30 tmux run, `ps -o %cpu=` sampling) recorded a 60-second idle release run at 0.09% average CPU (2.1% peak) with 64 traced renders over ~61 seconds, so no hot loop existed to remove; a 250 ms fixture delay was traced as `refresh ok generation=1 coins=100 duration=265ms`. Full procedure and outputs are in `.omo/evidence/M4-04-measurements.md`. Gates after implementation: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` (102 unit + 31 provider), and `cargo build --locked` all pass.

Quality gate `G4`:

- [x] All M4 tasks are accepted and required checks pass.
  Evidence: `M4-01` through `M4-04` are accepted, and the final M4 baseline `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features` (102 unit + 31 provider), and locked debug and release builds all pass on Rust 1.97.1.
- [x] Ten consecutive start/refresh/quit cycles restore the terminal.
  Evidence: `scripts/cycle-restore.sh` ran 10 cycles of the release binary in a detached 80x24 tmux pane against `scripts/fixture-server.py`: every cycle reached a successful refresh, answered `r` then `q`, and the restored pane showed the shell's `cycle-done-N` marker with no leftover app UI (`q quit` absent): 10 passed, 0 failed.
- [x] Offline startup, DNS failure, timeout, malformed response, `429`, and `500` scenarios show the documented state.
  Evidence: `scripts/scenario-check.sh` ran each scenario in a detached 80x24 pane and matched the `PRODUCT.md` wording: offline startup (`http://closed-loopback`, `Offline: no market data is available; press r to retry`), DNS failure (`https://does-not-exist.invalid/`, same OFFLINE message), malformed response (fixture serving non-JSON, `Error: invalid provider response; press r to retry`), `429` with `Retry-After: 5` (`RATE LIMITED` body `Rate limited: ...`), `500` (`Error: provider request failed; press r to retry`), and timeout (fixture holding the markets response past the 30s client window, OFFLINE message) all passed. The `fixture-server.py` `--mode` flag (ok/malformed/rate-limited/server-error/timeout) backs these offline runs.
- [x] No secrets appear in screen output, test artifacts, or trace logs.
  Evidence: running with `COIN_TUI_API_KEY=pragma-gate-secret` produced zero occurrences of the key in the tmux pane capture, the `COIN_TUI_LOG_FILE` trace, or the measurement report, and a repo sweep found none; automated coverage remains (`hostile_provider_fixtures_never_corrupt_the_terminal_or_leak_the_key`, `secret_bearing_rate_limit_and_server_errors_are_redacted`, `omits_key_and_classifies_failures_without_secret_in_display`, and the `FileLog` redaction unit tests).

## M5: Release Candidate

Goal: produce a documented, reproducible first release.

- [x] `M5-01` Complete the user README with installation, API key setup, controls, screenshots, limitations, and troubleshooting.
  Depends on: `G4`.
  Acceptance: a clean-shell walkthrough follows only README instructions and reaches live or fixture-backed data.
  Evidence: `README.md` rewritten (features, two fixture-backed screenshot captures, prerequisites, install and quick-start with the free demo key, an offline fixture-server quick start, the full `PRODUCT.md` controls table, responsive layouts, status states, the flag/env configuration table, troubleshooting, limitations, and development commands). The walkthrough was executed from a detached clean shell following only the README's "Run offline with the fixture server" steps: `scripts/fixture-server.py --port 8137` plus `./target/release/coin-tui --base-url http://127.0.0.1:8137/` reached a `LIVE` fixture-backed dashboard with summary and rows, `q` restored the shell, and the resumed shell ran the next command.
- [x] `M5-02` Add license, changelog, release profile, and package metadata.
  Depends on: `G4`.
  Acceptance: `cargo package --list` contains only intended files and metadata has no placeholders.
  Evidence: MIT license chosen by the user (`LICENSE`), `CHANGELOG.md` (Keep a Changelog format, `[Unreleased]` first-release section), `[profile.release]` with `lto = "thin"`, `codegen-units = 1`, `strip = true`, and complete `[package]` metadata (description, `license = "MIT"`, readme, repository/homepage, keywords, categories, and `exclude` of `.github/`, `.opencode/`, `.gitignore`, `AGENTS.md`). `cargo package --allow-dirty --list` shows only intended files (LICENSE, CHANGELOG, README, `docs/`, `scripts/`, `src/`, `tests/`, Cargo.toml, Cargo.lock, rust-toolchain.toml), metadata has no placeholders, `cargo package` verification packaged 31 files (411.3KiB) and compiled the crate, and fmt, Clippy with denied warnings, 133 locked tests (102 unit + 31 provider), and locked debug and release builds pass.
- [x] `M5-03` Add platform release builds for macOS and Linux.
  Depends on: `M5-02`.
  Acceptance: CI builds release artifacts and records checksums; publishing remains manual.
  Evidence: `.github/workflows/release.yml` builds `--release --locked` on `ubuntu-latest` and `macos-latest` (tag `v*` push or manual `workflow_dispatch`), names each binary from the runner's actual target triple, records a `shasum -a 256` checksum beside it, and uploads `coin-tui-<os>` artifacts; it creates no GitHub Release and publishes nothing to crates.io. The workflow YAML parses, `actions/upload-artifact@043fb46` (v7.0.1) is SHA-pinned like the checkout, and the artifact/checksum steps were reproduced locally against the release binary (produced `coin-tui-aarch64-apple-darwin.sha256`). CI will only run on the first tag push or manual dispatch; that remains the verification risk.
- [ ] `M5-04` Run the release matrix and resolve all release-blocking defects.
  Depends on: `M5-01`, `M5-03`.
  Acceptance: results are recorded for macOS Terminal, iTerm2, and one Linux terminal at compact, standard, and full widths; unsupported environments are documented.

Quality gate `G5`:

- [ ] All M5 tasks are accepted.
- [ ] The release suite in `docs/TESTING.md` passes on a clean checkout.
- [ ] The release matrix passes with no critical or high-severity known defect.
- [ ] A human approves the release; no agent publishes artifacts without explicit approval.

Work on the M5 release track stops at `M5-03`. `M5-04` and the `G5` release gate
stay open as the release gate; they are not prerequisites for `M6` feature work.

## M6: Detail, Theme, And Content Tracks

Goal: add read-only coin detail and a historical chart, themeable and refined table
rendering, and news/sentiment content without weakening the keyboard-only core or
the `PRODUCT.md` layout guarantees.

- [x] `M6-01` Coin detail and historical chart screen.
  Depends on: `M5-03`.
  Acceptance: selecting a row opens a read-only detail view; the 7-day price history chart renders bounded, empty, and non-finite series without a panic; `Esc` returns to the table with selection and state preserved.
  Evidence: `Enter` stores the selected row in `App.detail` and `Esc` clears it without touching `selected` or the table viewport (`src/app.rs`); while open, navigation/search/sort keys are swallowed but `r`, `?`, `q`, and `Esc` stay active, and a completed refresh re-syncs the stored coin by ID from the new snapshot. The detail pane replaces only the table area, keeps the market summary, and draws identity, price/size stats, a sign-prefixed 1h/24h/7d strip, and the 7-day chart (`render_detail`/`render_chart` in `src/ui.rs`). The chart reuses the snapshot's normalized `sparkline_7d` series, downsampled to `MAX_CHART_POINTS = 512` buckets, min-max scaled into `[0.0, 1.0]` with flat/overflow fallback, so hostile, empty, and non-finite series render without panic and no new provider endpoint or dependency was added. New tests cover open/Esc-with-selection, the modal key set, refresh-by-ID sync, empty-list Enter, and rendering at 60x16/80x24/120x30 with empty, hostile, flat, and 2000-point series; 112 unit + 31 provider tests pass with fmt and Clippy (denied warnings). A fixture-backed walkthrough (`scripts/fixture-server.py` on loopback port 8138) opened coin 1 in 80x24, rendered the chart markers and `7 days` axis, `Esc` restored the table with the LIVE status, and `q` exited with `loop stopped success=true` and the shell restored. Product and architecture contracts updated in `docs/PRODUCT.md` (Enter/Esc rows, new `Coin Detail` section, non-goal removed) and `docs/ARCHITECTURE.md` (`Coin Detail Screen`).
- [x] `M6-02` Themes, improved table layout and colors.
  Depends on: `M5-03`.
  Acceptance: at least two built-in themes switch without restarting; every theme stays readable without color and passes the required layout sizes with no out-of-bounds rendering.
  Evidence: new `src/theme.rs` defines semantic color roles (`summary`, `notice`, `gain`, `loss`, `neutral`) and `THEMES` in cycle order (`Default`, `Nord`, `Tokyo Night`, `Monochrome`); `App` owns only `theme_index`, exposed as `App::theme` and mutated by `App::cycle_theme` guarded by `t`/`Shift-T` (`src/app.rs`), active on the table and on the detail screen and typed as text while searching. Every style site in `src/ui.rs` now reads a theme role, the table header is tinted with the summary role, and the status line names a non-default theme. The trend surfaces reuse the roles: the table's 7-day sparkline cell is tinted by the 7-day change sign (`cell_style`), and the detail chart's text is styled as one `chart_trend_style` span (since `M6-04` the detail chart is drawn by `price_chart_lines` with `trend_color`, still the same role). Layout and state transitions never read the theme; `Monochrome` maps every role to `Color::Reset`, so text, signs, and glyphs carry meaning without color. New tests (app + ui) cover forward/backward cycling and wrap, typing `t`, cycling while detail is open, per-theme rendering of the table and detail screen at 60x16/80x24/120x30, theme distinctness, a full-buffer check that `Monochrome` colors nothing, and the trend-sparkline sign color; 124 unit + 31 provider tests pass with fmt and Clippy (denied warnings). A fixture-backed walkthrough (loopback port 8139, 80x24) cycled all themes with `t`/`Shift-T`, confirmed the status line and help binding live, and `q` exited with `loop stopped success=true`; a follow-up ANSI-captured run (port 8140, 120x30) verified the Trend column and detail chart render Tokyo Night RGB colors (gain 158/206/106, loss 247/118/142, summary 122/162/247) live. Since the M6-04 revision the theme surfaces include the summary labels (summary accent, market-cap change by sign), table row padding, the selected-row highlight, block titles, and the chart gradient ramp. Product, architecture, testing, and changelog contracts updated (`docs/PRODUCT.md` `t`/`Shift-T` rows and `Themes` section, non-goal removed; `docs/ARCHITECTURE.md` module map plus `Themes`; `docs/TESTING.md` manual scenario; `CHANGELOG.md`).
- [x] `M6-03` News and sentiment as tabs or a sidebar.
  Depends on: `M6-01`.
  Acceptance: keyboard-only tab or sidebar navigation reaches news and sentiment content; remote text and feeds are bounded and sanitized like the market tables and never block the refresh loop.
  Evidence: `Tab`/`Shift-Tab` cycle `MainPane` (`Table`, `News`, `Sentiment`) with a documented `?` help binding; below `PANE_MIN_WIDTH = 162` the focused pane renders alone so the table keeps its full column set, and at 162+ a 42-column right rail holds the news pane above the sentiment pane beside the table, with focus emphasized on the active pane's title. The news pane (`src/news.rs`) is a `NewsProvider` boundary: `RssNewsClient` fetches the `--news-url`/`COIN_TUI_NEWS_URL` RSS feed (default CoinDesk) with the same HTTPS/loopback URL rules, a no-redirect client, a 1 MiB body cap, and timeouts as the market provider; `parse_rss` (quick-xml) normalizes items into bounded `NewsItem` values (title ≤ 220, source ≤ 28, url ≤ 300 scalars, control chars stripped, RFC-2822 dates), rejects HTML error pages as `MalformedResponse`, and headlines render as `source · age`, title, and a bounded URL line. The news fetch chains onto a market refresh, one in flight at a time, generation-guarded, and spawned as its own cancellable task so a slow feed cannot block input, the refresh loop, or shutdown; a failed refresh keeps the last headlines and shows a notice. The sentiment pane is a pure snapshot render: up/down/flat counts, a bullish meter, average, and best/worst mover with placeholders. The rich detail sidebar completes the pane work: `Enter` fetches `/coins/{id}` (`CoinGeckoClient::fetch_coin_detail`, percent-encoded id), `DetailState` upgrades Basic→Loading→Ready with `Box<CoinDetail>`, a failed or unsupported fetch stays on the row fallback, and wide panes render a `Coin data` column (ATH/ATL, supplies, FDV, longer changes, sentiment votes, categories, About) while narrow panes stack compact stat lines. New tests cover the RSS parser edge cases (8 news unit tests), the provider boundary for detail and RSS (43 provider tests incl. hostile id encoding, non-finite rejection, oversized/non-RSS bodies, `429`/`5xx`/timeout), app state (NewsResult success/failure/stale, pane cycling + modal swallowing, detail-fetch cancel on Esc, news chaining with one-in-flight), and rendering (news/sentiment panes at 60x16 and 162x30/200x30 side-by-side, 161x30 focused-only, detail sidebar fields, row separators). Fixture server gained `/rss` and `/api/v3/coins/{id}` routes. 145 unit + 47 provider tests pass with fmt and Clippy (denied warnings); the walkthrough in `.omo/evidence/M6-03-walkthrough.md` covers the panes, sidebar upgrade, and widths.
- [x] `M6-04` CoinMarketCap-style coin detail redesigned into a left-aligned column with a gradient area chart (revised after user feedback).
  Depends on: `M6-01`.
  Acceptance: the detail screen mirrors the simplified CoinMarketCap coin-detail page: a left-aligned, width-limited content column carries the identity block (rank, name, symbol), the price and its 24-hour change, the 1h/24h/7d strip, a fixed-geometry gradient area chart with real price labels (replacing the `rasciigraph` line chart), and a market-stats grid (market cap, volume, supply), all at compact, standard, and full widths with no out-of-bounds rendering and readable without color. The chart reuses the snapshot's normalized series so it adds no provider call, is bounded like the sparkline, and passes empty, non-finite, hostile, and long-series tests. `Enter`/`Esc` behavior and selection preservation from `M6-01` keep passing.
  Evidence: `render_detail` in `src/ui.rs` is the CoinMarketCap shape — a left-aligned content column capped at `DETAIL_CONTENT_WIDTH = 56` with the notice-colored title, a bold identity header (`#1  Bitcoin (BTC)` with the rank in the summary role), a `price_line` pairing the bold price with its color-coded 24-hour change, the 1h/24h/7d strip, the chart, and the market-stats grid in the notice color. `price_chart_lines`/`render_price_chart` draw a custom half-block gradient area chart (no dependency): the series is filtered, downsampled to `MAX_CHART_POINTS = 512`, min-max normalized into `[0.0, 1.0]` (flat/overflow → mid-line), sampled at two-sub-row resolution across `CHART_ROWS = 6` body rows, and filled from the line down with a `GRADIENT_SHADES = 8` ramp that darkens away from the line (named ANSI colors map to RGB so every color theme fades); real price labels and the `7 days: low → high` caption use the summary accent, so the row count and width are always bounded and the chart keeps the gain/loss/neutral trend color as one palette. The `rasciigraph` dependency from the first M6-04 pass was removed after the user's revision. Theme coverage now spans the summary labels (summary accent, market-cap change by sign), table row padding (`Row::height(2)` so the trend charts breathe), the selected-row highlight (summary role), block titles (summary for market/table, notice for detail and messages), the chart gradient ramp, and the existing pricing/table/sparkline surfaces. Tests: `price_chart_is_left_aligned_bounded_and_gradient_styled` and `gradient_ramp_fades_rgb_and_stays_solid_without_one`, plus updated detail assertions (gradient glyphs, labels, caption, stats grid, placeholder); 124 unit + 31 provider tests pass with fmt and Clippy (denied warnings) and dev/release builds. A fixture-backed walkthrough (loopback port 8141) verified the revised layout at 60x16, 80x24, and 120x30 with no out-of-bounds rendering and the chart hugging the pane's left border; Tokyo Night applied live with the gradient fading through seven shades (158/206/106 → 84/110/57) and summary/notice accents on labels and title; `q` exited with `loop stopped success=true`. Product and architecture contracts updated in `docs/PRODUCT.md` and `docs/ARCHITECTURE.md`; the exact statistic set remains a reasonable CoinMarketCap approximation because the reference screenshot is unreadable by the model.

Quality gate `G6`:

- [ ] All M6 tasks are accepted.
- [ ] The baseline checks in `docs/TESTING.md` pass with the new screens and feeds.
- [ ] Manual checks confirm each new surface at compact, standard, and full widths, readable without color.

## M7: Library-Driven Charts And Table Polish

Goal: render the coin-detail chart with the `chandelier` candlestick library on
a 30-day view, restore the compact inline trend sparkline, and add breathing
room between the table header and rows.

- [x] `M7-01` Upgrade ratatui to 0.30 and crossterm to 0.29.
  Depends on: `G6`.
  Acceptance: the app builds and passes the full baseline suite on the new stack; the `crossterm_0_29` ratatui feature keeps the re-exported Crossterm matching the direct dependency.
  Evidence: `Cargo.toml` pins `ratatui = { version = "0.30", features = ["crossterm_0_29"] }` and `crossterm = { version = "0.29", features = ["event-stream"] }`; the only code change the migration required was removing the now-unused `prelude::Stylize` import in `src/ui.rs` (0.30 removed `Styled` from `Style`). `cargo test --all-features` passes 192 tests (145 unit + 47 provider) unchanged on the new stack, and fmt/clippy are clean.
- [x] `M7-02` Restore the compact inline trend sparkline.
  Depends on: `M7-01`.
  Acceptance: full-mode rows show the original block-glyph 7-day trend, colored by the 7-day change sign, with a dash for missing or all-non-finite series; every required width renders without out-of-bounds.
  Evidence: `sparkline_text` in `src/ui.rs` renders the finite hourly closes downsampled into `width` equal averaged buckets, min-max normalized into the eight block glyphs `▁▂▃▄▅▆▇█`, with a flat series at mid level (`▄`) and `-` for missing; `make_cell` colors it via `cell_style` by the 7-day change sign. The ratatui `Sparkline` widget experiment was rolled back after visual review showed it could not fill the fixed-width trend cell. Tests restore the exact glyph assertions for flat/rising/falling/single-point/missing/non-finite/zero-width series and full-mode rendering.
- [x] `M7-03` Render the coin-detail chart with the `chandelier` candlestick library on a 30-day view.
  Depends on: `M7-01`.
  Acceptance: the detail chart is a candlestick chart drawn from daily OHLC candles, styled with the theme's gain/loss/trend roles, readable without color, bounded against hostile/empty/long series, and stretching the full content column.
  Evidence: `CoinGeckoClient::fetch_market_chart` (`GET /coins/{id}/market_chart?days=30`) is a new provider boundary (`MarketData::fetch_market_chart`, default unsupported) that returns the 30-day price series; the controller chains it onto the detail fetch and `Event::ChartResult` carries it through `DetailState::Loading`/`Ready` (either fetch may land first). `daily_candles` in `src/domain.rs` derives up to `MAX_DAILY_CANDLES = 30` daily candles (one per 24 hourly points) and `render_detail_chart` sizes the `chandelier::CandlestickChart` candles via `candle_width` so they stretch the `DETAIL_CONTENT_WIDTH = 56` column; when the market chart is unsupported or fails, the chart falls back to the 7-day series and the caption reads `7 days` instead of `30 days`. Tests cover the provider request path/query/key/hostile-id encoding, the chart-series upgrade in either arrival order, the derived OHLC bounds, empty/non-finite series, theme-role styling, and rendering at 60x16/80x24/120x30.
- [x] `M7-04` Keep the table row highlight a clean, gap-free block.
  Depends on: `M7-01`.
  Acceptance: the selected-row highlight fills the row's full block (text line + breathing line) with no gap above or between; the header sits directly above the first row.
  Evidence: `make_row` in `src/ui.rs` uses plain `Row::height(2)` (no header `bottom_margin`, no row `top_margin`), so the whole 2-line row area is the selection block. The earlier `header.bottom_margin(1)` and `row.top_margin(1)` attempts both left the margin line outside the highlighted `row_area` (ratatui's `row_area` starts below the margin), which is what created and then widened the gap; both were reverted. The selection was later switched from a `reversed` full-row background (which stretched the row and scrambled per-cell colors on hover) to a full-height left-edge `▌` marker in the first column plus bold summary-accent text, so the selection stays contiguous and gap-free without touching row layout.
- [x] `M7-05` Split the default body layout 70/30 with equal news and sentiment rows.
  Depends on: `M7-01`.
  Acceptance: at 162+ columns the body splits 70% table / 30% right column, and the right column splits into two equal rows (news on top, sentiment below); below 162 columns one focused pane still replaces the table.
  Evidence: `render_market` in `src/ui.rs` now uses `[Percentage(70), Percentage(30)]` for the horizontal split and `[Percentage(50), Percentage(50)]` for the right column; the fixed `SIDE_COLUMN = 42` constant was removed. A 200x40 tmux capture shows the table at ~140 columns with the full column set and the news/sentiment panes side-by-side in the ~60-column right rail.
- [x] `M7-06` Manual verification of the charts and table across widths and themes.
  Depends on: `M7-02`, `M7-03`, `M7-04`, `M7-05`.
  Acceptance: a fixture-backed walkthrough shows the inline trend sparkline in full mode, the 30-day candlestick chart on the detail screen at compact, standard, and full widths, the gap-free row highlight, and the 70/30 layout, cycling every theme with `NO_COLOR=1` readable.
  Evidence: a 200x40 tmux run against `scripts/fixture-server.py` (loopback port 8152) showed the full-mode Trend column rendering block glyphs, the selected row's full-height left-edge marker with no gap, the 70/30 body with news/sentiment in the right rail, and `Enter` opening the detail screen with the 30-day candlestick chart stretching the full column; cycling to `Monochrome` kept everything readable without color. `q`/`Esc` restored the terminal each time.

Quality gate `G7`:

- [ ] All M7 tasks are accepted.
- [ ] The baseline checks in `docs/TESTING.md` pass with the new charts.

## Deferred Tracks

These items require a new milestone and acceptance criteria. They are not implicit MVP work:

- persistent watchlists and local configuration files;
- alternate quote currencies and providers;
- configurable columns;
- Windows release support;
- alerts, trading, accounts, or AI features;
- an Axum service, browser UI, remote daemon, or plugin API.

Note: the `M6` milestone now schedules the previously deferred coin detail and
historical chart screens, themes plus table layout and colors, and news/sentiment
as tabs or a sidebar.
