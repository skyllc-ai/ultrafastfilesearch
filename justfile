# TTAPI - Modern Rust Development Workflow (Cross-Platform & Educational)
#
# ═══════════════════════════════════════════════════════════════════════════════
# 🎓 LEARNING GUIDE: Professional Rust Development Workflow Masterclass
# ═══════════════════════════════════════════════════════════════════════════════
#
# This justfile is designed as both a working build system AND an educational
# resource for professional Rust development. Every design decision, workaround,
# and quirk is documented to help developers understand the complexities of
# cross-platform development and professional workflow design.
#
# 📚 WHAT YOU'LL LEARN:
# • How to handle shell differences between Windows PowerShell, Git Bash, and Unix
# • Why certain approaches fail and what works instead (with examples)
# • Cross-platform binary handling and PATH management strategies
# • Professional two-phase development workflow design (fast-fail methodology)
# • Advanced Rust tool ecosystem management (13+ tools with idempotent installation)
# • Color system implementation across different terminals and environments
# • Platform detection strategies and their trade-offs
# • CI pipeline benchmarking and performance optimization techniques
# • Security analysis and dependency optimization workflows
#
# 🔍 HOW TO READ THIS FILE:
# • Each section explains WHY before showing HOW
# • Failed approaches are documented to prevent repeating mistakes
# • Cross-references help you understand relationships between concepts
# • Examples show what happens when you don't follow the patterns
# • Educational comments explain the reasoning behind every design decision
#
# ═══════════════════════════════════════════════════════════════════════════════
# 📋 CRITICAL DESIGN DECISIONS & PLATFORM QUIRKS
# ═══════════════════════════════════════════════════════════════════════════════
#
# This justfile has been battle-tested across Windows, macOS, and Linux with
# specific workarounds for cross-platform compatibility. Each quirk below
# represents hours of debugging and testing. Please read before making changes.
#
# 🔧 SHELL CONFIGURATION STRATEGY:
# ─────────────────────────────────────────────────────────────────────────────
# PROBLEM: Different platforms use different shells with incompatible syntax
# • Windows: PowerShell (doesn't understand bash syntax like ||, &&, if)
# • macOS/Linux: bash/zsh (don't understand PowerShell syntax)
# • Git Bash on Windows: bash-compatible but path resolution issues
#
# SOLUTION: Hybrid approach with explicit shell control
# • set shell := ["bash", "-euo", "pipefail", "-c"] - Global bash preference
#   - "-e" = exit on any error (fast-fail behavior)
#   - "-u" = exit on undefined variables (catch typos)
#   - "-o pipefail" = exit if any command in pipeline fails
# • set windows-shell := ["powershell.exe", ...] - Fallback for Git Bash issues
# • #!/usr/bin/env bash shebang on ALL recipes with logic/colors
#
# WHY THIS WORKS:
# • Forces bash execution even when just defaults to PowerShell
# • Ensures consistent behavior across all platforms
# • Provides fast-fail behavior that stops on first error
# • Catches undefined variable typos immediately
#
# WHAT FAILS WITHOUT THIS:
# • PowerShell: "if ! command -v tool" → syntax error
# • PowerShell: "command && other_command" → syntax error
# • Mixed shells: inconsistent variable expansion and error handling
# • Silent failures: commands continue executing after errors
#

set shell := ["bash", "-euo", "pipefail", "-c"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# 🎨 COLOR SYSTEM IMPLEMENTATION:
# ─────────────────────────────────────────────────────────────────────────────
# PROBLEM: Cross-platform color support is surprisingly complex
# • just variable expansion ({{GREEN}}) fails on some platforms
# • @echo shows raw ANSI codes instead of colors
# • Different terminals have different color support
# • NO_COLOR environment variable must be respected
#
# FAILED APPROACHES (documented to prevent repetition):
# ❌ Color variables with just expansion: {{GREEN}} shows as literal "{{GREEN}}"
#    Example: echo -e "{{GREEN}}text{{NC}}" → displays "{{GREEN}}text{{NC}}"
# ❌ @echo with ANSI codes: displays "\033[0;32m" instead of green text
#    Example: @echo "\033[0;32mtext\033[0m" → shows escape codes literally
# ❌ Dynamic color detection: inconsistent between Windows/Unix
#    Example: tput colors fails on some Windows terminals
# ❌ Shell-specific color commands: breaks cross-platform compatibility
#    Example: PowerShell Write-Host vs bash echo -e
#
# ✅ WORKING SOLUTION: Direct ANSI codes with @printf
# • Hardcoded ANSI escape sequences: \033[0;34m (blue), \033[0;32m (green)
# • @printf with \n for consistent newline handling across platforms
# • Environment variables to force color support in all tools
# • Colors used consistently throughout:
#   - \033[0;34m = Blue (info/steps/progress)
#   - \033[0;32m = Green (success/completion)
#   - \033[1;33m = Yellow (warnings/attention)
#   - \033[0;31m = Red (errors/failures)
#   - \033[0m = Reset to default (always used to close colors)
#
# WHY HARDCODED ANSI CODES:
# • Variable expansion fails in just on some platforms
# • Direct codes work consistently across all terminals
# • No dependency on external tools or shell features
# • Predictable behavior in CI/CD environments
#
# ═══════════════════════════════════════════════════════════════════════════════
# 🎨 COLOR ENVIRONMENT - Professional Terminal Output
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Ensures consistent color output across all platforms and terminals
# • FORCE_COLOR: Forces color output even in non-TTY environments (CI/CD)
# • CLICOLOR_FORCE: macOS/BSD color forcing standard
# • TERM settings: Ensures terminal capabilities are properly detected
# • COLORTERM: Indicates truecolor support to applications
# • CARGO_TERM_COLOR: Forces Cargo to use colors in all environments
# • NO_COLOR support: Automatically respected by compliant tools

export FORCE_COLOR := "1"
export CLICOLOR_FORCE := "1"
export TERM := "xterm-256color"
export COLORTERM := "truecolor"
export CARGO_TERM_COLOR := "always"

# ═══════════════════════════════════════════════════════════════════════════════
# 🔧 ENHANCED FORMATTING WORKFLOW - Rust Master Standards
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Three-step process ensures maximum code quality before formatting
#
# STEP 1: cargo fix --all-targets --broken-code --allow-dirty
# • Applies automatic compiler fixes for edition changes
# • Fixes deprecated syntax and outdated patterns
# • Resolves compilation errors that can be auto-fixed
# • Essential for maintaining compatibility across Rust editions
# • --allow-dirty: Works with uncommitted changes (development workflow)
#
# STEP 2: cargo clippy --fix --allow-dirty --allow-staged
# • Applies ALL safe Clippy suggestions automatically
# • Improves code quality and performance
# • Fixes common patterns and idioms (const fn, redundant patterns, etc.)
# • --allow-dirty: Works with uncommitted changes
# • --allow-staged: Works with staged changes
# • Aggressive mode: Fixes everything that's safe to auto-fix
#
# STEP 3: cargo fmt --all
# • Applies consistent formatting across entire workspace
# • Ensures uniform code style
# • Final step after all fixes are applied
#
# BENEFITS:
# • Reduces manual code review overhead
# • Catches issues early in development cycle
# • Maintains consistent code quality standards
# • Automates tedious manual fixes

# ═══════════════════════════════════════════════════════════════════════════════
# 🚀 COMPILATION OPTIMIZATION - Performance Enhancement
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Optimize compilation speed and caching for faster development workflow
# • CARGO_INCREMENTAL: Enable incremental compilation (faster rebuilds)
# • CARGO_NET_RETRY: Retry network operations (more reliable downloads)
# • CARGO_HTTP_TIMEOUT: Increase timeout for large downloads
# • CARGO_HTTP_MULTIPLEXING: Enable HTTP/2 multiplexing for faster downloads
# • RUSTC_WRAPPER: Unset by default to prevent sccache interference
#   - sccache will be enabled explicitly in recipes that benefit from it
#   - Prevents issues with cargo clean and other non-compilation commands

export CARGO_INCREMENTAL := "1"
export CARGO_NET_RETRY := "3"
export CARGO_HTTP_TIMEOUT := "30"
export CARGO_HTTP_MULTIPLEXING := "true"
export RUSTC_WRAPPER := ""

# 🔄 TOOL INSTALLATION PHILOSOPHY:
# ─────────────────────────────────────────────────────────────────────────────
# PROBLEM: Installing multiple tools can fail in complex ways
# • Network issues during installation
# • Dependency conflicts between tools
# • Platform-specific installation methods
# • Version compatibility issues
# • Compilation time for complex tools
#
# SOLUTION: Isolated, idempotent installation pattern
# • Individual helper function calls instead of bash loops (Windows compatibility)
# • Each tool installation is isolated (one failure doesn't break others)
# • Idempotent design: tools only installed if missing (safe to re-run)
# • cargo-binstall preference: faster binary downloads vs compilation
# • Graceful fallbacks: cargo install if cargo-binstall unavailable
#
# WHY NOT BASH LOOPS:
# • FAILED: for tool in $tools; do just _install $tool; done
# • PROBLEM: PowerShell doesn't understand bash for-loop syntax
# • SOLUTION: Explicit individual calls - more verbose but cross-platform
# • BENEFIT: Clear error messages showing exactly which tool failed
#
# ═══════════════════════════════════════════════════════════════════════════════
# 🛠️ TOOL ECOSYSTEM - Professional Rust Development Stack
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Centralized tool management with comprehensive development capabilities
#
# CORE TOOLS (Essential for professional development):
# • cargo-binstall: Faster binary installations (downloads pre-built binaries)
#   - Reduces installation time from minutes to seconds
#   - Fallback to cargo install if binary unavailable
# • cargo-nextest: Modern test runner with better output and parallelism
#   - Faster test execution with better reporting
#   - Superior parallel execution compared to cargo test
# • cargo-llvm-cov: Code coverage with LLVM integration
#   - More accurate coverage than tarpaulin
#   - Better integration with modern Rust toolchain
#
# SECURITY & ANALYSIS TOOLS:
# • cargo-deny: Comprehensive dependency analysis and security
#   - License compliance checking
#   - Dependency graph analysis
#   - Security vulnerability detection
# • cargo-audit: Security vulnerability scanning
#   - RustSec advisory database integration
#   - Automated security reporting
# • cargo-geiger: Unsafe code detection and analysis
#   - Quantifies unsafe code usage
#   - Helps maintain memory safety guarantees
#
# DEPENDENCY MANAGEMENT:
# • cargo-outdated: Dependency update checking
#   - Identifies outdated dependencies
#   - Helps maintain security and performance
# • cargo-udeps: Unused dependency detection
#   - Finds dependencies that can be removed
#   - Reduces compilation time and binary size
# • cargo-machete: Automatic unused dependency removal
#   - Automated cleanup of unused dependencies
#   - Maintains clean Cargo.toml files
#
# DEBUGGING & DEVELOPMENT:
# • cargo-expand: Macro expansion for debugging
#   - Shows what macros expand to
#   - Essential for complex macro debugging
# • cargo-watch: File watching for development
#   - Automatic rebuilds on file changes
#   - Improves development workflow
# • rust-script: Execute Rust files as scripts
#   - Enables Rust scripting capabilities
#   - Used by our version management system
#
# PERFORMANCE & BENCHMARKING:
# • cargo-criterion: Advanced benchmarking with statistical analysis
#   - Professional performance measurement
#   - Statistical significance testing
#   - HTML report generation
#
# RUSTUP COMPONENTS:
# • llvm-tools-preview: LLVM tools for coverage and profiling
#   - Required for cargo-llvm-cov
#   - Enables advanced profiling capabilities
# • miri: Undefined behavior detection
#   - Catches memory safety violations
#   - Essential for unsafe code validation
#

all_tools := "cargo-binstall cargo-watch cargo-nextest cargo-llvm-cov cargo-deny cargo-audit cargo-outdated cargo-udeps cargo-machete cargo-expand cargo-geiger cargo-criterion cargo-update sccache rust-script"
rust_components := "llvm-tools-preview miri"

# Common clippy flags - Rust master approach
# 🚀 TWO-PHASE WORKFLOW DESIGN:
# ─────────────────────────────────────────────────────────────────────────────
# PHILOSOPHY: Separate testing from deployment for professional development
#
# PHASE 1 (Testing & Validation): Comprehensive quality assurance
# • Format code automatically (rustfmt with project settings)
# • Run all tests with coverage (nextest + llvm-cov for efficiency)
# • Run documentation tests separately (avoid duplicate compilation)
# • Ultra-strict linting for production code (unwrap/expect forbidden)
# • Pragmatic linting for test code (allows unwrap/expect for clarity)
# • Legal compliance checking (REUSE standard)
#
# PHASE 2 (Build & Deploy): Version and deploy
# • Version increment (automated semantic versioning)
# • Release build with optimizations
# • Cross-platform binary deployment to ~/bin
# • Git commit with auto-generated descriptive message
# • Push to remote repository
#
# WHY TWO PHASES:
# • Phase 1 can be run repeatedly during development (fast feedback)
# • Phase 2 only runs when ready to ship (prevents unnecessary commits)
# • Fast-fail behavior: any error stops entire workflow immediately
# • Clear separation of concerns: testing vs deployment
# • Enables CI/CD pipeline optimization and benchmarking
#
# FAST-FAIL STRATEGY:
# • bash -euo pipefail ensures ANY command failure stops execution
# • Prevents cascading failures and wasted time on broken code
# • Clear error messages show exactly where failure occurred
# • Example: test failure stops workflow before attempting to commit
#
# ═══════════════════════════════════════════════════════════════════════════════
# 🔍 CLIPPY CONFIGURATION - Rust Master Approach
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Different linting standards for production vs test code
#
# CLIPPY PHILOSOPHY:
# • Production code: Ultra-strict (prevent runtime panics)
# • Test code: Pragmatic (allow unwrap/expect for test clarity)
# • Comprehensive coverage: pedantic + nursery + cargo lints
# • Zero warnings policy: -D warnings (treat warnings as errors)
#
# LINT CATEGORIES EXPLAINED:
# • clippy::pedantic: Nitpicky lints for code quality
# • clippy::nursery: Experimental lints (cutting-edge quality)
# • clippy::cargo: Cargo.toml and dependency lints
# • clippy::panic: Detect potential panic sources
# • clippy::todo: Prevent TODO comments in production
# • clippy::unimplemented: Prevent unimplemented!() in production
#
# PRODUCTION CODE RESTRICTIONS:
# • clippy::unwrap_used: Forbid .unwrap() (use proper error handling)
# • clippy::expect_used: Forbid .expect() (use proper error handling)
# • clippy::missing_docs_in_private_items: Require documentation
#
# TEST CODE ALLOWANCES:
# • Allow unwrap/expect: Tests should be clear and concise
# • Allow missing docs: Test function names should be self-documenting
#
# EXCEPTIONS:
# • clippy::multiple_crate_versions: Allowed (common in large projects)
#   - Different crates may require different versions of dependencies
#   - Not always under our control (transitive dependencies)
#

common_flags := "-D clippy::pedantic -D clippy::nursery -D clippy::cargo -A clippy::multiple_crate_versions -W clippy::panic -W clippy::todo -W clippy::unimplemented -D warnings"
prod_flags := common_flags + " -W clippy::unwrap_used -W clippy::expect_used -W clippy::missing_docs_in_private_items"
test_flags := common_flags + " -A clippy::unwrap_used -A clippy::expect_used"

# ═══════════════════════════════════════════════════════════════════════════════
# 🚀 DEFAULT RECIPE - Main Help System (MUST BE FIRST)
# ═══════════════════════════════════════════════════════════════════════════════

# Default recipe - show available commands
default:
    @printf "\033[0;34m🚀 TTAPI - Modern Rust Development Workflow\033[0m\n"
    @echo "=================================================="
    @echo ""
    @printf "\033[0;32m🚀 Main Workflow (RUST OPTIMIZED):\033[0m\n"
    @echo "  just go           - Complete two-phase fast-fail workflow (Rust async)"
    @echo "  just check-all    - Comprehensive validation (Rust parallel)"
    @echo "  just coverage-report - Smart coverage report generation"
    @echo "  just go-justfile  - Complete workflow (justfile fallback)"
    @echo ""
    @printf "\033[0;32m📋 Individual Steps (Phase 1 - Testing):\033[0m\n"
    @echo "  just fmt          - Format code"
    @echo "  just test         - Run all tests with nextest"
    @echo "  just test-doc     - Run documentation tests"
    @echo "  just test-slow    - Run slow/ignored tests (comprehensive validation)"
    @echo "  just coverage     - Generate coverage report"
    @echo "  just lint-prod    - Ultra-strict production linting"
    @echo "  just lint-tests   - Pragmatic test linting"
    @echo "  just lint-ci      - CI lint gating (proven workflow)"
    @echo "  just legal-check  - Check REUSE compliance"
    @echo ""
    @printf "\033[0;32m📦 Individual Steps (Phase 2 - Build/Deploy):\033[0m\n"
    @echo "  just version-bump - Increment version"
    @echo "  just build        - Build development binary"
    @echo "  just deploy       - Copy binary to ~/bin"
    @echo "  just commit       - Git commit with auto message"
    @echo "  just push         - Git push to remote"
    @echo ""
    @printf "\033[0;32m🚀 GitHub Releases (triggers Actions workflow):\033[0m\n"
    @echo "  just release         - Trigger release (auto-detect version from Cargo.toml)"
    @echo "  just release v0.1.X  - Trigger release with explicit version"
    @echo "  just release-watch   - Watch the running release workflow"
    @echo "  just release-status  - Open release workflow in browser"
    @echo ""
    @echo "🔄 Workflow State Management:"
    @echo "  just workflow-status - Check current workflow status"
    @echo "  just workflow-reset  - Reset workflow state (force clean slate)"
    @echo "  just workflow-resume - Resume incomplete workflow"
    @echo ""
    @printf "\033[0;32m🔧 Development:\033[0m\n"
    @echo "  just dev          - Watch mode with testing"
    @echo "  just check        - Quick validation (default features)"
    @echo "  just check-all    - Comprehensive validation (all features)"
    @echo "  just clean        - Clean build artifacts"
    @echo ""
    @printf "\033[0;32m📊 Analysis & Legal:\033[0m\n"
    @echo "  just setup        - Universal development environment setup"
    @echo "  just audit        - Security audit"
    @echo "  just audit-comprehensive - Multi-tool security audit"
    @echo "  just deps-optimize - Dependency optimization and cleanup"
    @echo "  just debug-deep   - Advanced debugging (macro expansion + miri)"
    @echo "  just legal-setup  - Complete legal setup"
    @echo "  just version      - Show current version"
    @echo ""
    @printf "\033[0;32m🚀 Compilation Acceleration:\033[0m\n"
    @echo "  just setup-sccache - Install and configure sccache for faster compilation"
    @echo "  just sccache-stats - Show sccache cache statistics"
    @echo "  just sccache-clear - Clear sccache cache to free disk space"
    @echo "  just update-tools  - Update all development tools efficiently"
    @echo ""
    @printf "\033[0;32m Cross-Platform Development:\033[0m\n"
    @echo "  just setup-cross   - Install cross-compilation targets"
    @echo "  just check-cross   - Cross-compile check for GitHub Actions compatibility"
    @echo ""
    @printf "\033[0;32m Development Environment (Rust-based):\033[0m\n"
    @echo "  just devenv-setup  - Complete development environment setup"
    @echo "  just devenv-tools  - Install all development tools"
    @echo "  just devenv-tools-core - Install core development tools only"
    @echo "  just devenv-tools-refactoring - Install refactoring tools only"
    @echo "  just devenv-legal-headers - Add legal headers to source files"
    @echo "  just devenv-legal-complete - Complete legal compliance for all files"
    @echo "  just devenv-legal-validate - Validate legal compliance status"
    @echo "  just devenv-ci-setup - Setup CI/CD pipeline configuration"
    @echo "  just devenv-ci-validate - Validate CI configuration"
    @echo "  just devenv-monitoring-setup - Setup monitoring infrastructure"
    @echo "  just devenv-validate-production - Validate production readiness"
    @echo "  just devenv-validate-improvements - Validate code quality improvements"
    @echo "  just devenv-test-unit - Run unit tests"
    @echo "  just devenv-test-all - Run all test categories"
    @echo "  just devenv-setup - Complete development environment setup"
    @echo "  just devenv-release VERSION - Create software release"
    @echo "  just devenv-rollback - Rollback to previous version"
    @echo ""
    @printf "\033[0;32m🏁 UFFS Performance Benchmarks:\033[0m\n"
    @echo "  just bench-vs-cpp - Compare Rust vs C++ (uffs.com) performance"
    @echo "  just bench-vs-cpp D - Compare on specific drive"
    @echo "  just bench-micro - Run criterion micro-benchmarks"
    @echo "  just bench-search - End-to-end search benchmark"
    @echo "  just bench-all-compare - Run all and compare to baseline"
    @echo "  just bench-update-baseline - Update baseline with current results"
    @echo "  just bench-help - Show all benchmark commands"
    @echo ""
    @printf "\033[0;32m🚀 CI Pipeline Benchmarking:\033[0m\n"
    @echo "  just benchmark-flows - Compare Rust script vs JUST file flows (RECOMMENDED)"
    @echo "  just benchmark-rust-script - Benchmark Rust script workflow only"
    @echo "  just benchmark-justfile - Benchmark JUST file workflow only"
    @echo "  just benchmark-both - Compare both workflow approaches"
    @echo "  just benchmark-flows-debug - Debug version with visible output"
    @echo "  just benchmark-advanced - Advanced performance profiling"
    @echo "  just ci-analyze - Comprehensive CI pipeline analysis"
    @echo ""
    @printf "\033[0;32m🔬 Windows Profiling:\033[0m\n"
    @echo "  just profile-usb  - Build profiling binaries and copy to USB"
    @echo "  just profile-load - Load profile.json from USB and open in Firefox Profiler"

# ═══════════════════════════════════════════════════════════════════════════════
# 🔧 HELPER FUNCTIONS - Reusable Installation & Setup Logic
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Idempotent, cross-platform tool installation with intelligent fallbacks
#
# DESIGN PRINCIPLES:
# • Idempotent: Safe to run multiple times (checks before installing)
# • Cross-platform: Works on Windows, macOS, and Linux
# • Intelligent fallbacks: cargo-binstall → cargo install
# • Clear feedback: Shows what's happening and why
# • Error isolation: One tool failure doesn't break others
#
# HELPER FUNCTION STRATEGY:
# • Prefixed with underscore: Private functions (not shown in just --list)
# • Bash shebang: Ensures consistent shell behavior across platforms
# • command -v: Cross-platform way to check if tool exists
# • Quiet installation: Reduces noise during setup
# • Color-coded output: Clear visual feedback on status
#
# WHY NOT LOOPS:
# • PowerShell compatibility: Bash loops don't work in PowerShell
# • Error isolation: Individual calls allow precise error handling
# • Clear progress: Shows exactly which tool is being installed
# • Debugging: Easy to test individual tool installations
# Install cargo tool if missing (idempotent)
# USAGE: just _install-if-missing cargo-nextest cargo-nextest
# WHY IDEMPOTENT: Checks if tool exists before attempting installation
# WHY CARGO-BINSTALL: Downloads pre-built binaries (faster than compilation)

# FALLBACK: Uses cargo install if cargo-binstall unavailable
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
# USAGE: just _install-component llvm-tools-preview
# WHY COMPONENTS: Some tools require rustup components (not cargo crates)

# EXAMPLES: llvm-tools-preview (for coverage), miri (for undefined behavior detection)
_install-component COMPONENT:
    #!/usr/bin/env bash
    if ! rustup component list --installed | grep -q "{{ COMPONENT }}"; then
        printf "\033[0;34m📦 Installing {{ COMPONENT }} component...\033[0m\n"
        rustup component add {{ COMPONENT }}
        printf "\033[0;32m✅ {{ COMPONENT }} component installed\033[0m\n"
    else
        printf "\033[0;32m✅ {{ COMPONENT }} component already installed (skip)\033[0m\n"
    fi

# ═══════════════════════════════════════════════════════════════════════════════
# 🚀 UNIVERSAL SETUP - Complete Development Environment
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: One-command setup for consistent development environment across all platforms
#
# SETUP STRATEGY:
# • Install cargo-binstall first: Makes subsequent installations much faster
# • Individual tool installation: Isolated failures, clear progress feedback
# • Idempotent design: Safe to run multiple times (won't reinstall existing tools)
# • Component installation: Adds required rustup components for advanced features
# • Cross-platform: Works identically on Windows, macOS, and Linux
#
# INSTALLATION ORDER MATTERS:
# 1. cargo-binstall: Enables faster binary downloads for other tools
# 2. Core tools: Essential development workflow tools
# 3. Analysis tools: Security and quality analysis capabilities
# 4. Debugging tools: Advanced debugging and macro expansion
# 5. Rustup components: Required for coverage and undefined behavior detection
#
# WHY NOT BATCH INSTALLATION:
# • Individual calls provide clear progress feedback
# • Isolated failures: One tool failure doesn't break entire setup
# • Cross-platform compatibility: Avoids bash loop syntax issues
# • Debugging: Easy to identify which specific tool failed
# Universal development environment setup
# USAGE: just setup
# SAFE TO RE-RUN: Idempotent design checks for existing tools

# CROSS-PLATFORM: Works on Windows (Git Bash/PowerShell), macOS, Linux
setup:
    #!/usr/bin/env bash
    printf "\033[0;34m🔧 Universal Smart Development Environment Setup\033[0m\n"
    printf "\033[0;34m   Installing essential Rust development tools...\033[0m\n"

    # Install cargo-binstall first (makes other installations faster)
    # WHY FIRST: Subsequent tools will use binary downloads instead of compilation
    just _install-if-missing cargo-binstall cargo-binstall

    # Core development workflow tools
    just _install-if-missing cargo-watch cargo-watch      # File watching for development
    just _install-if-missing cargo-nextest cargo-nextest  # Modern test runner
    just _install-if-missing cargo-llvm-cov cargo-llvm-cov # Code coverage

    # Security and analysis tools
    just _install-if-missing cargo-deny cargo-deny        # Dependency analysis
    just _install-if-missing cargo-audit cargo-audit      # Security scanning
    just _install-if-missing cargo-geiger cargo-geiger    # Unsafe code detection

    # Dependency management tools
    just _install-if-missing cargo-outdated cargo-outdated # Update checking
    just _install-if-missing cargo-udeps cargo-udeps      # Unused dependency detection
    just _install-if-missing cargo-machete cargo-machete  # Automatic cleanup

    # Debugging and development tools
    just _install-if-missing cargo-expand cargo-expand    # Macro expansion
    just _install-if-missing rust-script rust-script      # Script execution

    # Performance analysis tools
    just _install-if-missing cargo-criterion cargo-criterion # Advanced benchmarking

    # Tool maintenance and optimization
    just _install-if-missing cargo-update cargo-update      # Efficient tool updates
    just _install-if-missing sccache sccache                # Compilation caching

    # Install required rustup components
    just _install-component llvm-tools-preview  # Required for cargo-llvm-cov
    just _install-component miri                # Undefined behavior detection

    printf "\033[0;32m✅ Development environment setup complete!\033[0m\n"
    printf "\033[0;34mNext steps:\033[0m\n"
    printf "  • Run 'just go' to build and test everything\n"
    printf "  • Run 'just ship' for the complete two-phase workflow\n"
    printf "  • Run 'just ci-analyze' for CI pipeline optimization\n"

# ═══════════════════════════════════════════════════════════════════════════════
# 🔧 INDIVIDUAL WORKFLOW COMMANDS - Granular Control with Fast-Fail
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Individual commands for granular control and debugging
#
# FAST-FAIL PHILOSOPHY:
# • Each command stops immediately on any error (bash -euo pipefail)
# • Prevents wasted time on broken code
# • Clear error messages show exactly where failure occurred
# • Enables rapid development iteration
#
# COMMAND DESIGN PRINCIPLES:
# • Consistent color coding: Blue for progress, Green for success
# • Descriptive output: Shows what's happening and why
# • Workspace-wide: Operates on entire workspace (all crates)
# • Feature-complete: Tests all features to catch feature-specific issues
# • Parallel execution: Uses available CPU cores for speed
# Format code with rustfmt
# WHY: Consistent code formatting across entire project
# SCOPE: --all includes all workspace members

# Verify Rust toolchain consistency (prevents toolchain switching issues)
# WHY: Ensures consistent nightly toolchain usage across all environments
# PREVENTS: Compilation errors from mixed toolchain artifacts
# SCOPE: Validates rust-toolchain.toml configuration is active
verify-toolchain:
    @printf "\033[0;34m🔧 Verifying Rust toolchain consistency...\033[0m\n"
    @rust-script scripts/verify-toolchain.rs

# AUTOMATIC: Applies formatting automatically (no --check flag)
# ENHANCED: Uses centralized formatting logic (simplified)
fmt:
    @printf "\033[0;34m📝 Formatting code (CENTRALIZED WORKFLOW)...\033[0m\n"
    @printf "\033[0;36m  → Using ttapi-core formatting module\033[0m\n"
    cargo run -p ttapi-devenv -- format

# Check code formatting without applying fixes
# WHY: Validation for CI/CD pipelines - ensures code is properly formatted
# ENHANCED: Uses centralized formatting logic
fmt-check:
    @printf "\033[0;34m📝 Checking code formatting (CENTRALIZED WORKFLOW)...\033[0m\n"
    @printf "\033[0;36m  → Using ttapi-core formatting module\033[0m\n"
    cargo run -p ttapi-devenv -- format --check --verbose
    @printf "\033[0;36m  → Step 3: Validate formatting (all workspace members)\033[0m\n"
    cargo fmt --all -- --check

# Run all tests with nextest (modern test runner)
# WHY NEXTEST: Faster execution, better output, superior parallelism
# SCOPE: --workspace --all-features --all-targets (comprehensive testing)
# PARALLELISM: --jobs 16 (adjust based on available CPU cores)

# FAST-FAIL: Stops on first test failure (rapid feedback)
test:
    @printf "\033[0;34m🧪 Running all tests with nextest (FAST-FAIL)...\033[0m\n"
    cargo llvm-cov nextest run --workspace --all-features --all-targets --jobs 16 --no-report

# Run documentation tests separately
# WHY SEPARATE: Doc tests require different compilation (avoid duplication)
# RUSTDOCFLAGS: -Dwarnings treats doc warnings as errors

# SCOPE: --workspace --all-features (comprehensive doc testing)
test-doc:
    @printf "\033[0;34m📚 Running documentation tests (FAST-FAIL)...\033[0m\n"
    RUSTDOCFLAGS='-Dwarnings -Dmissing_docs' cargo llvm-cov test --doc --workspace --all-features --no-report

# Run slow/ignored tests (comprehensive validation)
# WHY SEPARATE: These tests take 2+ minutes each and include full workspace validation
# SCOPE: --ignored runs only tests marked with #[ignore]
# PROFILE: Uses nextest 'slow' profile with extended timeouts (30 minutes)
# USAGE: For nightly CI, comprehensive validation, and manual deep testing
test-slow:
    @printf "\033[0;34m🐌 Running slow/ignored tests (comprehensive validation)...\033[0m\n"
    @printf "\033[1;33m⚠️  These tests may take several minutes - includes full workspace validation\033[0m\n"
    cargo nextest run --workspace --all-features --all-targets --profile slow --run-ignored ignored-only

# Generate coverage report

# WHY CLEAN: Ensures accurate coverage data without stale artifacts
coverage:
    @printf "\033[0;34m📊 Generating coverage report...\033[0m\n"
    cargo clean
    @if command -v cargo-llvm-cov >/dev/null 2>&1; then \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --html; \
        printf "\033[0;32m📁 Coverage report: target/llvm-cov/html/index.html\033[0m\n"; \
    else \
        printf "\033[1;33mInstalling cargo-llvm-cov...\033[0m\n"; \
        just _install-if-missing cargo-llvm-cov cargo-llvm-cov; \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --html; \
        printf "\033[0;32m📁 Coverage report: target/llvm-cov/html/index.html\033[0m\n"; \
    fi

# Ultra-strict production linting (FAST-FAIL)
lint-prod:
    @printf "\033[0;34m🔍 Ultra-strict production linting (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --lib --bins --all-features --no-deps -- {{ prod_flags }}

# Pragmatic test linting (FAST-FAIL)
lint-tests:
    @printf "\033[0;34m🧪 Pragmatic test linting (FAST-FAIL)...\033[0m\n"
    cargo clippy --workspace --tests --all-features --no-deps -- {{ test_flags }}

# Gate residual lints in CI (PROVEN WORKFLOW)
# This fails the build if any hand-review items are left
lint-ci:
    @printf "\033[0;34m🚨 CI lint gating (PROVEN WORKFLOW)...\033[0m\n"
    cargo clippy --workspace --all-features -- -D warnings

# Version increment
version-bump:
    @printf "\033[0;34m📈 Incrementing version...\033[0m\n"
    @if [ -f "./build/update_all_versions.rs" ]; then \
        ./build/update_all_versions.rs patch; \
    else \
        printf "\033[1;33mVersion script not found, using manual increment...\033[0m\n"; \
        echo "Manual version increment needed"; \
    fi

# Build dev binary (FAST-FAIL) - faster for active development
build:
    @printf "\033[0;34m🔨 Building dev binary (FAST-FAIL)...\033[0m\n"
    cargo build --workspace

# Build release binary (for production releases)
build-release:
    @printf "\033[0;34m🔨 Building optimized release binary...\033[0m\n"
    @printf "\033[1;33m⚠️  This will take significantly longer than dev builds\033[0m\n"
    cargo build --release --workspace

# Build and install all binaries locally (PLATFORM-SPECIFIC)
build-local:
    @printf "\033[0;34m🚀 Building and installing all TTAPI binaries locally...\033[0m\n"
    @printf "\033[1;33m📦 Platform-specific build with Fat LTO optimization\033[0m\n"
    @printf "\033[1;33m📁 Installing to ~/bin with PATH setup\033[0m\n"
    @echo "========================================================"
    rust-script scripts/build-local.rs

# Check and build current platform binaries (NO VERSION INCREMENT)
build-current-platform:
    @printf "\033[0;34m🔍 Checking current platform binaries (NO VERSION INCREMENT)...\033[0m\n"
    @printf "\033[1;33m📦 Builds only if missing, commits and pushes automatically\033[0m\n"
    @echo "========================================================"
    rust-script scripts/build-current-platform.rs

# Quick alias for local build and install
install: build-local

# Deploy binary to ~/bin
deploy:
    @printf "\033[0;34m📦 Deploying dev binary...\033[0m\n"
    just copy-binary debug

# Deploy release binary to ~/bin
deploy-release:
    @printf "\033[0;34m📦 Deploying release binary...\033[0m\n"
    just copy-binary release

# Install latest pre-built binaries from dist/ to ~/bin (platform-aware)
# Install UFFS binaries to ~/bin (Windows only)
# UFFS is Windows-only - it requires NTFS MFT access via Windows APIs
# Binaries: uffs, uffs_mft, uffs_tui, uffs_gui
# This recipe uses PowerShell on Windows, bash on Unix (where it will fail gracefully)
[windows]
use:
    Write-Host "📦 Installing latest UFFS binaries to ~/bin..." -ForegroundColor Blue; \
    Write-Host "ℹ️  Note: UFFS is Windows-only (requires NTFS MFT access)" -ForegroundColor Yellow; \
    $binDir = "$env:USERPROFILE\bin"; \
    if (-not (Test-Path $binDir)) { New-Item -ItemType Directory -Path $binDir -Force | Out-Null }; \
    $distDir = $null; \
    $latestItem = Get-Item "dist\latest" -ErrorAction SilentlyContinue; \
    if ($latestItem -and $latestItem.LinkType -eq 'SymbolicLink') { $distDir = $latestItem.Target } \
    elseif ($latestItem -and -not $latestItem.PSIsContainer) { $target = (Get-Content "dist\latest" -Raw).Trim(); if (Test-Path "dist\$target") { $distDir = "dist\$target" } }; \
    if (-not $distDir) { $versions = Get-ChildItem -Path "dist" -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '^v\d' } | Sort-Object Name -Descending; if ($versions) { $distDir = $versions[0].FullName } }; \
    if (-not $distDir) { Write-Host "❌ No binaries found in dist/. Run 'just go' first." -ForegroundColor Red; exit 1 }; \
    Write-Host "  → Using: $distDir" -ForegroundColor Cyan; \
    $binaries = @("uffs", "uffs_mft", "uffs_tui", "uffs_gui", "analyze_mft_parents", "dump_mft_records", "scan_mft_magic", "dump_mft_extents", "cross_check_mft_reference", "compare_raw_mft", "inspect_mft_record_flow"); \
    $installed = 0; $skipped = 0; \
    foreach ($bin in $binaries) { $src = "$distDir\$bin\$bin-windows-x64.exe"; $dest = "$binDir\$bin.exe"; if (Test-Path $src) { Copy-Item $src $dest -Force; Write-Host "  ✅ $bin.exe" -ForegroundColor Green; $installed++ } else { Write-Host "  ⚠️  $bin (not found)" -ForegroundColor Yellow; $skipped++ } }; \
    Write-Host "✅ Installed $installed binaries to ~/bin" -ForegroundColor Green; \
    if ($skipped -gt 0) { Write-Host "⚠️  Skipped $skipped (run 'just go' to build all binaries)" -ForegroundColor Yellow }

# Unix version of 'use' - explains that UFFS is Windows-only
[unix]
use:
    #!/usr/bin/env bash
    printf "\033[0;31m❌ UFFS only runs on Windows.\033[0m\n"
    printf "\033[0;34m   UFFS reads the NTFS Master File Table directly using Windows APIs.\033[0m\n"
    printf "\033[0;34m   Use 'just go' to cross-compile Windows binaries from this host.\033[0m\n"
    exit 1

# Git commit with auto-generated message
commit:
    @printf "\033[0;34m📝 Creating auto-generated commit...\033[0m\n"
    git add .
    @NEW_VERSION=`awk '/^\[workspace\.package\]/{flag=1; next} flag && /^version/{print $3; exit}' Cargo.toml | tr -d '"'`; \
    git -c commit.gpgsign=false commit -m "chore: release v$NEW_VERSION - comprehensive testing complete [auto-commit]"

# Git push to remote
push:
    @printf "\033[0;34m🚀 Pushing to remote...\033[0m\n"
    # Check if there are any uncommitted changes and commit them
    @if git diff --quiet && git diff --cached --quiet; then \
        echo "📋 Working directory is clean"; \
    else \
        echo "📋 Committing pending changes..."; \
        git add .; \
        git -c commit.gpgsign=false commit -m "chore: commit pending changes before push [auto-commit]"; \
    fi
    git pull origin main --rebase
    git push origin main

# Comprehensive code validation (CI/CD grade - all features and targets)
check:
    @printf "\033[0;34m🔍 Comprehensive validation (CI/CD grade)...\033[0m\n"
    cargo check --workspace --all-targets --all-features
    cargo fmt --all -- --check

# Quick code validation (fast development iteration)
check-quick:
    @printf "\033[0;34m⚡ Quick validation (development speed)...\033[0m\n"
    cargo check --workspace

# ═══════════════════════════════════════════════════════════════════════════

# Update Polars git lock to latest main (Option 3c)
update-polars:
    @printf "\033[0;34m📦 Updating Polars (git, branch=main) lockfile...\033[0m\n"
    cargo update -w
    @printf "\033[0;32m✅ Polars lockfile updated\033[0m\n"

# Professional Two-Phase Development Workflow - FAST-FAIL
# ═══════════════════════════════════════════════════════════════════════════
# Phase 1: Code & Extensive Testing (Manual Trigger)
# Phase 2: Build/Commit/Push/Deploy (Manual Trigger)
# ═══════════════════════════════════════════════════════════════════════════
# PHASE 1: Code & Extensive Testing (FAST-FAIL)
# WHY CLEAN FIRST: Prevents cross-project contamination and ensures fresh builds
# • Removes stale build artifacts that might cause false positives/negatives
# • Ensures consistent compilation environment across different runs
# • Prevents issues with incremental compilation when switching between features
# • Essential for accurate coverage reporting (no stale coverage data)

# • Matches modern justfile best practices for professional workflows
phase1-test:
    @printf "\033[0;34m🧪 PHASE 1: Code & Extensive Testing (FAST-FAIL)\033[0m\n"
    @printf "\033[1;33mRunning MOST extensive tests - STOPPING at FIRST failure...\033[0m\n"
    @echo "========================================================"
    @echo ""

    # Step 1: Clean build artifacts (prevent cross-project contamination)
    # WHY: Ensures fresh compilation state and accurate test/coverage results
    @printf "\033[0;34mStep 1: Cleaning build artifacts...\033[0m\n"
    @printf "\033[0;36m  → cargo clean\033[0m\n"
    cargo clean
    @printf "\033[0;32m✅ Build artifacts cleaned\033[0m\n"

    # Step 2: Auto-formatting
    @printf "\033[0;34mStep 2: Auto-formatting code...\033[0m\n"
    @printf "\033[0;36m  → just fmt\033[0m\n"
    just fmt

    # Step 3: Run all tests with coverage data collection (FAST-FAIL)
    @printf "\033[0;34mStep 3: Running all tests with coverage data collection (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report\033[0m\n"
    @if command -v cargo-llvm-cov >/dev/null 2>&1; then \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report; \
        printf "\033[0;32m✅ All tests passed, coverage data collected\033[0m\n"; \
    else \
        printf "\033[1;33mInstalling cargo-llvm-cov...\033[0m\n"; \
        just _install-if-missing cargo-llvm-cov cargo-llvm-cov; \
        cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report; \
        printf "\033[0;32m✅ All tests passed, coverage data collected\033[0m\n"; \
    fi

    # Step 4: Coverage data collected (HTML report skipped for speed)
    @printf "\033[0;32m✅ Coverage data collected (use 'just coverage-report' for HTML)\033[0m\n"

    # Step 5: Documentation tests (FAST-FAIL)
    @printf "\033[0;34mStep 5: Documentation tests validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just test-doc\033[0m\n"
    just test-doc

    # Step 6: Ultra-strict production linting (FAST-FAIL)
    @printf "\033[0;34mStep 6: Ultra-strict production code linting (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-prod\033[0m\n"
    just lint-prod

    # Step 7: Pragmatic test linting (FAST-FAIL)
    @printf "\033[0;34mStep 7: Pragmatic test code linting (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-tests\033[0m\n"
    just lint-tests

    # Step 8: Legal compliance check (FAST-FAIL)
    @printf "\033[0;34mStep 8: Legal compliance validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just legal-check\033[0m\n"
    just legal-check

    # Step 9: Enhanced format validation (final check) (FAST-FAIL)
    @printf "\033[0;34mStep 9: Enhanced format validation (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → cargo fix --workspace --all-targets --edition-idioms --allow-dirty (ENHANCED)\033[0m\n"
    cargo fix --workspace --all-targets --edition-idioms --allow-dirty
    @printf "\033[0;36m  → cargo clippy --fix --workspace --allow-dirty --allow-staged (PROVEN WORKFLOW)\033[0m\n"
    cargo clippy --fix --workspace --allow-dirty --allow-staged -- -D warnings -D clippy::pedantic -D clippy::nursery -D clippy::cargo -A clippy::multiple_crate_versions
    @printf "\033[0;36m  → cargo fmt --all -- --check\033[0m\n"
    cargo fmt --all -- --check

    # Step 10: CI lint gating (proven workflow) (FAST-FAIL)
    @printf "\033[0;34mStep 10: CI lint gating (proven workflow) (FAST-FAIL)...\033[0m\n"
    @printf "\033[0;36m  → just lint-ci (fails build if hand-review items remain)\033[0m\n"
    just lint-ci

    @echo ""
    @printf "\033[0;32m✅ PHASE 1 FAST-FAIL COMPLETE: All extensive tests passed, code ready for commit!\033[0m\n"
    @printf "\033[0;34m💡 Next: Run 'just phase2-ship' when ready to build/commit/push\033[0m\n"

# PHASE 2: Version/Build/Deploy (Professional Grade)
phase2-ship:
    @printf "\033[0;34m🚀 PHASE 2: Version/Build/Deploy (Post-Testing)\033[0m\n"
    @printf "\033[1;33mAssumes Phase 1 completed: format ✅ clippy ✅ compile ✅ tests ✅\033[0m\n"
    @echo "========================================================"
    @echo ""

    # Step 1: Version increment
    @printf "\033[0;34mStep 1: Version increment...\033[0m\n"
    @printf "\033[0;36m  → just version-bump\033[0m\n"
    just version-bump

    # Step 2: Build with new version (dev build for faster iteration)
    @printf "\033[0;34mStep 2: Building dev binary with NEW version...\033[0m\n"
    @printf "\033[0;36m  → just build\033[0m\n"
    just build

    # Step 3: Copy binary to deployment location
    @printf "\033[0;34mStep 3: Copy binary to deployment location...\033[0m\n"
    @printf "\033[0;36m  → just deploy\033[0m\n"
    just deploy

    # Step 4: Create auto-generated commit
    @printf "\033[0;34mStep 4: Creating auto-generated commit...\033[0m\n"
    @printf "\033[0;36m  → just commit\033[0m\n"
    just commit

    # Step 5: Sync with remote and push
    @printf "\033[0;34mStep 5: Syncing with remote and pushing...\033[0m\n"
    @printf "\033[0;36m  → just push\033[0m\n"
    just push

    @echo ""
    @printf "\033[0;32m✅ PHASE 2 COMPLETE: Version incremented, built, deployed, committed, and pushed!\033[0m\n"

# Backward compatibility alias for phase2-ship
phase2-commit: phase2-ship

# Complete two-phase fast-fail workflow (RUST OPTIMIZED)
go:
    @printf "\033[0;34m🚀 Complete Two-Phase Fast-Fail Workflow (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mUsing high-performance Rust script with async parallelism...\033[0m\n"
    @echo "========================================================"
    rust-script scripts/ci-pipeline.rs go

# Complete two-phase fast-fail workflow (JUSTFILE FALLBACK)
go-justfile:
    #!/usr/bin/env bash
    printf "\033[0;34m🚀 Complete Two-Phase Fast-Fail Workflow (JUSTFILE)\033[0m\n"
    printf "\033[1;33mFailing fast at ANY error in either phase...\033[0m\n"
    echo "========================================================"
    echo ""

    START_TIME=$(date +%s)
    printf "\033[0;36m⏱️  Workflow started at: $(date +%H:%M:%S)\033[0m\n"
    echo ""

    printf "\033[0;34m🧪 PHASE 1: Comprehensive Fast-Fail Testing & Validation\033[0m\n"
    just phase1-test

    echo ""
    printf "\033[0;32m✅ PHASE 1 COMPLETE - All validation passed!\033[0m\n"
    printf "\033[0;34m🚀 Starting PHASE 2: Build/Deploy...\033[0m\n"
    echo ""

    printf "\033[0;34m📦 PHASE 2: Fast-Fail Build & Deploy\033[0m\n"
    just phase2-ship

    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    echo ""
    printf "\033[0;32m🎉 COMPLETE TWO-PHASE FAST-FAIL WORKFLOW FINISHED!\033[0m\n"
    printf "\033[0;32m✅ Phase 1: Testing & Validation\033[0m\n"
    printf "\033[0;32m✅ Phase 2: Build/Commit/Push/Deploy\033[0m\n"
    printf "\033[0;36m⏱️  Total execution time: ${DURATION}s\033[0m\n"

# Comprehensive validation (RUST OPTIMIZED)
check-all:
    @printf "\033[0;34m📋 Comprehensive Validation (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mUsing high-performance Rust script with parallel execution...\033[0m\n"
    @echo "========================================================"
    rust-script scripts/ci-pipeline.rs check-all

# Generate coverage report (RUST OPTIMIZED)
coverage-report:
    @printf "\033[0;34m📊 Coverage Report Generation (RUST OPTIMIZED)\033[0m\n"
    @printf "\033[1;33mSmart detection: generates from existing data or runs tests if needed...\033[0m\n"
    @echo "========================================================"
    rust-script scripts/ci-pipeline.rs coverage-report

# ═══════════════════════════════════════════════════════════════════════════
# Development Utilities
# ═══════════════════════════════════════════════════════════════════════════

# Watch mode development
dev:
    @printf "\033[0;34m🔄 Starting watch mode...\033[0m\n"
    @if command -v cargo-watch >/dev/null 2>&1; then \
        cargo watch -x "nextest run --workspace --jobs 16" -x "clippy --workspace --all-targets --all-features --no-deps -- {{ test_flags }}"; \
    else \
        printf "\033[1;33mInstalling cargo-watch...\033[0m\n"; \
        just _install-if-missing cargo-watch cargo-watch; \
        cargo watch -x "nextest run --workspace --jobs 16" -x "clippy --workspace --all-targets --all-features --no-deps -- {{ test_flags }}"; \
    fi

# Security audit
audit:
    @printf "\033[0;34m🔒 Security audit...\033[0m\n"
    @if command -v cargo-audit >/dev/null 2>&1; then \
        cargo audit; \
    else \
        printf "\033[1;33mInstalling cargo-audit...\033[0m\n"; \
        just _install-if-missing cargo-audit cargo-audit; \
        cargo audit; \
    fi

# Show current version
version:
    @printf "\033[0;34m📋 Current version:\033[0m\n"
    @cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "ttapi") | .version'

# Trigger GitHub Actions release workflow (builds binaries for all platforms)
# Usage: just release          - Auto-detect version from Cargo.toml
#        just release v0.1.193 - Explicit version
# Requires: gh CLI (brew install gh && gh auth login)
release VERSION="":
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🚀 Triggering GitHub Release Workflow...\033[0m\n"

    if [ -z "{{ VERSION }}" ]; then
        # Auto-detect version from Cargo.toml
        ./scripts/release-github.sh
    else
        ./scripts/release-github.sh "{{ VERSION }}"
    fi

# Watch the latest release workflow run
release-watch:
    @printf "\033[0;34m👀 Watching latest release workflow...\033[0m\n"
    @gh run watch $(gh run list --workflow=release.yml --limit=1 --json databaseId --jq '.[0].databaseId')

# Open release workflow in browser
release-status:
    @printf "\033[0;34m🔗 Opening release workflow in browser...\033[0m\n"
    @gh run list --workflow=release.yml --limit=1 --web

# Check current workflow status
workflow-status:
    @printf "\033[0;34m📊 Checking workflow status...\033[0m\n"
    @rust-script scripts/ci-pipeline.rs workflow-status

# Reset workflow state (force clean slate)
workflow-reset:
    @printf "\033[0;33m⚠️  Resetting workflow state...\033[0m\n"
    @rust-script scripts/ci-pipeline.rs workflow-reset

# Resume incomplete workflow
workflow-resume:
    @printf "\033[0;34m🔄 Resuming workflow...\033[0m\n"
    @rust-script scripts/ci-pipeline.rs workflow-resume

# Debug version of benchmark-flows with visible output
benchmark-flows-debug:
    #!/usr/bin/env bash
    printf "\033[0;34m⚡ Workflow Orchestration Comparison (DEBUG MODE)...\033[0m\n"
    printf "\033[1;33m🎯 GOAL: Compare Rust script vs JUST file workflow approaches\033[0m\n"
    printf "\033[0;34m📋 SAME LOGIC: Both run identical steps with same commands\033[0m\n"
    printf "\033[0;34m📋 ONLY DIFFERENCE: Orchestration approach (async/parallel vs sequential)\033[0m\n"

    # Test 1: Rust script approach (async/parallel)
    printf "\033[0;34m\n🔄 Test 1: Rust Script Workflow (just go)\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"

    cargo clean
    TEST1_START=$(date +%s)

    printf "\033[0;34m  → Running Rust script workflow with async/parallel optimization (VERBOSE)...\033[0m\n"
    if rust-script scripts/ci-pipeline.rs go -v; then
        TEST1_END=$(date +%s)
        TEST1_TOTAL=$((TEST1_END - TEST1_START))
        printf "\033[0;32m  ✅ Rust script completed successfully in ${TEST1_TOTAL}s\033[0m\n"
    else
        TEST1_END=$(date +%s)
        TEST1_TOTAL=-1
        printf "\033[0;31m  ❌ Rust script failed\033[0m\n"
    fi

    # Test 2: JUST file approach (sequential)
    printf "\033[0;34m\n🔄 Test 2: JUST File Workflow (just go-justfile)\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"

    cargo clean
    TEST2_START=$(date +%s)

    printf "\033[0;34m  → Running JUST file workflow with sequential execution...\033[0m\n"
    if just go-justfile; then
        TEST2_END=$(date +%s)
        TEST2_TOTAL=$((TEST2_END - TEST2_START))
        printf "\033[0;32m  ✅ JUST file workflow completed successfully in ${TEST2_TOTAL}s\033[0m\n"
    else
        TEST2_END=$(date +%s)
        TEST2_TOTAL=-1
        printf "\033[0;31m  ❌ JUST file workflow failed\033[0m\n"
    fi

    # Results
    printf "\033[0;32m\n⚡ WORKFLOW ORCHESTRATION RESULTS:\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"

    if [ "$TEST1_TOTAL" -eq -1 ] && [ "$TEST2_TOTAL" -eq -1 ]; then
        printf "\033[0;31m❌ Both workflows failed - check output above for errors\033[0m\n"
    elif [ "$TEST1_TOTAL" -eq -1 ]; then
        printf "\033[0;31m📊 Rust Script (async/parallel):  FAILED\033[0m\n"
        printf "\033[0;34m📊 JUST File (sequential):        ${TEST2_TOTAL}s\033[0m\n"
        printf "\033[0;32m🏆 WINNER: JUST File (just go-justfile)\033[0m\n"
    elif [ "$TEST2_TOTAL" -eq -1 ]; then
        printf "\033[0;34m📊 Rust Script (async/parallel):  ${TEST1_TOTAL}s\033[0m\n"
        printf "\033[0;31m📊 JUST File (sequential):        FAILED\033[0m\n"
        printf "\033[0;32m🏆 WINNER: Rust Script (just go)\033[0m\n"
    else
        printf "\033[0;34m📊 Rust Script (async/parallel):  ${TEST1_TOTAL}s\033[0m\n"
        printf "\033[0;34m📊 JUST File (sequential):        ${TEST2_TOTAL}s\033[0m\n"
        if [ "$TEST1_TOTAL" -lt "$TEST2_TOTAL" ]; then
            SAVINGS=$((TEST2_TOTAL - TEST1_TOTAL))
            printf "\033[0;32m🏆 WINNER: Rust Script (just go)\033[0m\n"
            printf "\033[0;32m💰 SAVINGS: ${SAVINGS}s faster\033[0m\n"
        elif [ "$TEST2_TOTAL" -lt "$TEST1_TOTAL" ]; then
            SAVINGS=$((TEST1_TOTAL - TEST2_TOTAL))
            printf "\033[0;32m🏆 WINNER: JUST File (just go-justfile)\033[0m\n"
            printf "\033[0;32m💰 SAVINGS: ${SAVINGS}s faster\033[0m\n"
        else
            printf "\033[0;33m🤝 TIE: Both approaches have identical performance\033[0m\n"
        fi
    fi

# Clean build artifacts
clean:
    @printf "\033[0;34m🧹 Cleaning build artifacts...\033[0m\n"
    cargo clean

# Emergency killswitch
kill:
    @printf "\033[0;31m🚨 EMERGENCY: Killing all cargo processes...\033[0m\n"
    @pkill -f "cargo" 2>/dev/null || true
    @pkill -f "rustc" 2>/dev/null || true
    @printf "\033[0;32m✅ All cargo processes terminated\033[0m\n"

# ═══════════════════════════════════════════════════════════════════════════
# Cross-Platform Development
# ═══════════════════════════════════════════════════════════════════════════

# Install cross-compilation targets for GitHub Actions compatibility
setup-cross:
    @printf "\033[0;34m Installing cross-compilation targets...\033[0m\n"
    rustup target add x86_64-unknown-linux-gnu
    rustup target add aarch64-unknown-linux-gnu
    rustup target add x86_64-pc-windows-msvc
    @printf "\033[0;32m✅ Cross-compilation targets installed\033[0m\n"

# Cross-compile check for GitHub Actions compatibility (using CI pipeline script)
check-cross:
    @printf "\033[0;34m Cross-compilation validation using CI pipeline...\033[0m\n"
    rust-script scripts/ci-pipeline.rs cross-check

# ═══════════════════════════════════════════════════════════════════════════
# Development Environment Management (Rust-based)
# ═══════════════════════════════════════════════════════════════════════════

# Complete development environment setup
devenv-setup:
    @printf "\033[0;34m🚀 Setting up complete development environment...\033[0m\n"
    cargo run -p ttapi-devenv -- setup

# Install all development tools (core + refactoring)
devenv-tools:
    @printf "\033[0;34m🛠️ Installing all development tools...\033[0m\n"
    cargo run -p ttapi-devenv -- tools

# Install core development tools only
devenv-tools-core:
    @printf "\033[0;34m🔧 Installing core development tools...\033[0m\n"
    cargo run -p ttapi-devenv -- tools --tool core

# Install refactoring and analysis tools only
devenv-tools-refactoring:
    @printf "\033[0;34m🔍 Installing refactoring and analysis tools...\033[0m\n"
    cargo run -p ttapi-devenv -- tools --tool refactoring

# Install specific development tool
devenv-tool TOOL:
    @printf "\033[0;34m📦 Installing specific tool: {{ TOOL }}...\033[0m\n"
    cargo run -p ttapi-devenv -- tools --tool {{ TOOL }}

# Preview development tools installation (dry run)
devenv-tools-preview:
    @printf "\033[0;34m👀 Previewing development tools installation...\033[0m\n"
    cargo run -p ttapi-devenv -- --dry-run tools

# Add legal headers to source files (Rust, TOML, Markdown)
devenv-legal-headers:
    @printf "\033[0;34m🏢 Adding legal headers to source files...\033[0m\n"
    cargo run -p ttapi-devenv -- legal headers

# Complete legal compliance for all file types
devenv-legal-complete:
    @printf "\033[0;34m📋 Complete legal compliance setup...\033[0m\n"
    cargo run -p ttapi-devenv -- legal complete

# Validate legal compliance status
devenv-legal-validate:
    @printf "\033[0;34m🔍 Validating legal compliance...\033[0m\n"
    cargo run -p ttapi-devenv -- legal validate

# Preview legal compliance changes (dry run)
devenv-legal-preview:
    @printf "\033[0;34m👀 Previewing legal compliance changes...\033[0m\n"
    cargo run -p ttapi-devenv -- --dry-run legal headers

# Setup CI/CD pipeline configuration
devenv-ci-setup:
    @printf "\033[0;34m🚀 Setting up CI/CD pipeline...\033[0m\n"
    cargo run -p ttapi-devenv -- ci setup

# Validate CI configuration
devenv-ci-validate:
    @printf "\033[0;34m🔍 Validating CI configuration...\033[0m\n"
    cargo run -p ttapi-devenv -- ci validate

# Run local CI pipeline
devenv-ci-run:
    @printf "\033[0;34m⚡ Running local CI pipeline...\033[0m\n"
    cargo run -p ttapi-devenv -- ci run

# Setup monitoring infrastructure
devenv-monitoring-setup:
    @printf "\033[0;34m📊 Setting up monitoring infrastructure...\033[0m\n"
    cargo run -p ttapi-devenv -- monitoring setup

# Setup performance monitoring only
devenv-monitoring-performance:
    @printf "\033[0;34m📈 Setting up performance monitoring...\033[0m\n"
    cargo run -p ttapi-devenv -- monitoring performance

# Setup log monitoring only
devenv-monitoring-logs:
    @printf "\033[0;34m📋 Setting up log monitoring...\033[0m\n"
    cargo run -p ttapi-devenv -- monitoring logs

# Validate monitoring configuration
devenv-monitoring-validate:
    @printf "\033[0;34m🔍 Validating monitoring configuration...\033[0m\n"
    cargo run -p ttapi-devenv -- monitoring validate

# Preview CI/CD setup (dry run)
devenv-ci-preview:
    @printf "\033[0;34m👀 Previewing CI/CD setup...\033[0m\n"
    cargo run -p ttapi-devenv -- --dry-run ci setup

# Preview monitoring setup (dry run)
devenv-monitoring-preview:
    @printf "\033[0;34m👀 Previewing monitoring setup...\033[0m\n"
    cargo run -p ttapi-devenv -- --dry-run monitoring setup

# Validate production readiness
devenv-validate-production:
    @printf "\033[0;34m🚀 Validating production readiness...\033[0m\n"
    cargo run -p ttapi-devenv -- validate production

# Validate code quality improvements
devenv-validate-improvements:
    @printf "\033[0;34m🔍 Validating code quality improvements...\033[0m\n"
    cargo run -p ttapi-devenv -- validate improvements

# Validate workspace structure
devenv-validate-workspace:
    @printf "\033[0;34m🏗️ Validating workspace structure...\033[0m\n"
    cargo run -p ttapi-devenv -- validate workspace

# Validate all aspects
devenv-validate-all:
    @printf "\033[0;34m📊 Running comprehensive validation...\033[0m\n"
    cargo run -p ttapi-devenv -- validate all

# Run unit tests
devenv-test-unit:
    @printf "\033[0;34m🧪 Running unit tests...\033[0m\n"
    cargo run -p ttapi-devenv -- test unit

# Run functional tests
devenv-test-functional:
    @printf "\033[0;34m⚙️ Running functional tests...\033[0m\n"
    cargo run -p ttapi-devenv -- test functional

# Run integration tests
devenv-test-integration:
    @printf "\033[0;34m🔗 Running integration tests...\033[0m\n"
    cargo run -p ttapi-devenv -- test integration

# Run performance tests
devenv-test-performance:
    @printf "\033[0;34m⚡ Running performance tests...\033[0m\n"
    cargo run -p ttapi-devenv -- test performance

# Run all test categories
devenv-test-all:
    @printf "\033[0;34m🎯 Running all test categories...\033[0m\n"
    cargo run -p ttapi-devenv -- test all

# Run specific test category
devenv-test-category CATEGORY:
    @printf "\033[0;34m🔬 Running test category: {{ CATEGORY }}...\033[0m\n"
    cargo run -p ttapi-devenv -- test category {{ CATEGORY }}

# Complete development environment setup (minimal)
devenv-setup-minimal:
    @printf "\033[0;34m⚡ Setting up minimal development environment...\033[0m\n"
    cargo run -p ttapi-devenv -- setup --minimal

# Create software release
devenv-release VERSION:
    @printf "\033[0;34m📦 Creating release: {{ VERSION }}...\033[0m\n"
    cargo run -p ttapi-devenv -- release {{ VERSION }}

# Rollback to previous version
devenv-rollback:
    @printf "\033[0;34m⏪ Rolling back to previous version...\033[0m\n"
    cargo run -p ttapi-devenv -- rollback

# Rollback to specific backup
devenv-rollback-target TARGET:
    @printf "\033[0;34m⏪ Rolling back to: {{ TARGET }}...\033[0m\n"
    cargo run -p ttapi-devenv -- rollback --target {{ TARGET }}

# Preview setup (dry run)
devenv-setup-preview:
    @printf "\033[0;34m👀 Previewing development environment setup...\033[0m\n"
    cargo run -p ttapi-devenv -- --dry-run setup

# Preview release creation (dry run)
devenv-release-preview VERSION:
    @printf "\033[0;34m👀 Previewing release creation: {{ VERSION }}...\033[0m\n"
    cargo run -p ttapi-devenv -- --dry-run release {{ VERSION }}

# ═══════════════════════════════════════════════════════════════════════════
# Tool Installation & Management
# ═══════════════════════════════════════════════════════════════════════════

# Install all modern development tools
install-tools:
    @printf "\033[0;34m🔧 Installing modern development tools...\033[0m\n"
    @printf "\033[1;33mUsing cargo-binstall for fast installation...\033[0m\n"
    cargo binstall cargo-nextest cargo-llvm-cov cargo-audit cargo-watch just --no-confirm
    @printf "\033[0;32m✅ All modern tools installed successfully!\033[0m\n"

# Update all development tools to latest versions

# WHY: cargo install-update is more efficient than reinstalling everything
update-tools:
    @printf "\033[0;34m🔄 Updating development tools...\033[0m\n"
    @if command -v cargo-install-update >/dev/null 2>&1; then \
        cargo install-update -a; \
    else \
        printf "\033[1;33mInstalling cargo-update for efficient updates...\033[0m\n"; \
        just _install-if-missing cargo-update cargo-update; \
        cargo install-update -a; \
    fi
    @printf "\033[0;32m✅ All tools updated to latest versions!\033[0m\n"

# ═══════════════════════════════════════════════════════════════════════════
# Binary Deployment
# ═══════════════════════════════════════════════════════════════════════════

# Cross-platform binary deployment with intelligent path handling
copy-binary profile:
    #!/usr/bin/env bash
    printf "\033[0;34m📦 Cross-platform binary deployment ({{ profile }})...\033[0m\n"

    # Platform-specific binary naming
    if [[ "$OSTYPE" == "msys"* ]] || [[ "$OSTYPE" == "cygwin"* ]] || [[ -n "$WINDIR" ]]; then
        TARGET_BINARY="tt.exe"
        printf "\033[0;34m  → Windows detected, using tt.exe\033[0m\n"
    else
        TARGET_BINARY="tt"
        printf "\033[0;34m  → Unix-like system detected, using tt\033[0m\n"
    fi

    # Use standard per-project target directory (Rust Master approach)
    SOURCE_DIR="target/{{ profile }}"
    printf "\033[0;34m  → Using standard target directory\033[0m\n"

    # Determine source binary path
    SOURCE_PATH="$SOURCE_DIR/$TARGET_BINARY"

    # Validate source binary exists
    if [[ ! -f "$SOURCE_PATH" ]]; then
        printf "\033[0;31m❌ Binary not found: $SOURCE_PATH\033[0m\n"
        printf "\033[1;33m💡 Run 'just build' first\033[0m\n"
        exit 1
    fi

    # Create ~/bin if it doesn't exist
    mkdir -p ~/bin

    # Copy binary with validation
    DEST_PATH="$HOME/bin/$TARGET_BINARY"
    printf "\033[0;34m  → Source: $SOURCE_PATH\033[0m\n"
    printf "\033[0;34m  → Destination: $DEST_PATH\033[0m\n"

    if cp "$SOURCE_PATH" "$DEST_PATH" && chmod +x "$DEST_PATH"; then
        printf "\033[0;32m✅ Binary deployed to ~/bin/$TARGET_BINARY\033[0m\n"

        # Set extended attributes (macOS/Linux)
        if command -v xattr >/dev/null 2>&1; then
            xattr -w com.ttapi.buildtype "{{ profile }}" "$DEST_PATH" 2>/dev/null || true
            printf "\033[0;32m🏷️  Extended attributes set\033[0m\n"
        fi
    else
        printf "\033[0;31m❌ Failed to copy binary\033[0m\n"
        exit 1
    fi

    # PATH guidance
    if ! echo "$PATH" | grep -q "$HOME/bin"; then
        printf "\033[1;33m⚠️  ~/bin not in PATH. Add this to your shell profile:\033[0m\n"
        printf "   export PATH=\"\$HOME/bin:\$PATH\"\n"
    fi

# ═══════════════════════════════════════════════════════════════════════════
# Legal Compliance Tools (migrated from Makefile)
# ═══════════════════════════════════════════════════════════════════════════

# Add legal headers to all source files (using Rust implementation)
legal-headers:
    @printf "\033[0;34m🏢 Adding legal headers to all source files...\033[0m\n"
    cargo run -p ttapi-devenv -- legal headers

# Check REUSE compliance (requires reuse tool)
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

# Fix REUSE compliance issues
legal-fix:
    @printf "\033[0;34m🔧 Fixing REUSE compliance...\033[0m\n"
    @if command -v reuse >/dev/null 2>&1; then \
        reuse annotate --license "MPL-2.0 OR LicenseRef-TTAPI-Commercial" \
                      --copyright "SKY, LLC." \
                      --template rust \
                      src/**/*.rs crates/**/*.rs; \
    elif [ -f "/Users/rnio/Library/Python/3.12/bin/reuse" ]; then \
        /Users/rnio/Library/Python/3.12/bin/reuse annotate --license "MPL-2.0 OR LicenseRef-TTAPI-Commercial" \
                      --copyright "SKY, LLC." \
                      --template rust \
                      src/**/*.rs crates/**/*.rs; \
    else \
        printf "\033[0;31m❌ reuse tool not installed. Install with: just install-legal-tools\033[0m\n"; \
        printf "\033[1;33m🔄 Falling back to Rust implementation...\033[0m\n"; \
        cargo run -p ttapi-devenv -- legal headers; \
    fi

# Install legal compliance tools
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

# ═══════════════════════════════════════════════════════════════════════════════
# 🔬 ADVANCED ANALYSIS TOOLS - Security, Performance & Debugging
# ═══════════════════════════════════════════════════════════════════════════════

# Comprehensive security audit with multiple tools
audit-comprehensive:
    #!/usr/bin/env bash
    printf "\033[0;34m🔒 Comprehensive security audit...\033[0m\n"

    # Security vulnerability scanner
    printf "\033[0;34m  → Running cargo-audit (vulnerability scan)...\033[0m\n"
    if command -v cargo-audit >/dev/null 2>&1; then
        cargo audit
    else
        printf "\033[1;33m⚠️  cargo-audit not found, run 'just setup' first\033[0m\n"
    fi

    # Comprehensive dependency analysis
    printf "\033[0;34m  → Running cargo-deny (dependency analysis)...\033[0m\n"
    if command -v cargo-deny >/dev/null 2>&1; then
        cargo deny check
    else
        printf "\033[1;33m⚠️  cargo-deny not found, run 'just setup' first\033[0m\n"
    fi

    # Unsafe code detection
    printf "\033[0;34m  → Running cargo-geiger (unsafe code detection)...\033[0m\n"
    if command -v cargo-geiger >/dev/null 2>&1; then
        cargo geiger
    else
        printf "\033[1;33m⚠️  cargo-geiger not found, run 'just setup' first\033[0m\n"
    fi

# Dependency optimization and cleanup
deps-optimize:
    #!/usr/bin/env bash
    printf "\033[0;34m🔧 Optimizing dependencies...\033[0m\n"

    # Find unused dependencies
    printf "\033[0;34m  → Finding unused dependencies...\033[0m\n"
    if command -v cargo-udeps >/dev/null 2>&1; then
        cargo +nightly udeps
    else
        printf "\033[1;33m⚠️  cargo-udeps not found, run 'just setup' first\033[0m\n"
    fi

    # Check for outdated dependencies
    printf "\033[0;34m  → Checking for outdated dependencies...\033[0m\n"
    if command -v cargo-outdated >/dev/null 2>&1; then
        cargo outdated
    else
        printf "\033[1;33m⚠️  cargo-outdated not found, run 'just setup' first\033[0m\n"
    fi

# Advanced debugging and analysis
debug-deep:
    #!/usr/bin/env bash
    printf "\033[0;34m🔬 Deep debugging and analysis...\033[0m\n"

    # Expand macros for debugging
    printf "\033[0;34m  → Expanding macros...\033[0m\n"
    if command -v cargo-expand >/dev/null 2>&1; then
        cargo expand
    else
        printf "\033[1;33m⚠️  cargo-expand not found, run 'just setup' first\033[0m\n"
    fi

    # Check for undefined behavior with Miri
    printf "\033[0;34m  → Running Miri (undefined behavior detection)...\033[0m\n"
    if rustup component list --installed | grep -q "miri"; then
        cargo +nightly miri test
    else
        printf "\033[1;33m⚠️  miri component not found, run 'just setup' first\033[0m\n"
    fi

# ═══════════════════════════════════════════════════════════════════════════════
# 📊 CI PIPELINE BENCHMARKING - Performance Analysis & Optimization
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Data-driven CI/CD optimization through comprehensive performance analysis
#
# BENCHMARKING PHILOSOPHY:
# • Measure, don't guess: Actual timing data drives optimization decisions
# • Compare approaches: Two-phase vs separate steps vs alternative strategies
# • Identify bottlenecks: Find the slowest parts of the development workflow
# • Platform-specific: Different platforms may have different optimal strategies
# • Reproducible: Consistent measurement methodology for reliable comparisons
#
# BENCHMARK TYPES:
# • Current workflow: Time our existing two-phase approach
# • Alternative workflow: Time separate individual steps
# • Comparison: Side-by-side analysis with clean state between runs
# • Advanced profiling: Deep performance analysis with specialized tools
# • CI analysis: Comprehensive project analysis with optimization recommendations
#
# MEASUREMENT STRATEGY:
# • Clean state: Remove build artifacts between benchmarks for fair comparison
# • Wall-clock time: Real-world timing that includes I/O and system overhead
# • Individual step timing: Identify specific bottlenecks in the workflow
# • Total workflow timing: End-to-end performance measurement
# • Platform detection: Account for different CPU cores and system capabilities
#
# WHY BENCHMARKING MATTERS:
# • Developer productivity: Faster workflows mean more iteration cycles
# • CI/CD optimization: Reduce build times in continuous integration
# • Resource utilization: Better use of available CPU cores and memory
# • Cost optimization: Faster builds reduce cloud CI/CD costs
# • Feedback loops: Quicker feedback improves development experience
# Benchmark Rust script workflow approach
# MEASURES: Rust script async/parallel workflow timing
# OUTPUT: Total execution time with detailed step timing

# CLEAN STATE: Ensures clean starting state for accurate measurement
benchmark-rust-script:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Benchmarking Rust script workflow approach...\033[0m\n"
    printf "\033[1;33m🚀 RUST SCRIPT: Async/parallel optimized workflow\033[0m\n"

    # Clean state for accurate measurement
    printf "\033[0;34m  → Cleaning for accurate measurement...\033[0m\n"
    cargo clean

    # Record start time
    START_TIME=$(date +%s)

    printf "\033[0;34m  → Running Rust script workflow (just go)...\033[0m\n"
    just go

    # Calculate total time
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))

    printf "\033[0;32m✅ Rust script workflow benchmark complete!\033[0m\n"
    printf "\033[0;34m📊 Performance summary:\033[0m\n"
    printf "\033[0;34m  → Total execution time: ${TOTAL_TIME}s\033[0m\n"
    printf "\033[0;34m  → Approach: Async/parallel with detailed step timing\033[0m\n"

# Benchmark JUST file workflow approach
# MEASURES: Traditional JUST file sequential workflow timing
# OUTPUT: Total execution time with overall timing only

# CLEAN STATE: Ensures clean starting state for accurate measurement
benchmark-justfile:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Benchmarking JUST file workflow approach...\033[0m\n"
    printf "\033[1;33m📋 JUST FILE: Traditional sequential workflow\033[0m\n"

    # Clean state for accurate measurement
    printf "\033[0;34m  → Cleaning for accurate measurement...\033[0m\n"
    cargo clean

    # Record start time
    START_TIME=$(date +%s)

    printf "\033[0;34m  → Running JUST file workflow (just go-justfile)...\033[0m\n"
    just go-justfile

    # Calculate total time
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))

    printf "\033[0;32m✅ JUST file workflow benchmark complete!\033[0m\n"
    printf "\033[0;34m📊 Performance summary:\033[0m\n"
    printf "\033[0;34m  → Total execution time: ${TOTAL_TIME}s\033[0m\n"
    printf "\033[0;34m  → Approach: Sequential with overall timing only\033[0m\n"

# Benchmark LLVM compilation reuse strategy
# FOCUS: Tests the efficiency of reusing LLVM compilation artifacts

# STRATEGY: Run coverage first, then reuse artifacts for subsequent steps
benchmark-llvm-reuse:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Benchmarking LLVM compilation reuse strategy...\033[0m\n"
    printf "\033[1;33m🎯 FOCUS: Testing LLVM artifact reuse efficiency\033[0m\n"

    # Record start time
    START_TIME=$(date +%s)

    # Clean state for accurate measurement
    printf "\033[0;34m  → Cleaning for accurate measurement...\033[0m\n"
    cargo clean

    # Step 1: Run coverage analysis (builds everything with LLVM instrumentation)
    printf "\033[0;34m  → Step 1: Coverage analysis (full LLVM compilation)...\033[0m\n"
    STEP1_START=$(date +%s)
    cargo llvm-cov nextest --workspace --all-features --all-targets --jobs 16 --no-report
    STEP1_END=$(date +%s)
    STEP1_TIME=$((STEP1_END - STEP1_START))

    # Step 2: Coverage data ready (HTML report skipped for speed)
    printf "\033[0;34m  → Step 2: Coverage data ready (HTML skipped for speed)...\033[0m\n"
    STEP2_START=$(date +%s)
    printf "\033[0;32m✅ Coverage data available (use 'just coverage-report' for HTML)\033[0m\n"
    STEP2_END=$(date +%s)
    STEP2_TIME=$((STEP2_END - STEP2_START))

    # Step 3: Run additional tests (REUSES compilation artifacts)
    printf "\033[0;34m  → Step 3: Additional test run (artifact reuse)...\033[0m\n"
    STEP3_START=$(date +%s)
    cargo llvm-cov nextest run --workspace --all-features --all-targets --jobs 16 --no-report
    STEP3_END=$(date +%s)
    STEP3_TIME=$((STEP3_END - STEP3_START))

    # Calculate total time
    END_TIME=$(date +%s)
    TOTAL_TIME=$((END_TIME - START_TIME))

    printf "\033[0;32m✅ LLVM reuse benchmark complete!\033[0m\n"
    printf "\033[0;34m📊 Performance breakdown:\033[0m\n"
    printf "\033[0;34m  → Step 1 (Full LLVM compilation): ${STEP1_TIME}s\033[0m\n"
    printf "\033[0;34m  → Step 2 (LLVM report reuse): ${STEP2_TIME}s\033[0m\n"
    printf "\033[0;34m  → Step 3 (Artifact reuse): ${STEP3_TIME}s\033[0m\n"
    printf "\033[0;34m  → Total execution time: ${TOTAL_TIME}s\033[0m\n"
    printf "\033[0;34m💡 Reuse efficiency: Steps 2+3 should be significantly faster than Step 1\033[0m\n"

# ═══════════════════════════════════════════════════════════════════════════════
# ⚡ PURE SPEED OPTIMIZATION TESTING - TIMING ONLY
# ═══════════════════════════════════════════════════════════════════════════════
# GOAL: Find the fastest way to run comprehensive Phase 1 + Phase 2 workflow
# FOCUS: Pure timing - no compilation details, just speed optimization
# WORKFLOW COMPARISON - Compare Rust script vs JUST file approaches
# MEASURES: Complete CI pipeline with different orchestration approaches

# GOAL: Compare Rust script (async/parallel) vs JUST file (sequential) workflows
benchmark-flows:
    #!/usr/bin/env bash
    printf "\033[0;34m⚡ Workflow Orchestration Comparison...\033[0m\n"
    printf "\033[1;33m🎯 GOAL: Compare Rust script vs JUST file workflow approaches\033[0m\n"
    printf "\033[0;34m📋 SAME LOGIC: Both run identical steps with same commands\033[0m\n"
    printf "\033[0;34m📋 ONLY DIFFERENCE: Orchestration approach (async/parallel vs sequential)\033[0m\n"

    # Test 1: Rust script approach (async/parallel)
    printf "\033[0;34m\n🔄 Test 1: Rust Script Workflow (just go)\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"

    cargo clean >/dev/null 2>&1
    TEST1_START=$(date +%s)

    printf "\033[0;34m  → Running Rust script workflow with async/parallel optimization...\033[0m\n"
    if just go >/dev/null 2>&1; then
        TEST1_END=$(date +%s)
        TEST1_TOTAL=$((TEST1_END - TEST1_START))
        printf "\033[0;32m  ✅ Rust script completed successfully\033[0m\n"
    else
        TEST1_END=$(date +%s)
        TEST1_TOTAL=-1
        printf "\033[0;31m  ❌ Rust script failed\033[0m\n"
    fi

    # Test 2: JUST file approach (sequential)
    printf "\033[0;34m\n🔄 Test 2: JUST File Workflow (just go-justfile)\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"

    cargo clean >/dev/null 2>&1
    TEST2_START=$(date +%s)

    printf "\033[0;34m  → Running JUST file workflow with sequential execution...\033[0m\n"
    if just go-justfile >/dev/null 2>&1; then
        TEST2_END=$(date +%s)
        TEST2_TOTAL=$((TEST2_END - TEST2_START))
        printf "\033[0;32m  ✅ JUST file workflow completed successfully\033[0m\n"
    else
        TEST2_END=$(date +%s)
        TEST2_TOTAL=-1
        printf "\033[0;31m  ❌ JUST file workflow failed\033[0m\n"
    fi

    # Results - Workflow Orchestration Comparison
    printf "\033[0;32m\n⚡ WORKFLOW ORCHESTRATION RESULTS:\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"

    # Handle failures
    if [ "$TEST1_TOTAL" -eq -1 ] && [ "$TEST2_TOTAL" -eq -1 ]; then
        printf "\033[0;31m❌ Both workflows failed - check for errors\033[0m\n"
        printf "\033[0;34m💡 Run 'just go' and 'just go-justfile' individually to debug\033[0m\n"
        exit 1
    elif [ "$TEST1_TOTAL" -eq -1 ]; then
        printf "\033[0;31m📊 Rust Script (async/parallel):  FAILED\033[0m\n"
        printf "\033[0;34m📊 JUST File (sequential):        ${TEST2_TOTAL}s\033[0m\n"
        printf "\033[0;32m🏆 WINNER: JUST File (just go-justfile) - Rust script failed\033[0m\n"
        printf "\033[0;34m💡 RECOMMENDATION: Fix Rust script errors, then re-run benchmark\033[0m\n"
        exit 1
    elif [ "$TEST2_TOTAL" -eq -1 ]; then
        printf "\033[0;34m📊 Rust Script (async/parallel):  ${TEST1_TOTAL}s\033[0m\n"
        printf "\033[0;31m📊 JUST File (sequential):        FAILED\033[0m\n"
        printf "\033[0;32m🏆 WINNER: Rust Script (just go) - JUST file failed\033[0m\n"
        printf "\033[0;34m💡 RECOMMENDATION: Fix JUST file errors, then re-run benchmark\033[0m\n"
        exit 1
    else
        # Both succeeded - normal comparison
        printf "\033[0;34m📊 Rust Script (async/parallel):  ${TEST1_TOTAL}s\033[0m\n"
        printf "\033[0;34m📊 JUST File (sequential):        ${TEST2_TOTAL}s\033[0m\n"

        # Determine winner
        if [ "$TEST1_TOTAL" -lt "$TEST2_TOTAL" ]; then
            WINNER="Rust Script"
            SAVINGS=$((TEST2_TOTAL - TEST1_TOTAL))
            PERCENT_SAVINGS=$(echo "scale=1; $SAVINGS * 100 / $TEST2_TOTAL" | bc -l 2>/dev/null || echo "N/A")
            printf "\033[0;32m🏆 WINNER: Rust Script (just go)\033[0m\n"
            printf "\033[0;32m💰 SAVINGS: ${SAVINGS}s faster (${PERCENT_SAVINGS}%% improvement)\033[0m\n"
            printf "\033[0;34m💡 RECOMMENDATION: Use 'just go' for optimal performance\033[0m\n"
        elif [ "$TEST2_TOTAL" -lt "$TEST1_TOTAL" ]; then
            WINNER="JUST File"
            SAVINGS=$((TEST1_TOTAL - TEST2_TOTAL))
            PERCENT_SAVINGS=$(echo "scale=1; $SAVINGS * 100 / $TEST1_TOTAL" | bc -l 2>/dev/null || echo "N/A")
            printf "\033[0;32m🏆 WINNER: JUST File (just go-justfile)\033[0m\n"
            printf "\033[0;32m💰 SAVINGS: ${SAVINGS}s faster (${PERCENT_SAVINGS}%% improvement)\033[0m\n"
            printf "\033[0;34m💡 RECOMMENDATION: Use 'just go-justfile' for optimal performance\033[0m\n"
        else
            printf "\033[0;33m🤝 TIE: Both approaches have identical performance\033[0m\n"
            printf "\033[0;34m💡 RECOMMENDATION: Use either approach (no performance difference)\033[0m\n"
        fi
    fi

    printf "\033[0;34m\n📋 COMPARISON SUMMARY:\033[0m\n"
    printf "\033[0;34m  → Same logic: Both run identical steps with same commands\033[0m\n"
    printf "\033[0;34m  → Only difference: Orchestration (async/parallel vs sequential)\033[0m\n"
    printf "\033[0;34m  → Winner shows which workflow orchestration is faster\033[0m\n"

# Compare both workflow orchestration approaches
benchmark-both:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Comprehensive workflow orchestration comparison...\033[0m\n"
    printf "\033[1;33m⚠️  This will run both workflows and compare performance\033[0m\n"

    # Clean state for fair comparison
    printf "\033[0;34m  → Cleaning build artifacts for fair comparison...\033[0m\n"
    just clean

    printf "\033[0;34m\n🔄 Running Benchmark 1: Rust Script Workflow\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"
    just benchmark-rust-script

    # Clean state between benchmarks
    printf "\033[0;34m  → Cleaning for second benchmark...\033[0m\n"
    just clean

    printf "\033[0;34m\n🔄 Running Benchmark 2: JUST File Workflow\033[0m\n"
    printf "═══════════════════════════════════════════════════════\n"
    just benchmark-justfile

    printf "\033[0;32m\n🎯 Benchmark Comparison Complete!\033[0m\n"
    printf "\033[0;34m📊 Review the timing results above to compare approaches\033[0m\n"
    printf "\033[0;34m💡 Consider the approach that best fits your development workflow\033[0m\n"

# Performance profiling with advanced tools
benchmark-advanced:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 Advanced performance profiling...\033[0m\n"

    # Check for criterion benchmarking
    if command -v cargo-criterion >/dev/null 2>&1; then
        printf "\033[0;34m  → Running criterion benchmarks...\033[0m\n"
        cargo criterion
    else
        printf "\033[1;33m⚠️  cargo-criterion not found, run 'just setup' first\033[0m\n"
    fi

    # Memory usage analysis
    printf "\033[0;34m  → Analyzing memory usage patterns...\033[0m\n"
    if command -v valgrind >/dev/null 2>&1; then
        printf "\033[0;34m    → Running valgrind analysis...\033[0m\n"
        valgrind --tool=massif cargo test
    else
        printf "\033[1;33m⚠️  valgrind not available, skipping memory analysis\033[0m\n"
    fi

    # Build time analysis
    printf "\033[0;34m  → Analyzing build time breakdown...\033[0m\n"
    cargo build --timings

    printf "\033[0;32m✅ Advanced profiling complete!\033[0m\n"
    printf "\033[0;34m📊 Check target/cargo-timings/ for detailed build analysis\033[0m\n"

# ═══════════════════════════════════════════════════════════════════════════════
# 🚀 UFFS Performance Benchmarks (Rust vs C++ Comparison)
# ═══════════════════════════════════════════════════════════════════════════════
#
# These benchmarks compare the Rust implementation against the C++ reference
# implementation (uffs.com) to track optimization progress.
#
# Usage:
#   just bench-vs-cpp          # Compare Rust vs C++ for *.txt search
#   just bench-vs-cpp-drive C  # Compare on specific drive
#   just bench-micro           # Run criterion micro-benchmarks
#   just bench-search          # End-to-end search benchmark
#   just bench-all-compare     # Run all benchmarks and compare to baseline
#   just bench-update-baseline # Update baseline with current results
#
# ═══════════════════════════════════════════════════════════════════════════════

# Compare Rust vs C++ implementation (integration benchmark)
bench-vs-cpp drive="C":
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🏁 UFFS Benchmark: Rust vs C++ Comparison\033[0m\n"
    printf "\033[0;34m   Drive: {{drive}}:\033[0m\n"
    printf "\033[0;34m   Pattern: *.txt\033[0m\n"
    echo ""

    # Check for C++ reference implementation
    CPP_UFFS="$HOME/bin/uffs.com"
    if [[ ! -f "$CPP_UFFS" ]]; then
        printf "\033[1;31m❌ C++ reference not found at $CPP_UFFS\033[0m\n"
        printf "\033[0;33m   Please ensure uffs.com is in ~/bin/\033[0m\n"
        exit 1
    fi

    # Build Rust release version
    printf "\033[0;34m📦 Building Rust release...\033[0m\n"
    cargo build --release -p uffs-cli --quiet
    RUST_UFFS="target/release/uffs.exe"
    if [[ ! -f "$RUST_UFFS" ]]; then
        RUST_UFFS="target/release/uffs"
    fi

    # Create temp files for output comparison
    TEMP_DIR=$(mktemp -d)
    CPP_OUTPUT="$TEMP_DIR/cpp_output.txt"
    RUST_OUTPUT="$TEMP_DIR/rust_output.txt"
    trap "rm -rf $TEMP_DIR" EXIT

    # Benchmark C++ version
    printf "\033[0;34m\n⏱️  Benchmarking C++ (uffs.com)...\033[0m\n"
    CPP_START=$(date +%s.%N)
    "$CPP_UFFS" "*.txt" --drive {{drive}} > "$CPP_OUTPUT" 2>&1 || true
    CPP_END=$(date +%s.%N)
    CPP_TIME=$(echo "$CPP_END - $CPP_START" | bc)
    CPP_COUNT=$(wc -l < "$CPP_OUTPUT" | tr -d ' ')
    printf "\033[0;32m   C++ Time: ${CPP_TIME}s, Files: ${CPP_COUNT}\033[0m\n"

    # Benchmark Rust version
    printf "\033[0;34m\n⏱️  Benchmarking Rust (uffs)...\033[0m\n"
    RUST_START=$(date +%s.%N)
    RUST_LOG=error "$RUST_UFFS" "*.txt" --drive {{drive}} > "$RUST_OUTPUT" 2>&1 || true
    RUST_END=$(date +%s.%N)
    RUST_TIME=$(echo "$RUST_END - $RUST_START" | bc)
    RUST_COUNT=$(wc -l < "$RUST_OUTPUT" | tr -d ' ')
    printf "\033[0;32m   Rust Time: ${RUST_TIME}s, Files: ${RUST_COUNT}\033[0m\n"

    # Calculate ratio
    if (( $(echo "$CPP_TIME > 0" | bc -l) )); then
        RATIO=$(echo "scale=2; $RUST_TIME / $CPP_TIME" | bc)
        SPEEDUP=$(echo "scale=2; $CPP_TIME / $RUST_TIME" | bc)
    else
        RATIO="N/A"
        SPEEDUP="N/A"
    fi

    # Compare file counts
    COUNT_DIFF=$((RUST_COUNT - CPP_COUNT))

    # Print summary
    printf "\033[0;34m\n═══════════════════════════════════════════════════════════\033[0m\n"
    printf "\033[0;34m📊 BENCHMARK RESULTS\033[0m\n"
    printf "\033[0;34m═══════════════════════════════════════════════════════════\033[0m\n"
    printf "%-20s %15s %15s\n" "Metric" "C++" "Rust"
    printf "%-20s %15s %15s\n" "────────────────────" "───────────────" "───────────────"
    printf "%-20s %14.2fs %14.2fs\n" "Time" "$CPP_TIME" "$RUST_TIME"
    printf "%-20s %15s %15s\n" "Files Found" "$CPP_COUNT" "$RUST_COUNT"
    printf "\033[0;34m═══════════════════════════════════════════════════════════\033[0m\n"

    if (( $(echo "$SPEEDUP > 1" | bc -l) )); then
        printf "\033[0;32m✅ Rust is ${SPEEDUP}x FASTER than C++\033[0m\n"
    elif (( $(echo "$SPEEDUP < 1" | bc -l) )); then
        printf "\033[1;33m⚠️  Rust is ${RATIO}x SLOWER than C++\033[0m\n"
    else
        printf "\033[0;34m   Rust and C++ are approximately equal\033[0m\n"
    fi

    if [[ $COUNT_DIFF -ne 0 ]]; then
        printf "\033[1;33m⚠️  File count difference: $COUNT_DIFF (Rust - C++)\033[0m\n"
        printf "\033[0;33m   This may indicate path resolution issues\033[0m\n"
    else
        printf "\033[0;32m✅ File counts match!\033[0m\n"
    fi

    # Save results to JSON
    RESULTS_DIR="benchmarks"
    mkdir -p "$RESULTS_DIR"
    TIMESTAMP=$(date +%Y%m%d_%H%M%S)
    RESULTS_FILE="$RESULTS_DIR/bench_${TIMESTAMP}.json"
    echo "{" > "$RESULTS_FILE"
    echo "    \"timestamp\": \"$(date -Iseconds)\"," >> "$RESULTS_FILE"
    echo "    \"drive\": \"{{drive}}\"," >> "$RESULTS_FILE"
    echo "    \"pattern\": \"*.txt\"," >> "$RESULTS_FILE"
    echo "    \"cpp\": {" >> "$RESULTS_FILE"
    echo "        \"time_seconds\": $CPP_TIME," >> "$RESULTS_FILE"
    echo "        \"file_count\": $CPP_COUNT" >> "$RESULTS_FILE"
    echo "    }," >> "$RESULTS_FILE"
    echo "    \"rust\": {" >> "$RESULTS_FILE"
    echo "        \"time_seconds\": $RUST_TIME," >> "$RESULTS_FILE"
    echo "        \"file_count\": $RUST_COUNT" >> "$RESULTS_FILE"
    echo "    }," >> "$RESULTS_FILE"
    echo "    \"speedup\": $SPEEDUP," >> "$RESULTS_FILE"
    echo "    \"file_count_diff\": $COUNT_DIFF" >> "$RESULTS_FILE"
    echo "}" >> "$RESULTS_FILE"
    printf "\033[0;34m📁 Results saved to: $RESULTS_FILE\033[0m\n"

# Run criterion micro-benchmarks
bench-micro:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔬 Running Criterion Micro-Benchmarks...\033[0m\n"
    echo ""

    # Run uffs-core benchmarks
    printf "\033[0;34m📊 uffs-core benchmarks (pattern, query, path resolution)...\033[0m\n"
    cargo bench -p uffs-core --bench search_benchmarks

    # Run uffs-mft benchmarks (if not placeholder)
    printf "\033[0;34m\n📊 uffs-mft benchmarks (MFT reading)...\033[0m\n"
    cargo bench -p uffs-mft --bench mft_read

    printf "\033[0;32m\n✅ Micro-benchmarks complete!\033[0m\n"
    printf "\033[0;34m📊 View HTML report: target/criterion/report/index.html\033[0m\n"

# End-to-end search benchmark using cached MFT
bench-search drive="C":
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔍 End-to-End Search Benchmark\033[0m\n"
    printf "\033[0;34m   Drive: {{drive}}:\033[0m\n"
    echo ""

    # Build release
    cargo build --release -p uffs-cli --quiet
    RUST_UFFS="target/release/uffs.exe"
    if [[ ! -f "$RUST_UFFS" ]]; then
        RUST_UFFS="target/release/uffs"
    fi

    # Run multiple patterns and measure
    PATTERNS=("*.txt" "*.rs" "*.log" "*.dll" "*.exe")

    printf "\033[0;34m%-20s %15s %15s\033[0m\n" "Pattern" "Time (s)" "Files"
    printf "%-20s %15s %15s\n" "────────────────────" "───────────────" "───────────────"

    for PATTERN in "${PATTERNS[@]}"; do
        START=$(date +%s.%N)
        COUNT=$(RUST_LOG=error "$RUST_UFFS" "$PATTERN" --drive {{drive}} 2>/dev/null | wc -l | tr -d ' ')
        END=$(date +%s.%N)
        TIME=$(echo "$END - $START" | bc)
        printf "%-20s %14.2fs %15s\n" "$PATTERN" "$TIME" "$COUNT"
    done

    printf "\033[0;32m\n✅ Search benchmark complete!\033[0m\n"

# Run all benchmarks and compare to baseline
bench-all-compare:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📊 Running All Benchmarks with Baseline Comparison...\033[0m\n"
    echo ""

    BASELINE_FILE="benchmarks/baseline.json"

    # Run vs-cpp benchmark
    just bench-vs-cpp C

    # Compare to baseline if exists
    if [[ -f "$BASELINE_FILE" ]]; then
        printf "\033[0;34m\n📈 Comparing to baseline...\033[0m\n"
        BASELINE_TIME=$(jq -r '.rust.time_seconds' "$BASELINE_FILE")
        LATEST_FILE=$(ls -t benchmarks/bench_*.json 2>/dev/null | head -1)
        if [[ -n "$LATEST_FILE" ]]; then
            CURRENT_TIME=$(jq -r '.rust.time_seconds' "$LATEST_FILE")
            CHANGE=$(echo "scale=2; (($CURRENT_TIME - $BASELINE_TIME) / $BASELINE_TIME) * 100" | bc)
            if (( $(echo "$CHANGE > 10" | bc -l) )); then
                printf "\033[1;31m⚠️  REGRESSION: ${CHANGE}%% slower than baseline!\033[0m\n"
            elif (( $(echo "$CHANGE < -10" | bc -l) )); then
                printf "\033[0;32m🎉 IMPROVEMENT: ${CHANGE}%% faster than baseline!\033[0m\n"
            else
                printf "\033[0;34m   Within 10%% of baseline (${CHANGE}%%)\033[0m\n"
            fi
        fi
    else
        printf "\033[1;33m⚠️  No baseline found. Run 'just bench-update-baseline' to create one.\033[0m\n"
    fi

    printf "\033[0;32m\n✅ All benchmarks complete!\033[0m\n"

# Update baseline with current benchmark results
bench-update-baseline:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m📝 Updating Benchmark Baseline...\033[0m\n"

    # Run fresh benchmark
    just bench-vs-cpp C

    # Copy latest result as baseline
    LATEST_FILE=$(ls -t benchmarks/bench_*.json 2>/dev/null | head -1)
    if [[ -n "$LATEST_FILE" ]]; then
        cp "$LATEST_FILE" "benchmarks/baseline.json"
        printf "\033[0;32m✅ Baseline updated from: $LATEST_FILE\033[0m\n"
        printf "\033[0;34m   Baseline saved to: benchmarks/baseline.json\033[0m\n"
    else
        printf "\033[1;31m❌ No benchmark results found!\033[0m\n"
        exit 1
    fi

# Quick benchmark help
bench-help:
    #!/usr/bin/env bash
    printf "\033[0;34m🏁 UFFS Benchmark Commands\033[0m\n"
    echo ""
    printf "\033[0;32m  Integration Benchmarks (Rust vs C++):\033[0m\n"
    echo "    just bench-vs-cpp          # Compare on C: drive"
    echo "    just bench-vs-cpp D        # Compare on D: drive"
    echo ""
    printf "\033[0;32m  Micro-Benchmarks (Criterion):\033[0m\n"
    echo "    just bench-micro           # Run all criterion benchmarks"
    echo ""
    printf "\033[0;32m  End-to-End Benchmarks:\033[0m\n"
    echo "    just bench-search          # Search benchmark with multiple patterns"
    echo "    just bench-search D        # Search benchmark on D: drive"
    echo ""
    printf "\033[0;32m  Baseline Management:\033[0m\n"
    echo "    just bench-all-compare     # Run all and compare to baseline"
    echo "    just bench-update-baseline # Update baseline with current results"
    echo ""
    printf "\033[0;34m📊 Results are saved to: benchmarks/*.json\033[0m\n"
    printf "\033[0;34m📈 Criterion reports: target/criterion/report/index.html\033[0m\n"

# Comprehensive CI pipeline analysis and optimization recommendations
ci-analyze:
    #!/usr/bin/env bash
    printf "\033[0;34m🔬 Comprehensive CI Pipeline Analysis...\033[0m\n"

    # Analyze current project structure
    printf "\033[0;34m  → Analyzing project structure...\033[0m\n"
    CRATE_COUNT=$(find . -name "Cargo.toml" | wc -l | tr -d ' ')
    SOURCE_FILES=$(find . -name "*.rs" | wc -l | tr -d ' ')
    TEST_FILES=$(find . -name "*.rs" -path "*/tests/*" -o -name "*test*.rs" | wc -l | tr -d ' ')

    printf "\033[0;34m    → Crates: $CRATE_COUNT\033[0m\n"
    printf "\033[0;34m    → Source files: $SOURCE_FILES\033[0m\n"
    printf "\033[0;34m    → Test files: $TEST_FILES\033[0m\n"

    # Analyze dependencies
    printf "\033[0;34m  → Analyzing dependencies...\033[0m\n"
    if command -v cargo-tree >/dev/null 2>&1; then
        DEP_COUNT=$(cargo tree --depth 1 | grep -c "├\|└" || echo "0")
        printf "\033[0;34m    → Direct dependencies: $DEP_COUNT\033[0m\n"
    fi

    # Check for parallel execution capabilities
    printf "\033[0;34m  → Checking parallel execution capabilities...\033[0m\n"
    CPU_CORES=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo "unknown")
    printf "\033[0;34m    → Available CPU cores: $CPU_CORES\033[0m\n"

    # Analyze test execution patterns
    printf "\033[0;34m  → Analyzing test execution patterns...\033[0m\n"
    if command -v cargo-nextest >/dev/null 2>&1; then
        printf "\033[0;32m    ✅ Using cargo-nextest (optimized test runner)\033[0m\n"
    else
        printf "\033[1;33m    ⚠️  Using standard cargo test (consider cargo-nextest)\033[0m\n"
    fi

    # Check for incremental compilation
    printf "\033[0;34m  → Checking incremental compilation settings...\033[0m\n"
    if grep -q "incremental.*true" Cargo.toml 2>/dev/null; then
        printf "\033[0;32m    ✅ Incremental compilation enabled\033[0m\n"
    else
        printf "\033[1;33m    💡 Consider enabling incremental compilation for faster builds\033[0m\n"
    fi

    # Provide optimization recommendations
    printf "\033[0;32m\n🎯 CI Pipeline Optimization Recommendations:\033[0m\n"
    printf "\033[0;34m\n📊 Current Workflow Analysis:\033[0m\n"
    printf "  • Two-phase workflow: ✅ Excellent for fast-fail behavior\n"
    printf "  • Parallel test execution: ✅ Using --jobs 16\n"
    printf "  • Modern tooling: ✅ nextest, llvm-cov, clippy\n"

    printf "\033[0;34m\n🚀 Performance Optimization Opportunities:\033[0m\n"
    if [ "$CPU_CORES" != "unknown" ] && [ "$CPU_CORES" -gt 16 ]; then
        printf "  • Consider increasing --jobs from 16 to $CPU_CORES for better parallelism\n"
    fi
    printf "  • Use 'just benchmark-both' to compare workflow approaches\n"
    printf "  • Consider caching strategies for dependencies\n"
    printf "  • Monitor 'just benchmark-advanced' for bottlenecks\n"

    printf "\033[0;34m\n🔧 Tool Recommendations:\033[0m\n"
    printf "  • cargo-binstall: ✅ Faster tool installations\n"
    printf "  • cargo-nextest: ✅ Modern test runner with better output\n"
    printf "  • cargo-llvm-cov: ✅ Comprehensive coverage analysis\n"
    printf "  • sccache: 💡 Consider for distributed compilation caching\n"

    printf "\033[0;32m\n✅ CI Analysis complete! Use benchmarking tools for detailed performance data.\033[0m\n"

# ═══════════════════════════════════════════════════════════════════════════════
# 🚀 COMPILATION ACCELERATION - Advanced Performance Optimization
# ═══════════════════════════════════════════════════════════════════════════════

# Install and configure sccache for distributed compilation caching
setup-sccache:
    #!/usr/bin/env bash
    printf "\033[0;34m🚀 Setting up sccache for faster compilation...\033[0m\n"

    # Install sccache if not present
    if ! command -v sccache >/dev/null 2>&1; then
        printf "\033[0;34m  → Installing sccache...\033[0m\n"
        just _install-if-missing sccache sccache
    else
        printf "\033[0;32m  ✅ sccache already installed\033[0m\n"
    fi

    # Configure sccache environment
    printf "\033[0;34m  → Configuring sccache environment...\033[0m\n"

    # Set up sccache cache directory
    SCCACHE_DIR="$HOME/.cache/sccache"
    mkdir -p "$SCCACHE_DIR"

    printf "\033[0;34m  → Cache directory: $SCCACHE_DIR\033[0m\n"
    printf "\033[0;34m  → Setting RUSTC_WRAPPER=sccache\033[0m\n"

    # Show current cache stats
    if command -v sccache >/dev/null 2>&1; then
        printf "\033[0;34m  → Current cache statistics:\033[0m\n"
        RUSTC_WRAPPER=sccache sccache --show-stats || true
    fi

    printf "\033[0;32m✅ sccache setup complete!\033[0m\n"
    printf "\033[0;34m💡 To enable sccache, set: export RUSTC_WRAPPER=sccache\033[0m\n"
    printf "\033[0;34m💡 Add to your shell profile for permanent activation\033[0m\n"

# Show sccache statistics and performance impact
sccache-stats:
    #!/usr/bin/env bash
    printf "\033[0;34m📊 sccache compilation cache statistics...\033[0m\n"

    if ! command -v sccache >/dev/null 2>&1; then
        printf "\033[1;33m⚠️  sccache not installed. Run 'just setup-sccache' first.\033[0m\n"
        exit 1
    fi

    printf "\033[0;34m  → Cache statistics:\033[0m\n"
    RUSTC_WRAPPER=sccache sccache --show-stats

    printf "\033[0;34m  → Cache location:\033[0m\n"
    printf "    $HOME/.cache/sccache\n"

    if [ -d "$HOME/.cache/sccache" ]; then
        CACHE_SIZE=$(du -sh "$HOME/.cache/sccache" 2>/dev/null | cut -f1 || echo "unknown")
        printf "\033[0;34m  → Cache size: $CACHE_SIZE\033[0m\n"
    fi

    printf "\033[0;32m✅ sccache statistics complete!\033[0m\n"

# Clear sccache to free up disk space
sccache-clear:
    #!/usr/bin/env bash
    printf "\033[0;34m🧹 Clearing sccache compilation cache...\033[0m\n"

    if ! command -v sccache >/dev/null 2>&1; then
        printf "\033[1;33m⚠️  sccache not installed. Nothing to clear.\033[0m\n"
        exit 0
    fi

    # Show current cache size before clearing
    if [ -d "$HOME/.cache/sccache" ]; then
        CACHE_SIZE=$(du -sh "$HOME/.cache/sccache" 2>/dev/null | cut -f1 || echo "unknown")
        printf "\033[0;34m  → Current cache size: $CACHE_SIZE\033[0m\n"
    fi

    # Clear the cache
    RUSTC_WRAPPER=sccache sccache --zero-stats
    rm -rf "$HOME/.cache/sccache"
    mkdir -p "$HOME/.cache/sccache"

    printf "\033[0;32m✅ sccache cache cleared!\033[0m\n"

legal-setup: install-legal-tools legal-headers legal-check
    @printf "\033[0;32m🎉 Legal setup complete!\033[0m\n"
    @printf "\033[0;32m📋 All files now have proper legal headers\033[0m\n"
    @printf "\033[0;32m✅ REUSE compliance verified\033[0m\n"
    @echo ""
    @printf "\033[0;34mLegal Information:\033[0m\n"
    @echo "  Copyright: SKY, LLC."
    @echo "  Contact: skylegal@nios.net"
    @echo "  License: MPL-2.0 OR LicenseRef-TTAPI-Commercial"

# ═══════════════════════════════════════════════════════════════════════════════
# ⚠️  TESTING REQUIREMENTS BEFORE CHANGES
# ═══════════════════════════════════════════════════════════════════════════════
# This justfile must work on ALL platforms. Test these scenarios before changes:
#
# 🖥️  PLATFORMS TO TEST:
# ─────────────────────────────────────────────────────────────────────────────
# • Windows PowerShell + Git Bash
#   - Test: PowerShell fallback behavior
#   - Test: Git Bash compatibility with bash shebangs
#   - Test: Binary naming (tt.exe vs tt)
#   - Test: PATH setup guidance accuracy
#
# • macOS Terminal + Homebrew
#   - Test: Color output in Terminal.app
#   - Test: Homebrew tool installation
#   - Test: Extended attributes (xattr) functionality
#   - Test: CPU core detection (sysctl)
#
# • Linux (Ubuntu/Debian, RHEL/CentOS, Arch)
#   - Test: Package manager detection
#   - Test: Color output in various terminals
#   - Test: CPU core detection (nproc)
#   - Test: Tool installation across distributions
#
# 🧪 SCENARIOS TO VALIDATE:
# ─────────────────────────────────────────────────────────────────────────────
# • Fresh environment: just setup (installs all tools)
# • Complete workflow: just go (end-to-end testing)
# • No color mode: NO_COLOR=1 just go
# • Individual commands: just test, just build, etc.
# • Cross-platform binary deployment: just copy-binary debug
# • CI benchmarking: just benchmark-both
# • Analysis tools: just ci-analyze, just audit-comprehensive
#
# ✅ VALIDATION CHECKLIST:
# ─────────────────────────────────────────────────────────────────────────────
# • Colors display correctly (not as raw escape codes)
# • Fast-fail behavior works (stops on first error)
# • Cross-platform binary naming (tt vs tt.exe)
# • PATH setup guidance is accurate for each platform
# • Tool installation is idempotent (safe to re-run)
# • Helper functions work correctly (prefixed with underscore)
# • All recipes show in just --list (except helper functions)
# • Default recipe shows comprehensive help
# • Educational documentation is clear and helpful
#
# ═══════════════════════════════════════════════════════════════════════════════
# 🔬 PROFILING - Windows Performance Analysis
# ═══════════════════════════════════════════════════════════════════════════════
# WHY: Profile UFFS on Windows to identify performance bottlenecks
# WORKFLOW:
#   1. Build profiling binaries on Mac (with debug symbols, no LTO)
#   2. Copy to USB drive
#   3. Run samply on Windows to capture profile
#   4. Analyze profile.json on Mac with Firefox Profiler

# Build profiling binaries and copy to USB for Windows profiling
# USAGE: just profile-usb
# REQUIRES: USB drive mounted at /Volumes/UFFSPRO
profile-usb:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    printf "\033[0;34m  🔬 UFFS Profiling → USB Workflow\033[0m\n"
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    echo ""

    # Step 1: Build profiling binaries
    printf "\033[0;34mStep 1: Building profiling binaries...\033[0m\n"
    printf "\033[0;36m  → UFFS_PROFILING_BUILD=1 rust-script scripts/build-cross-all.rs\033[0m\n"
    UFFS_PROFILING_BUILD=1 rust-script scripts/build-cross-all.rs
    printf "\033[0;32m✅ Profiling binaries built\033[0m\n"
    echo ""

    # Step 2: Check for USB drive
    printf "\033[0;34mStep 2: Checking for USB drive...\033[0m\n"
    USB_PATHS=("/Volumes/UFFSPRO" "/Volumes/USB" "/Volumes/UNTITLED")
    USB_PATH=""
    for path in "${USB_PATHS[@]}"; do
        if [ -d "$path" ]; then
            USB_PATH="$path"
            break
        fi
    done

    if [ -z "$USB_PATH" ]; then
        printf "\033[1;33m⚠️  No USB drive found at common mount points.\033[0m\n"
        printf "\033[0;34m   Looking for mounted volumes...\033[0m\n"
        echo ""
        ls -la /Volumes/ 2>/dev/null || true
        echo ""
        printf "\033[1;33m   Please enter the USB mount path (e.g., /Volumes/UFFSPRO): \033[0m"
        read -r USB_PATH
        if [ ! -d "$USB_PATH" ]; then
            printf "\033[0;31m❌ USB path not found: $USB_PATH\033[0m\n"
            exit 1
        fi
    fi
    printf "\033[0;32m✅ USB found at: $USB_PATH\033[0m\n"
    echo ""

    # Step 3: Copy files to USB
    printf "\033[0;34mStep 3: Copying files to USB...\033[0m\n"

    DEST_DIR="$USB_PATH/uffs_profiling"
    mkdir -p "$DEST_DIR"

    # Profiling binaries are in the cargo target cache (not dist/)
    # This matches the CARGO_TARGET_DIR used by build-cross-all.rs
    SOURCE_DIR="$HOME/Library/Caches/uffs/target-xwin-release/x86_64-pc-windows-msvc/profiling"
    SOURCE_EXE="$SOURCE_DIR/uffs.exe"

    if [ ! -f "$SOURCE_EXE" ]; then
        printf "\033[0;31m   ❌ Profiling binary not found: $SOURCE_EXE\033[0m\n"
        printf "\033[0;33m   The build may have failed - check output above\033[0m\n"
        exit 1
    fi

    printf "\033[0;36m   Source: $SOURCE_DIR\033[0m\n"

    echo "   Copying uffs.exe ($(du -h "$SOURCE_EXE" | cut -f1))..."
    cp "$SOURCE_EXE" "$DEST_DIR/uffs.exe"
    printf "\033[0;32m   ✓ uffs.exe\033[0m\n"

    # Note: Skipping PDB - not needed for samply profiling (debug info embedded in binary)

    # Copy PowerShell script
    if [ -f "scripts/windows-profiling.ps1" ]; then
        cp "scripts/windows-profiling.ps1" "$DEST_DIR/run-profiling.ps1"
        printf "\033[0;32m   ✓ run-profiling.ps1\033[0m\n"
    fi

    echo ""
    printf "\033[0;32m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    printf "\033[0;32m  ✅ DONE! USB is ready for Windows profiling\033[0m\n"
    printf "\033[0;32m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    echo ""
    printf "\033[0;34m  Contents of $DEST_DIR:\033[0m\n"
    ls -lh "$DEST_DIR"
    echo ""
    printf "\033[1;33m  📋 ON WINDOWS:\033[0m\n"
    printf "\033[0;36m     Option 1: Right-click run-profiling.ps1 → 'Run with PowerShell'\033[0m\n"
    printf "\033[0;36m     Option 2: In PowerShell: G:\\uffs_profiling\\run-profiling.ps1\033[0m\n"
    echo ""
    printf "\033[1;33m  📋 BACK ON MAC:\033[0m\n"
    printf "\033[0;36m     samply load $USB_PATH/uffs_profiling/profile.json\033[0m\n"
    echo ""

# Load and analyze a profile from USB
# USAGE: just profile-load
# Copies profile_*.json, uffs.pdb, and summary files from USB to docs/profiles/
# Then opens the latest profile in Firefox Profiler with symbolication
#
# SYMBOLICATION:
# - uffs.pdb is copied alongside profiles for local symbol resolution
# - Microsoft Symbol Server is used for Windows library symbols
# - samply serves symbols to Firefox Profiler via local HTTP server
profile-load:
    #!/usr/bin/env bash
    set -euo pipefail
    printf "\033[0;34m🔬 Loading profiles from USB...\033[0m\n"

    # Find USB with profile files
    USB_DIR=""
    for dir in /Volumes/*/uffs_profiling; do
        if [ -d "$dir" ]; then
            USB_DIR="$dir"
            break
        fi
    done

    if [ -z "$USB_DIR" ]; then
        printf "\033[0;31m❌ No uffs_profiling folder found on USB\033[0m\n"
        printf "\033[0;34m   Looking for USB drives...\033[0m\n"
        ls -la /Volumes/ 2>/dev/null || true
        exit 1
    fi

    printf "\033[0;32m✅ Found USB at: $USB_DIR\033[0m\n"

    # Create docs/profiles directory if needed
    PROFILES_DIR="docs/profiles"
    mkdir -p "$PROFILES_DIR"

    # Copy PDB file (critical for symbolication!)
    COPIED=0
    if [ -f "$USB_DIR/uffs.pdb" ]; then
        printf "\033[0;34m   Copying uffs.pdb (for symbolication)...\033[0m\n"
        cp "$USB_DIR/uffs.pdb" "$PROFILES_DIR/uffs.pdb"
        COPIED=$((COPIED + 1))
        printf "\033[0;32m   ✓ uffs.pdb copied\033[0m\n"
    else
        printf "\033[0;33m   ⚠ uffs.pdb not found - profile will lack uffs.exe symbols\033[0m\n"
    fi

    # Copy all profile_*.json files from USB to docs/profiles/
    for profile in "$USB_DIR"/profile_*.json; do
        if [ -f "$profile" ]; then
            BASENAME=$(basename "$profile")
            DEST="$PROFILES_DIR/$BASENAME"
            if [ ! -f "$DEST" ]; then
                printf "\033[0;34m   Copying $BASENAME...\033[0m\n"
                cp "$profile" "$DEST"
                COPIED=$((COPIED + 1))
            else
                printf "\033[0;33m   Skipping $BASENAME (already exists)\033[0m\n"
            fi
        fi
    done

    # Copy all profile_summary_*.txt files from USB to docs/profiles/
    for summary in "$USB_DIR"/profile_summary_*.txt; do
        if [ -f "$summary" ]; then
            BASENAME=$(basename "$summary")
            DEST="$PROFILES_DIR/$BASENAME"
            if [ ! -f "$DEST" ]; then
                printf "\033[0;34m   Copying $BASENAME...\033[0m\n"
                cp "$summary" "$DEST"
                COPIED=$((COPIED + 1))
            else
                printf "\033[0;33m   Skipping $BASENAME (already exists)\033[0m\n"
            fi
        fi
    done

    # Also check for old-format profile.json
    if [ -f "$USB_DIR/profile.json" ]; then
        TIMESTAMP=$(date +"%Y-%m-%d_%H-%M-%S")
        DEST="$PROFILES_DIR/profile_imported_$TIMESTAMP.json"
        printf "\033[0;34m   Copying profile.json as profile_imported_$TIMESTAMP.json...\033[0m\n"
        cp "$USB_DIR/profile.json" "$DEST"
        COPIED=$((COPIED + 1))
    fi

    printf "\033[0;32m✅ Copied $COPIED file(s) to $PROFILES_DIR/\033[0m\n"

    # Find the most recent profile
    LATEST=$(ls -t "$PROFILES_DIR"/profile_*.json 2>/dev/null | head -1)
    if [ -z "$LATEST" ]; then
        printf "\033[0;31m❌ No profiles found in $PROFILES_DIR\033[0m\n"
        exit 1
    fi

    printf "\033[0;32m✅ Latest profile: $LATEST\033[0m\n"
    echo ""
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    printf "\033[0;34m  📁 Profiles saved to: docs/profiles/\033[0m\n"
    printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
    echo ""
    ls -lh "$PROFILES_DIR"/profile_*.json "$PROFILES_DIR"/uffs.pdb "$PROFILES_DIR"/profile_summary_*.txt 2>/dev/null || true
    echo ""

    # Show latest summary if exists
    LATEST_SUMMARY=$(ls -t "$PROFILES_DIR"/profile_summary_*.txt 2>/dev/null | head -1)
    if [ -n "$LATEST_SUMMARY" ] && [ -f "$LATEST_SUMMARY" ]; then
        printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
        printf "\033[0;34m  📋 Latest Summary: $(basename "$LATEST_SUMMARY")\033[0m\n"
        printf "\033[0;34m━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\033[0m\n"
        cat "$LATEST_SUMMARY"
        echo ""
    fi

    # Check if PDB exists for symbolication
    PDB_PATH="$PROFILES_DIR/uffs.pdb"
    if [ -f "$PDB_PATH" ]; then
        printf "\033[0;32m✓ PDB file available for symbolication\033[0m\n"
    else
        printf "\033[0;33m⚠ No PDB file - uffs.exe functions will show as addresses\033[0m\n"
    fi

    printf "\033[0;34m   Opening latest profile in Firefox Profiler...\033[0m\n"
    printf "\033[0;34m   (Using Microsoft Symbol Server for Windows libraries)\033[0m\n"
    echo ""

    # Run samply load with symbol server for Windows libraries
    # The PDB in the same directory will be used for uffs.exe symbols
    cd "$PROFILES_DIR"
    samply load --windows-symbol-server "https://msdl.microsoft.com/download/symbols" "$(basename "$LATEST")"

# 🚨 CRITICAL FAILURE MODES TO AVOID:
# ─────────────────────────────────────────────────────────────────────────────
# • PowerShell syntax errors: Use bash shebangs on all complex recipes
# • Color variable expansion failures: Use direct ANSI codes with @printf
# • Tool installation loops: Use individual calls for cross-platform compatibility
# • Silent failures: Ensure bash -euo pipefail catches all errors
# • PATH issues: Provide clear guidance for each platform
# • Binary naming conflicts: Handle .exe extension on Windows
#
# 📚 EDUCATIONAL VALUE VERIFICATION:
# ─────────────────────────────────────────────────────────────────────────────
# • Every design decision has a documented WHY
# • Failed approaches are documented to prevent repetition
# • Cross-platform quirks are explained with examples
# • Tool selection rationale is clear
# • Workflow design philosophy is documented
# • Performance optimization strategies are explained
#
# This justfile serves as both a working build system AND an educational
# resource. Maintain both aspects when making changes.
