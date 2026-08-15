# Convenience recipes. Run `just` (no args) for the menu.
# Requires `just` (https://github.com/casey/just) — pre-installed in the nix shell.

# Default: list recipes
default:
    @just --list

# Run the full playwright suite (headless Chromium).
test:
    cd tests && pnpm install --silent && pnpm test

# Run with a visible browser window — debug-mode.
test-headed:
    cd tests && pnpm install --silent && pnpm test:headed

# Open Playwright's interactive UI runner.
test-ui:
    cd tests && pnpm install --silent && pnpm test:ui

# Run only one test by name fragment, e.g.: `just test-only "cursor stays"`.
test-only PATTERN:
    cd tests && pnpm install --silent && npx playwright test -g "{{PATTERN}}"

# Re-run a previously failed test with its trace open.
trace:
    cd tests && npx playwright show-trace test-results/*/trace.zip

# Workspace-wide Rust tests.
unit:
    cargo test --workspace

# Cargo check on every target we care about.
check:
    cargo check --workspace
    cargo check -p playground --target wasm32-unknown-unknown

# Start the desktop playground in dev mode.
dev:
    dx serve

# Start the web playground (matches what playwright uses).
dev-web:
    dx serve --platform web --port 8090
