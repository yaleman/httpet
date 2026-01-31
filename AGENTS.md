# Repository Guidelines

## Service Overview

- This site powers `httpet.org` and animal subdomains like `dog.httpet.org`.
- Requests to `/<status>` should return a themed response for that animal (e.g., `dog.httpet.org/500`).
- Subdomain-specific behavior should be driven by the request host (e.g., `Host: dog.httpet.org`).

## Project Structure & Module Organization

- `src/main.rs` wires the CLI and starts the Axum server.
- `src/lib.rs` exposes modules (`cli`, `config`, `db`, `web`).
- `src/web/` contains HTTP routing/handlers; add new routes here.
- `src/cli.rs` defines CLI flags and env var bindings (e.g., `HTTPET_PORT`).
- `src/config.rs` configures logging.
- `src/db/` is reserved for database code; `src/db/migrations/` contains SeaORM migrations.
- `target/` is build output and should not be edited or committed.

## Build, Test, and Development Commands

- `cargo run -- --debug --port 3000 --listen-address 127.0.0.1`: run the server with CLI flags.
- `cargo test`: run Rust unit tests.
- `cargo clippy --all-features`: lint the codebase.
- `just run`: wrapper for `cargo run`.
- `just test`: preferred test runner (wraps `cargo test`).
- `just check`: run codespell, clippy, tests, and markdown formatting checks.
  - Always run `just check` and ensure it passes before considering work complete.

## Coding Style & Naming Conventions

- Rust 2024 edition; follow standard Rust style and module naming.
- Run `cargo fmt` (rustfmt defaults) before committing.
- Use `snake_case` for modules/functions and `UpperCamelCase` for types.
- Keep handlers in `src/web/` and CLI options in `src/cli.rs` to avoid drift.
- Inline CSS is not allowed; add styles to `static/styles.css` and reference it from templates.

## Testing Guidelines

- Use Rust’s built-in test framework with `#[cfg(test)]` modules near code.
- Name tests descriptively (e.g., `root_handler_returns_200`).
- Add tests alongside new routes or CLI behaviors.
- Use an in-memory SQLite database for tests (`sqlite::memory:`).

## Commit & Pull Request Guidelines

- Git history is minimal (single “initial commit”), so no established convention.
- Use concise, imperative commit subjects (e.g., “Add status route”).
- PRs should include: summary, rationale, test coverage notes, and any relevant CLI/env changes.

## Configuration & Runtime Notes

- CLI flags map to env vars: `HTTPET_PORT`, `HTTPET_LISTEN_ADDRESS`, and `HTTPET_BASE_DOMAIN`.
- Logging level is controlled by `--debug` (Info by default, Debug when set).
- `docker-compose.yml` runs `ghcr.io/yaleman/httpet:latest` (built by GitHub Actions) and mounts `./images` to `/images` in the container.

## Documentation Hygiene

- Update this file whenever the site design, routing, or content expectations change.
