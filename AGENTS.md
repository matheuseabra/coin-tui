# Coin TUI Agent Guide

## Read First

- Read `docs/PRODUCT.md` before changing scope, user behavior, controls, layouts, or user-facing states.
- Read `docs/ARCHITECTURE.md` before changing system boundaries, dependencies, data models, or data flow.
- Read `docs/ROADMAP.md` before selecting work. It owns task state, dependencies, acceptance criteria, and milestone gates.
- Read `docs/WORKFLOW.md` before delegating work or changing task status.
- Read `docs/TESTING.md` before adding tests, changing quality commands, or accepting a quality gate.

## Engineering Rules

- Use stable Rust, Ratatui, Crossterm, Tokio, Reqwest with rustls, and Serde.
- Keep one binary crate until a second consumer makes a library boundary useful.
- Keep rendering pure: widgets read state and emit no I/O.
- Keep terminal input and network I/O behind typed events. The update function owns state transitions.
- Pass owned market snapshots through Tokio channels. Avoid shared mutable application state.
- Use provider response types only at the API boundary. Convert them into provider-independent domain types.
- Preserve the last successful snapshot when refresh fails and mark it stale.
- Treat all remote strings and numbers as untrusted. Bound text, reject non-finite values, and render missing values as `-`.
- Restore the terminal on normal exit, error, panic, and cancellation.
- Keep logs out of the alternate screen. Write diagnostics to a file or display concise status text.
- Keep the smallest correct dependency set. Record every new production dependency and its purpose in `docs/ARCHITECTURE.md`.

## Completion Contract

A task is complete only when its implementation, focused tests, relevant documentation, and acceptance criteria are complete. Update its checkbox and evidence in `docs/ROADMAP.md` in the same change. Report commands that actually ran and any remaining risk.
