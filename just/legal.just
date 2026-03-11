# Add legal headers to all source files.
legal-headers:
    @printf "\033[0;34m🏢 Legal headers should be added manually or via reuse tool\033[0m\n"
    @printf "\033[1;33m⚠️  Run 'just legal-check' to verify compliance\033[0m\n"

# Check REUSE compliance.
legal-check:
    @printf "\033[0;34m🔍 Checking REUSE compliance...\033[0m\n"
    @if command -v reuse >/dev/null 2>&1; then \
        reuse lint; \
    elif [ -f "/Users/rnio/Library/Python/3.12/bin/reuse" ]; then \
        /Users/rnio/Library/Python/3.12/bin/reuse lint; \
    else \
        printf "\033[0;31m❌ reuse tool not installed. Install with: just install-legal-tools\033[0m\n"; \
        exit 1; \
    fi

# Fix REUSE compliance issues.
legal-fix:
    @printf "\033[0;34m🔧 Fixing REUSE compliance...\033[0m\n"
    @if command -v reuse >/dev/null 2>&1; then \
        reuse annotate --license "MPL-2.0" --copyright "Robert Nio" --template rust src/**/*.rs crates/**/*.rs; \
    elif [ -f "/Users/rnio/Library/Python/3.12/bin/reuse" ]; then \
        /Users/rnio/Library/Python/3.12/bin/reuse annotate --license "MPL-2.0" --copyright "Robert Nio" --template rust src/**/*.rs crates/**/*.rs; \
    else \
        printf "\033[0;31m❌ reuse tool not installed. Install with: just install-legal-tools\033[0m\n"; \
    fi

# Install legal compliance tools.
install-legal-tools:
    @printf "\033[0;34m📦 Installing legal compliance tools...\033[0m\n"
    @if command -v pipx >/dev/null 2>&1; then \
        printf "\033[1;33mUsing pipx (recommended for Python tools)...\033[0m\n"; \
        pipx install reuse; \
    elif command -v pip >/dev/null 2>&1; then \
        printf "\033[1;33mUsing pip with --user flag...\033[0m\n"; \
        pip install --user reuse; \
    elif command -v pip3 >/dev/null 2>&1; then \
        printf "\033[1;33mUsing pip3 with --user flag...\033[0m\n"; \
        pip3 install --user reuse; \
    else \
        printf "\033[0;31m❌ No Python package manager found\033[0m\n"; \
        printf "\033[1;33mPlease install pipx: brew install pipx\033[0m\n"; \
        exit 1; \
    fi

# Complete legal setup.
legal-setup: install-legal-tools legal-headers legal-check
    @printf "\033[0;32m🎉 Legal setup complete!\033[0m\n"
    @printf "\033[0;32m📋 All files now have proper legal headers\033[0m\n"
    @printf "\033[0;32m✅ REUSE compliance verified\033[0m\n"
    @echo ""
    @printf "\033[0;34mLegal Information:\033[0m\n"
    @echo "  Copyright: Robert Nio"
    @echo "  Contact: 50460704+githubrobbi@users.noreply.github.com"
    @echo "  License: MPL-2.0"

