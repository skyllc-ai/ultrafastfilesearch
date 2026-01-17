# Robust repo-local Bash profile
# - Guards missing tools (nvm, sdkman, brew)
# - Idempotent PATH management
# - Keeps noise low in non-interactive shells

# --- NVM (guarded) -----------------------------------------------------------
if [ -s "$HOME/.nvm/nvm.sh" ]; then
  . "$HOME/.nvm/nvm.sh"
fi

# --- PATH helper -------------------------------------------------------------
path_prepend() {
  case ":$PATH:" in
    *":$1:"*) ;; # already present
    *) PATH="$1:$PATH" ;;
  esac
}

# AppFabric helpers (if present)
[ -d "$HOME/.appfabric-bin" ] && path_prepend "$HOME/.appfabric-bin"
if command -v appf-webtools-mgr >/dev/null 2>&1; then
  alias atm=appf-webtools-mgr
fi

# --- SDKMAN (guarded) --------------------------------------------------------
export SDKMAN_DIR="$HOME/.sdkman"
[ -s "$SDKMAN_DIR/bin/sdkman-init.sh" ] && . "$SDKMAN_DIR/bin/sdkman-init.sh"

# --- Homebrew shellenv (Apple/Intel, guarded) --------------------------------
if [ -x /opt/homebrew/bin/brew ]; then
  eval "$(/opt/homebrew/bin/brew shellenv)"
elif [ -x /usr/local/bin/brew ]; then
  eval "$(/usr/local/bin/brew shellenv)"
fi


# --- Prefer JDK 11 if available --------------------------------------------
if command -v /usr/libexec/java_home >/dev/null 2>&1; then
  export JAVA_HOME="$(
    /usr/libexec/java_home -v 11 2>/dev/null || /usr/libexec/java_home 2>/dev/null
  )"
  path_prepend "$JAVA_HOME/bin"
fi
