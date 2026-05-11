# Sourced by pi before each bash command in this project.
# Blocks broad formatter commands from pi agents unless explicitly overridden.

cargo() {
  if [[ "${ALLOW_AGENT_FORMAT:-}" != "1" && "${STORES_ALLOW_FORMAT_CHURN:-}" != "1" ]]; then
    local first="${1:-}"
    local second="${2:-}"
    if [[ "$first" == +* && "$second" == "fmt" ]]; then
      for arg in "$@"; do [[ "$arg" == "--check" ]] && command cargo "$@" && return $?; done
      echo "BLOCKED: cargo fmt is disabled for agents in stores. Use ALLOW_AGENT_FORMAT=1 only for intentional scoped formatting." >&2
      return 2
    fi
    if [[ "$first" == "fmt" ]]; then
      for arg in "$@"; do [[ "$arg" == "--check" ]] && command cargo "$@" && return $?; done
      echo "BLOCKED: cargo fmt is disabled for agents in stores. Use ALLOW_AGENT_FORMAT=1 only for intentional scoped formatting." >&2
      return 2
    fi
  fi
  command cargo "$@"
}

rustfmt() {
  if [[ "${ALLOW_AGENT_FORMAT:-}" != "1" && "${STORES_ALLOW_FORMAT_CHURN:-}" != "1" ]]; then
    for arg in "$@"; do [[ "$arg" == "--check" ]] && command rustfmt "$@" && return $?; done
    echo "BLOCKED: rustfmt is disabled for agents in stores. Use ALLOW_AGENT_FORMAT=1 only for intentional scoped formatting." >&2
    return 2
  fi
  command rustfmt "$@"
}
