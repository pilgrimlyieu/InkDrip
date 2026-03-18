set dotenv-load
set shell := ["bash", "-c"]

# -----------------------------------------------------------------------------
# Aliases
# -----------------------------------------------------------------------------

alias d  := dev
alias b  := build
alias bd := build-debug
alias cl := clean
alias l  := lint
alias f  := format
alias c  := check
alias fi := fix
alias t  := test
alias ta := test-all
alias r  := run

# -----------------------------------------------------------------------------
# Core Development & Build
# -----------------------------------------------------------------------------

# List available commands
@_default:
    just --list --unsorted

# Run the server in development mode (debug build)
@dev:
    cargo run --bin inkdrip-server

# Run the server in production mode (release build)
@run:
    cargo run --release --bin inkdrip-server

# Run the CLI in development mode (pass extra args after --)
@cli-dev *args:
    cargo run --bin inkdrip-cli -- {{ args }}

# Run the CLI in production mode (pass extra args after --)
@cli *args:
    cargo run --release --bin inkdrip-cli -- {{ args }}

# Build all crates (release)
@build:
    cargo build --release --workspace

# Build all crates (debug)
@build-debug:
    cargo build --workspace

# Clean build artifacts
[confirm: "Remove all build artifacts? (target/)"]
@clean:
    cargo clean

# -----------------------------------------------------------------------------
# Code Quality
# -----------------------------------------------------------------------------

# Format all code
@format:
    cargo fmt --all

# Check formatting without writing
@format-check:
    cargo fmt --all --check

# Run Clippy lints
@lint:
    cargo clippy --workspace --all-targets

# Run Clippy and auto-fix
@fix:
    cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged

# Type-check all crates
@check:
    cargo check --workspace --all-targets

# Run all quality checks: format + check + lint
@check-all:
    just format-check
    just check
    just lint

# -----------------------------------------------------------------------------
# Testing
# -----------------------------------------------------------------------------

# Run lib tests
@test *args:
    cargo test --workspace --lib {{ args }}

# Run all tests (lib + integration + doc)
@test-all *args:
    cargo test --workspace {{ args }}

# Run tests with stdout output visible
@test-verbose *args:
    cargo test --workspace -- --nocapture {{ args }}

# -----------------------------------------------------------------------------
# Utilities
# -----------------------------------------------------------------------------

# Update all dependencies
@update:
    cargo update --workspace

# Show dependency tree
@deps:
    cargo tree --workspace

# Show outdated dependencies (requires cargo-outdated)
@outdated:
    cargo outdated --workspace

# Watch and rebuild on file changes (requires cargo-watch)
@watch:
    cargo watch -x "run --bin inkdrip-server"

# Watch and run tests on file changes (requires cargo-watch)
@watch-test:
    cargo watch -x "test --workspace --lib"

# Generate and open documentation
@doc:
    cargo doc --workspace --no-deps --open

# Start services with Docker Compose
@docker-up:
    docker compose up -d

# Stop Docker Compose services
@docker-down:
    docker compose down

