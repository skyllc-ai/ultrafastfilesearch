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

# Add LLVM to the PATH for the current session
# This ensures that the llvm binaries like clang and lld are found
export PATH="/usr/local/opt/llvm/bin:$PATH"

# Set the library flags for linking
# This tells the compiler where to find the llvm libraries
export LDFLAGS="-L/usr/local/opt/llvm/lib"

# Set the compiler flags for preprocessor includes
# This tells the compiler where to find the llvm header files
export CPPFLAGS="-I/usr/local/opt/llvm/include"

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

export PATH=$PATH:~/bin
# export PATH=$PATH:~/bin/rust/debug
export PATH=$PATH:~/bin/rust/release
export PATH=$PATH:$HOME/.appfabric-bin
export PATH=$PATH:/opt/homebrew/Cellar/zookeeper/3.9.1/libexec/bin

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

#THIS MUST BE AT THE END OF THE FILE FOR SDKMAN TO WORK!!!
export SDKMAN_DIR="$HOME/.sdkman"
[[ -s "$HOME/.sdkman/bin/sdkman-init.sh" ]] && source "$HOME/.sdkman/bin/sdkman-init.sh"

export RUST_LOG=error,tt=info

export RUSTC_WRAPPER=$(which sccache)

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
export PATH="/Users/rnio/.codeium/windsurf/bin:$PATH"
alias mbuild="MAVEN_OPTS=\"--add-opens java.base/java.lang=ALL-UNNAMED -Xmx4g -XX:+UseParallelGC\" mvnd -T 1.5C"
alias m15="MAVEN_OPTS=\"--add-opens java.base/java.lang=ALL-UNNAMED -Xmx4g -XX:+UseParallelGC\" mvn -T 1.5C"

export APP_NAME=ETS_COMPONENT_TEST
export DB_REGION=LOCAL

# TTAPI Secrets: do NOT store secrets in repo or shell rc files.
# Use per-user secure storage (1Password CLI or OS keychain). See docs.

export TT_ENVIRONMENT="production"  # "sandbox" or "production" 

export RUST_LOG="off"            # Terminal logging level
export RUST_LOG_FILE="trace"     # File logging level
eval "$(/opt/homebrew/bin/brew shellenv)"
export RUSTC_WRAPPER=sccache

# Created by `pipx` on 2025-07-17 23:43:14
export PATH="$PATH:/Users/rnio/.local/bin"
