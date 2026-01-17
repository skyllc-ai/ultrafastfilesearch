# Suppress instant prompt warnings
typeset -g POWERLEVEL9K_INSTANT_PROMPT=quiet

# Enable Powerlevel10k instant prompt. Should stay close to the top of ~/.zshrc.
# Initialization code that may require console input (password prompts, [y/n]
# confirmations, etc.) must go above this block; everything else may go below.
if [[ -r "${XDG_CACHE_HOME:-$HOME/.cache}/p10k-instant-prompt-${(%):-%n}.zsh" ]]; then
  source "${XDG_CACHE_HOME:-$HOME/.cache}/p10k-instant-prompt-${(%):-%n}.zsh"
fi

export LC_ALL=en_US.UTF-8
export LANG=en_US.UTF-8


# direnv hook (quiet) for interactive shells
if command -v direnv >/dev/null 2>&1; then
  export DIRENV_LOG_FORMAT=""
  eval "$(direnv hook zsh)"
fi

# PATH helpers and hygiene
# - Deduplicate automatically (unique elements) while preserving order
# - Append/prepend only if the directory exists
# Additional zsh options (safe defaults)
setopt HIST_VERIFY          # Edit recalled history line before executing
setopt NO_LIST_BEEP         # No beep when completing with multiple matches
setopt INTERACTIVE_COMMENTS # Allow comments in interactive commands
setopt NO_FLOW_CONTROL      # Disable XON/XOFF (prevents accidental ^S freeze)

typeset -gU path fpath
path_append_if_exists() { [ -d "$1" ] && path+=("$1"); }
path_prepend_if_exists() { [ -d "$1" ] && path=("$1" $path); }


# SSH agent auto-discovery/repair (macOS-friendly)
ssh_agent_setup() {
  # If an agent socket is already valid, do nothing
  if [ -n "$SSH_AUTH_SOCK" ] && [ -S "$SSH_AUTH_SOCK" ]; then
    return 0
  fi

  # Prefer 1Password SSH agent if available
  local op_sock="$HOME/.1password/agent.sock"
  if [ -S "$op_sock" ]; then
    export SSH_AUTH_SOCK="$op_sock"
    return 0
  fi

  # Try to load a previously started user agent
  if [ -r "$HOME/.ssh/agent.env" ]; then
    . "$HOME/.ssh/agent.env" >/dev/null 2>&1
    if [ -n "$SSH_AUTH_SOCK" ] && [ -S "$SSH_AUTH_SOCK" ]; then
      return 0
    fi
  fi

  # Start a new user agent (persist env for future shells)
  mkdir -p "$HOME/.ssh"
  ssh-agent -s > "$HOME/.ssh/agent.env"
  . "$HOME/.ssh/agent.env" >/dev/null 2>&1

  # Optionally load keys from macOS Keychain / default identities (quiet)
  if command -v ssh-add >/dev/null 2>&1; then
    ssh-add -A >/dev/null 2>&1 || true
  fi
}

ssh_agent_kill() {
  if [ -r "$HOME/.ssh/agent.env" ]; then
    . "$HOME/.ssh/agent.env" >/dev/null 2>&1
  fi
  command -v ssh-agent >/dev/null 2>&1 && ssh-agent -k >/dev/null 2>&1
  rm -f "$HOME/.ssh/agent.env"
  unset SSH_AUTH_SOCK SSH_AGENT_PID
}

# Initialize/repair SSH agent automatically (quiet)
ssh_agent_setup

# Handy aliases
alias ssh-fix='ssh_agent_setup'
alias ssh-agent-restart='ssh_agent_kill && ssh_agent_setup'


HISTFILE=~/.zsh_history
HISTSIZE=50000
SAVEHIST=50000

# zsh options: http://zsh.sourceforge.net/Doc/Release/Options.html
setopt APPEND_HISTORY # adds history
setopt HIST_IGNORE_ALL_DUPS # If a new command line being added to the history list duplicates an older one, the older command is removed from the list
setopt HIST_IGNORE_SPACE # No history when starting command with space
setopt HIST_SAVE_NO_DUPS # When writing out the history file, older commands that duplicate newer ones are omitted

# If you come from bash you might have to change your $PATH.
# export PATH=$HOME/bin:$HOME/.local/bin:/usr/local/bin:$PATH

# Add LLVM toolchain (portable via Homebrew prefix if available)
if command -v brew >/dev/null 2>&1; then
  __llvm_prefix="$(brew --prefix llvm 2>/dev/null)"
  if [ -n "$__llvm_prefix" ] && [ -d "$__llvm_prefix" ]; then
    export PATH="$__llvm_prefix/bin:$PATH"
    export LDFLAGS="-L$__llvm_prefix/lib${LDFLAGS:+:$LDFLAGS}"
    export CPPFLAGS="-I$__llvm_prefix/include${CPPFLAGS:+:$CPPFLAGS}"
  fi
fi

# Path to your Oh My Zsh installation.
export ZSH="$HOME/.oh-my-zsh"

# Set name of the theme to load --- if set to "random", it will
# load a random theme each time Oh My Zsh is loaded, in which case,
# to know which specific one was loaded, run: echo $RANDOM_THEME
# See https://github.com/ohmyzsh/ohmyzsh/wiki/Themes
# ZSH_THEME="robbyrussell"
ZSH_THEME=""

# Set list of themes to pick from when loading at random
# Setting this variable when ZSH_THEME=random will cause zsh to load
# a theme from this variable instead of looking in $ZSH/themes/
# If set to an empty array, this variable will have no effect.
# ZSH_THEME_RANDOM_CANDIDATES=( "robbyrussell" "agnoster" )

# Uncomment the following line to use case-sensitive completion.
# CASE_SENSITIVE="true"

# Uncomment the following line to use hyphen-insensitive completion.
# Case-sensitive completion must be off. _ and - will be interchangeable.
# HYPHEN_INSENSITIVE="true"

zstyle ':omz:update' mode auto

# Uncomment the following line to change how often to auto-update (in days).
zstyle ':omz:update' frequency 30

# Uncomment the following line if pasting URLs and other text is messed up.
# DISABLE_MAGIC_FUNCTIONS="true"

# Uncomment the following line to disable colors in ls.
# DISABLE_LS_COLORS="true"

# Uncomment the following line to disable auto-setting terminal title.
# DISABLE_AUTO_TITLE="true"

# Uncomment the following line to enable command auto-correction.
# ENABLE_CORRECTION="true"

# Uncomment the following line to display red dots whilst waiting for completion.
# You can also set it to another string to have that shown instead of the default red dots.
# e.g. COMPLETION_WAITING_DOTS="%F{yellow}waiting...%f"
# Caution: this setting can cause issues with multiline prompts in zsh < 5.7.1 (see #5765)
# COMPLETION_WAITING_DOTS="true"

# Uncomment the following line if you want to disable marking untracked files
# under VCS as dirty. This makes repository status check for large repositories
# much, much faster.
# DISABLE_UNTRACKED_FILES_DIRTY="true"

# Uncomment the following line if you want to change the command execution time
# stamp shown in the history command output.
# You can set one of the optional three formats:
# "mm/dd/yyyy"|"dd.mm.yyyy"|"yyyy-mm-dd"
# or set a custom format using the strftime function format specifications,
# see 'man strftime' for details.
# HIST_STAMPS="mm/dd/yyyy"

# Would you like to use another custom folder than $ZSH/custom?
# ZSH_CUSTOM=/path/to/new-custom-folder

# Which plugins would you like to load?
# Standard plugins can be found in $ZSH/plugins/
# Custom plugins may be added to $ZSH_CUSTOM/plugins/
# Example format: plugins=(rails git textmate ruby lighthouse)
# Add wisely, as too many plugins slow down shell startup.
# plugins=(git zsh-autosuggestions)

# load plugins
[ -f ~/.zplug.sh ] && source ~/.zplug.sh

source $ZSH/oh-my-zsh.sh

# User configuration

# export MANPATH="/usr/local/man:$MANPATH"

# You may need to manually set your language environment
# export LANG=en_US.UTF-8

# Preferred editor for local and remote sessions
# if [[ -n $SSH_CONNECTION ]]; then
#   export EDITOR='vim'
# else
#   export EDITOR='mvim'
# fi

# Compilation flags
# export ARCHFLAGS="-arch x86_64"

# Set personal aliases, overriding those provided by oh-my-zsh libs,
# plugins, and themes. Aliases can be placed here, though oh-my-zsh
# users are encouraged to define aliases within the ZSH_CUSTOM folder.
# For a full list of active aliases, run `alias`.
#
# Example aliases
# alias zshconfig="mate ~/.zshrc"
# alias ohmyzsh="mate ~/.oh-my-zsh"

# key bindings
bindkey '[C' forward-word   # alt+left
bindkey '[D' backward-word  # alt+right

path_append_if_exists "$HOME/bin"
# path_append_if_exists "$HOME/bin/rust/debug"
path_append_if_exists "$HOME/bin/rust/release"
path_append_if_exists "$HOME/.appfabric-bin"
path_append_if_exists "/opt/homebrew/Cellar/zookeeper/3.9.1/libexec/bin"

export JAVA_HOME="$(sdk home java 11.0.25-amzn)"

gg_func() {
    # Capture the output of the go_to Rust program
    local repo_path=$(go_to cd "$@")

    echo -e "\nWir gehen zu: $repo_path\n"
    echo cd $repo_path

    cd "$repo_path"

}

local_func() {
    go_to local "$@"
}

alias g='gg_func'
alias l="local_func"

alias awm=appf-webtools-mgr
alias atm=appf-webtools-mgr

export JMETER_HOME=/opt/homebrew/Cellar/jmeter/5.6.3/libexec
export TESTCONTAINERS_ENABLED=false
export DOCKER_HOST="$HOME/.local/share/containers/podman/machine/applehv/podman.sock"
export NODE_EXTRA_CA_CERTS=~/Documents/caadmin.netskope.com.pem

##########
# podman #
##########
unset SSH_AUTH_SOCK
[[ ! -v SSH_AUTH_SOCK ]] && echo "SSH_AUTH_SOCK is not set" # export NEXUS_PROXY_URL=https://nexus.intuit.com/nexus
# export NEXUS_PROXY_URL=https://nexus.intuit.com/nexus
export NEXUS_PROXY_URL=https://artifact.intuit.com/artifactory/maven-proxy

# To customize prompt, run `p10k configure` or edit ~/.p10k.zsh.
[[ ! -f ~/.p10k.zsh ]] || source ~/.p10k.zsh

test -e "${HOME}/.iterm2_shell_integration.zsh" && source "${HOME}/.iterm2_shell_integration.zsh"
source /opt/homebrew/share/powerlevel10k/powerlevel10k.zsh-theme


# ttapi zsh helpers (repo or system-wide)
# - use_ttapi_env: set TTAPI_TARGET_ENV and load from direnv if .envrc present, else from ~/.config/ttapi/.env.<env>
# - ttapi_unset: clear TT_*; if in a direnv-enabled repo, reload; otherwise just unset
# - aliases: quick toggles
__ttapi_is_direnv_active() { command -v direnv >/dev/null 2>&1 && [ -f .envrc ]; }
__ttapi_config_home() {
  if [ -n "$TTAPI_CONFIG_HOME" ] && [ -d "$TTAPI_CONFIG_HOME" ]; then
    printf '%s' "$TTAPI_CONFIG_HOME"
  else
    local base="${XDG_CONFIG_HOME:-$HOME/.config}"; printf '%s' "$base/ttapi"
  fi
}
__ttapi_env_path() { local env="$1"; local cfg="$(__ttapi_config_home)"; local f="$cfg/.env.$env"; [ -f "$f" ] && printf '%s' "$f"; }
__ttapi_load_env_file() { [ -f "$1" ] || return 1; set -a; . "$1"; set +a; [ -z "$TT_ENVIRONMENT" ] && export TT_ENVIRONMENT="${TTAPI_TARGET_ENV:-sandbox}"; }

ttapi_unset() {
  unset TT_ENVIRONMENT TT_OAUTH2_CLIENT_ID TT_OAUTH2_CLIENT_SECRET TT_OAUTH2_REFRESH_TOKEN TT_OAUTH2_CLIENT_ID_ENC TT_OAUTH2_CLIENT_SECRET_ENC TT_OAUTH2_REFRESH_TOKEN_ENC
  if __ttapi_is_direnv_active; then direnv reload >/dev/null 2>&1 || true; fi
}

use_ttapi_env() {
  case "$1" in
    sandbox|production)
      export TTAPI_TARGET_ENV="$1"
      if __ttapi_is_direnv_active; then
        direnv reload >/dev/null 2>&1 || true
      else
        ttapi_unset
        local p; p="$(__ttapi_env_path "$1")"
        if [ -n "$p" ]; then __ttapi_load_env_file "$p"; else echo "ttapi: profile not found: $(__ttapi_config_home)/.env.$1" >&2; fi
      fi
      ;;
    *) echo "usage: use_ttapi_env [sandbox|production]" >&2; return 1 ;;
  esac
}

alias tt-sbx='use_ttapi_env sandbox'
alias tt-prod='use_ttapi_env production'
alias tt-unset='ttapi_unset'


# 1Password CLI wrappers for process-scoped secrets injection
# Configure these (optional) to match your vault structure; defaults are sensible guesses
#   export TTAPI_OP_SANDBOX_CLIENT_ID="op://TTAPI/Sandbox/client_id"
#   export TTAPI_OP_SANDBOX_CLIENT_SECRET="op://TTAPI/Sandbox/client_secret"
#   export TTAPI_OP_SANDBOX_REFRESH_TOKEN="op://TTAPI/Sandbox/refresh_token"
#   export TTAPI_OP_PRODUCTION_CLIENT_ID="op://TTAPI/Production/client_id"
#   export TTAPI_OP_PRODUCTION_CLIENT_SECRET="op://TTAPI/Production/client_secret"
#   export TTAPI_OP_PRODUCTION_REFRESH_TOKEN="op://TTAPI/Production/refresh_token"
__ttapi_op_available() { command -v op >/dev/null 2>&1; }
__ttapi_op_signed_in() { op whoami >/dev/null 2>&1; }
__ttapi_op_config() {
  local env="$1"; local cid csec rtok
  case "$env" in
    sandbox)
      cid="${TTAPI_OP_SANDBOX_CLIENT_ID:-op://TTAPI/Sandbox/client_id}"
      csec="${TTAPI_OP_SANDBOX_CLIENT_SECRET:-op://TTAPI/Sandbox/client_secret}"
      rtok="${TTAPI_OP_SANDBOX_REFRESH_TOKEN:-op://TTAPI/Sandbox/refresh_token}"
      ;;
    production)
      cid="${TTAPI_OP_PRODUCTION_CLIENT_ID:-op://TTAPI/Production/client_id}"
      csec="${TTAPI_OP_PRODUCTION_CLIENT_SECRET:-op://TTAPI/Production/client_secret}"
      rtok="${TTAPI_OP_PRODUCTION_REFRESH_TOKEN:-op://TTAPI/Production/refresh_token}"
      ;;
    *) echo "ttapi: unknown env '$env'" >&2; return 1 ;;
  esac
  printf '%s|%s|%s' "$cid" "$csec" "$rtok"
}
__ttapi_op_run() {
  local env="$1"; shift || true
  if ! __ttapi_op_available; then echo "ttapi: 1Password CLI 'op' not found" >&2; return 1; fi
  if ! __ttapi_op_signed_in; then echo "ttapi: please sign in to 1Password CLI (run: op signin)" >&2; return 1; fi
  local triple; triple="$(__ttapi_op_config "$env")" || return 1
  local cid csec rtok
  IFS='|' read -r cid csec rtok <<EOF
$triple
EOF
  TT_ENVIRONMENT="$env" op run \
    --env "TT_OAUTH2_CLIENT_ID=$cid" \
    --env "TT_OAUTH2_CLIENT_SECRET=$csec" \
    --env "TT_OAUTH2_REFRESH_TOKEN=$rtok" \
    -- "$@"
}
# Usage: tt-sbx-run <command ...>
#        tt-prod-run <command ...>
# Example: tt-sbx-run cargo run -q -p ttapi-cli -- stream start --symbols AAPL,MSFT --types quotes
alias tt-sbx-run='__ttapi_op_run sandbox'
alias tt-prod-run='__ttapi_op_run production'


# gopass wrappers for process-scoped secrets injection (repo-backed, free)
# Override these to match your store structure if different
#   export TTAPI_GOPASS_SANDBOX_CLIENT_ID="TTAPI/Sandbox/client_id"
#   export TTAPI_GOPASS_SANDBOX_CLIENT_SECRET="TTAPI/Sandbox/client_secret"
#   export TTAPI_GOPASS_SANDBOX_REFRESH_TOKEN="TTAPI/Sandbox/refresh_token"
#   export TTAPI_GOPASS_PRODUCTION_CLIENT_ID="TTAPI/Production/client_id"
#   export TTAPI_GOPASS_PRODUCTION_CLIENT_SECRET="TTAPI/Production/client_secret"
#   export TTAPI_GOPASS_PRODUCTION_REFRESH_TOKEN="TTAPI/Production/refresh_token"
__ttapi_gopass_available() { command -v gopass >/dev/null 2>&1; }
__ttapi_gopass_paths() {
  local env="$1"; local cid csec rtok
  case "$env" in
    sandbox)
      cid="${TTAPI_GOPASS_SANDBOX_CLIENT_ID:-TTAPI/Sandbox/client_id}"
      csec="${TTAPI_GOPASS_SANDBOX_CLIENT_SECRET:-TTAPI/Sandbox/client_secret}"
      rtok="${TTAPI_GOPASS_SANDBOX_REFRESH_TOKEN:-TTAPI/Sandbox/refresh_token}"
      ;;
    production)
      cid="${TTAPI_GOPASS_PRODUCTION_CLIENT_ID:-TTAPI/Production/client_id}"
      csec="${TTAPI_GOPASS_PRODUCTION_CLIENT_SECRET:-TTAPI/Production/client_secret}"
      rtok="${TTAPI_GOPASS_PRODUCTION_REFRESH_TOKEN:-TTAPI/Production/refresh_token}"
      ;;
    *) echo "ttapi: unknown env '$env'" >&2; return 1 ;;
  esac
  printf '%s|%s|%s' "$cid" "$csec" "$rtok"
}
__ttapi_gopass_run() {
  local env="$1"; shift || true
  if ! __ttapi_gopass_available; then echo "ttapi: 'gopass' CLI not found" >&2; return 1; fi
  local triple; triple="$(__ttapi_gopass_paths "$env")" || return 1
  local cid_path csec_path rtok_path
  IFS='|' read -r cid_path csec_path rtok_path <<EOF
$triple
EOF
  local cid csec rtok
  cid="$(gopass show -o "$cid_path")" || { echo "ttapi: missing $cid_path" >&2; return 1; }
  csec="$(gopass show -o "$csec_path")" || { echo "ttapi: missing $csec_path" >&2; return 1; }
  rtok="$(gopass show -o "$rtok_path")" || { echo "ttapi: missing $rtok_path" >&2; return 1; }
  TT_ENVIRONMENT="$env" \
  TT_OAUTH2_CLIENT_ID="$cid" \
  TT_OAUTH2_CLIENT_SECRET="$csec" \
  TT_OAUTH2_REFRESH_TOKEN="$rtok" \
    "$@"
}
# Usage: tt-sbx-gopass-run <command ...>
#        tt-prod-gopass-run <command ...>
# Example: tt-sbx-gopass-run cargo run -q -p ttapi-cli -- stream start --symbols AAPL,MSFT --types quotes
alias tt-sbx-gopass-run='__ttapi_gopass_run sandbox'
alias tt-prod-gopass-run='__ttapi_gopass_run production'


# Profile-specific gopass wrappers (requested): tt_sandbox, tt_robbi, tt_sky
# - Uses explicit gopass paths for each profile
# - Sets TT_ENVIRONMENT accordingly (sandbox for Sandbox; production for Robert/Sky)
__ttapi_gopass_custom_run() {
  local env="$1"; local base="$2"; shift 2 || true
  if ! __ttapi_gopass_available; then echo "ttapi: 'gopass' CLI not found" >&2; return 1; fi
  local cid csec rtok
  cid="$(gopass show -o "$base/client_id")"       || { echo "ttapi: missing $base/client_id" >&2; return 1; }
  csec="$(gopass show -o "$base/client_secret")"  || { echo "ttapi: missing $base/client_secret" >&2; return 1; }
  rtok="$(gopass show -o "$base/refresh_token")"  || { echo "ttapi: missing $base/refresh_token" >&2; return 1; }
  TT_ENVIRONMENT="$env" \
  TT_OAUTH2_CLIENT_ID="$cid" \
  TT_OAUTH2_CLIENT_SECRET="$csec" \
  TT_OAUTH2_REFRESH_TOKEN="$rtok" \
    "$@"
}
# New simple wrappers without '-gopass-run'
# Usage: tt_sandbox <cmd...>
#        tt_robbi   <cmd...>
#        tt_sky     <cmd...>
# Notes: Robert and Sky are treated as Production environment by default
alias tt_sandbox='__ttapi_gopass_custom_run sandbox TTAPI/Sandbox'
alias tt_robbi='__ttapi_gopass_custom_run production TTAPI/Robert'
alias tt_sky='__ttapi_gopass_custom_run production TTAPI/Sky'


# Rust defaults (honor existing)
export RUST_LOG="${RUST_LOG:-error,tt=info}"
if command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER="$(command -v sccache)"
fi

# Maven commands
alias spotless='mvn spotless:apply'

# Spend Management commands
alias spend_cd='cd /Users/rnio/GitHub/spend-mgmt'
alias spend_eiam='eiamCli login; eiamCli getAWSTempCredentials -a 432647000723 -r PowerUser -p profile_1234'
alias spend_db_init='podman play kube podman-postgre-pod.yaml'
alias spend_db_start='podman machine start; podman start postgre-sql-spend-mgmt-db'
alias spend_db_stop='podman stop postgre-sql-spend-mgmt-db'
alias spend_run='spend_cd; mvn clean spring-boot:run -s settings.xml -f app/core/pom.xml'
alias spend_run_initial='spend_cd; mvn -s settings.xml clean install; mvn spring-boot:run -s settings.xml -f app/core/pom.xml'
alias spend_spotless='spend_cd; cd app; spotless'
alias spend_health='curl -k https://localhost:8443/health/full'

# Added by Windsurf
path_prepend_if_exists "$HOME/.codeium/windsurf/bin"
alias mbuild="MAVEN_OPTS=\"--add-opens java.base/java.lang=ALL-UNNAMED -Xmx4g -XX:+UseParallelGC\" mvnd -T 1.5C"
alias m15="MAVEN_OPTS=\"--add-opens java.base/java.lang=ALL-UNNAMED -Xmx4g -XX:+UseParallelGC\" mvn -T 1.5C"

export APP_NAME=ETS_COMPONENT_TEST
export DB_REGION=LOCAL

# TTAPI keys and credentials
# Avoid storing secrets in global shell rc files. Prefer per-repo .env files (with direnv)
# or process-scoped injection from OS keychain / 1Password CLI.
# See docs for cross-platform secrets guidance.
# export RUST_LOG="off"                # Prefer per-session in env files
# export RUST_LOG_FILE="trace"
# eval "$(/opt/homebrew/bin/brew shellenv)"  # Already configured in .zprofile
# export RUSTC_WRAPPER=sccache            # Already set above or per-project

# Created by `pipx` on 2025-07-17 23:43:14
path_append_if_exists "$HOME/.local/bin"


#THIS MUST BE AT THE END OF THE FILE FOR SDKMAN TO WORK!!!
export SDKMAN_DIR="$HOME/.sdkman"
[[ -s "$HOME/.sdkman/bin/sdkman-init.sh" ]] && source "$HOME/.sdkman/bin/sdkman-init.sh"
