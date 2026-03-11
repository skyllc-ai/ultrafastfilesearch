# Shared settings, flags, helpers, and bootstrap.

set shell := ["bash", "-euo", "pipefail", "-c"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

export FORCE_COLOR := "1"
export CLICOLOR_FORCE := "1"
export TERM := "xterm-256color"
export COLORTERM := "truecolor"
export CARGO_TERM_COLOR := "always"
export CARGO_INCREMENTAL := "1"
export CARGO_NET_RETRY := "3"
export CARGO_HTTP_TIMEOUT := "30"
export CARGO_HTTP_MULTIPLEXING := "true"
export RUSTC_WRAPPER := ""

common_flags := "-D clippy::pedantic -D clippy::nursery -D clippy::cargo -A clippy::multiple_crate_versions -W clippy::panic -W clippy::todo -W clippy::unimplemented -D warnings"
prod_flags := common_flags + " -W clippy::unwrap_used -W clippy::expect_used -W clippy::missing_docs_in_private_items"
test_flags := common_flags + " -A clippy::unwrap_used -A clippy::expect_used"

# Install cargo tool if missing (idempotent)
_install-if-missing TOOL CRATE:
    #!/usr/bin/env bash
    if ! command -v {{ TOOL }} >/dev/null 2>&1; then
        printf "\033[0;34m📦 Installing {{ CRATE }}...\033[0m\n"
        if command -v cargo-binstall >/dev/null 2>&1; then
            cargo binstall {{ CRATE }} --no-confirm --quiet
        else
            cargo install {{ CRATE }} --locked --quiet
        fi
        printf "\033[0;32m✅ {{ TOOL }} installed\033[0m\n"
    else
        printf "\033[0;32m✅ {{ TOOL }} already installed (skip)\033[0m\n"
    fi

# Install rustup component if missing (idempotent)
_install-component COMPONENT:
    #!/usr/bin/env bash
    if ! rustup component list --installed | grep -q "{{ COMPONENT }}"; then
        printf "\033[0;34m📦 Installing {{ COMPONENT }} component...\033[0m\n"
        rustup component add {{ COMPONENT }}
        printf "\033[0;32m✅ {{ COMPONENT }} component installed\033[0m\n"
    else
        printf "\033[0;32m✅ {{ COMPONENT }} component already installed (skip)\033[0m\n"
    fi

# Universal development environment setup
setup:
    #!/usr/bin/env bash
    printf "\033[0;34m🔧 Universal Smart Development Environment Setup\033[0m\n"
    printf "\033[0;34m   Installing essential Rust development tools...\033[0m\n"
    just _install-if-missing cargo-binstall cargo-binstall
    just _install-if-missing cargo-watch cargo-watch
    just _install-if-missing cargo-nextest cargo-nextest
    just _install-if-missing cargo-llvm-cov cargo-llvm-cov
    just _install-if-missing cargo-deny cargo-deny
    just _install-if-missing cargo-audit cargo-audit
    just _install-if-missing cargo-geiger cargo-geiger
    just _install-if-missing cargo-outdated cargo-outdated
    just _install-if-missing cargo-udeps cargo-udeps
    just _install-if-missing cargo-machete cargo-machete
    just _install-if-missing cargo-expand cargo-expand
    just _install-if-missing rust-script rust-script
    just _install-if-missing cargo-criterion cargo-criterion
    just _install-if-missing cargo-update cargo-update
    just _install-if-missing sccache sccache
    just _install-component llvm-tools-preview
    just _install-component miri
    printf "\033[0;32m✅ Development environment setup complete!\033[0m\n"
    printf "\033[0;34mNext steps:\033[0m\n"
    printf "  • Run 'just go' to validate everything safely\n"
    printf "  • Run 'just phase2-ship' for the explicit ship lane\n"
    printf "  • Run 'just ci-analyze' for CI pipeline optimization\n"

