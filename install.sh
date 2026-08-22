#!/bin/sh
set -eu

REPOSITORY="benja/ask"
VERSION="${1:-latest}"

error() {
  printf 'ask: %s\n' "$*" >&2
  exit 1
}

require() {
  command -v "$1" >/dev/null 2>&1 || error "$1 is required"
}

download() {
  url="$1"
  destination="$2"

  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$destination"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$destination" "$url"
  else
    error "curl or wget is required"
  fi
}

detect_target() {
  case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-musl" ;;
    *) error "unsupported operating system: $(uname -s)" ;;
  esac

  case "$(uname -m)" in
    arm64 | aarch64) arch="aarch64" ;;
    x86_64 | amd64) arch="x86_64" ;;
    *) error "unsupported architecture: $(uname -m)" ;;
  esac

  printf '%s-%s\n' "$arch" "$os"
}

verify_checksum() {
  archive="$1"
  checksum_name="${archive##*/}.sha256"
  archive_dir="${archive%/*}"
  IFS=' ' read -r _ checked_name < "$archive.sha256" ||
    error "invalid checksum for ${archive##*/}"
  [ "$checked_name" = "${archive##*/}" ] ||
    [ "$checked_name" = "*${archive##*/}" ] ||
    error "invalid checksum for ${archive##*/}"

  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$archive_dir" && sha256sum -c "$checksum_name") >/dev/null 2>&1 ||
      error "checksum verification failed"
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$archive_dir" && shasum -a 256 -c "$checksum_name") >/dev/null 2>&1 ||
      error "checksum verification failed"
  else
    error "sha256sum or shasum is required"
  fi
}

valid_component() {
  case "$1" in
    "" | *[!0-9]* | 0[0-9]*) return 1 ;;
    *) return 0 ;;
  esac
}

valid_version() {
  value="$1"
  major="${value%%.*}"
  rest="${value#*.}"
  minor="${rest%%.*}"
  patch="${rest#*.}"
  [ "$rest" != "$value" ] &&
    [ "$patch" != "$rest" ] &&
    [ "$patch" = "${patch#*.}" ] &&
    valid_component "$major" &&
    valid_component "$minor" &&
    valid_component "$patch"
}

main() {
  if [ -n "${ASK_INSTALL_DIR:-}" ]; then
    install_dir="$ASK_INSTALL_DIR"
  else
    [ -n "${HOME:-}" ] || error "HOME is not set"
    install_dir="$HOME/.local/bin"
  fi
  [ -n "$install_dir" ] || error "ASK_INSTALL_DIR must not be empty"

  require uname
  require mktemp
  require tar

  case "$VERSION" in
    latest) tag="" ;;
    v*) version="${VERSION#v}" ;;
    *) version="$VERSION" ;;
  esac
  if [ "$VERSION" != latest ]; then
    valid_version "$version" || error "invalid version: $VERSION"
    tag="v$version"
  fi

  target="$(detect_target)"
  archive_name="ask-${target}.tar.gz"
  if [ -z "$tag" ]; then
    release_url="https://github.com/${REPOSITORY}/releases/latest/download"
  else
    release_url="https://github.com/${REPOSITORY}/releases/download/${tag}"
  fi

  temp_dir="$(mktemp -d)" || error "could not create a temporary directory"
  staged=""
  cleanup() {
    [ -z "$staged" ] || rm -f "$staged"
    rm -rf "$temp_dir"
  }
  trap cleanup 0
  trap 'exit 1' HUP INT TERM

  archive="$temp_dir/$archive_name"
  checksum="$archive.sha256"
  unpacked="$temp_dir/unpacked"

  printf 'installing ask...\n' >&2
  download "$release_url/$archive_name" "$archive"
  download "$release_url/$archive_name.sha256" "$checksum"
  verify_checksum "$archive"

  mkdir -p "$unpacked"
  tar -xzf "$archive" -C "$unpacked"
  [ -f "$unpacked/ask" ] || error "release archive does not contain ask"

  mkdir -p "$install_dir"
  [ ! -d "$install_dir/ask" ] || error "$install_dir/ask is a directory"
  staged="$install_dir/.ask-install.$$"
  cp "$unpacked/ask" "$staged"
  chmod 755 "$staged"

  installed_version="$("$staged" --version 2>/dev/null)" ||
    error "downloaded binary could not run on this system"
  valid_version "$installed_version" ||
    error "downloaded binary returned an unexpected version"
  if [ -n "$tag" ] && [ "$installed_version" != "${tag#v}" ]; then
    error "downloaded ask $installed_version, expected ask ${tag#v}"
  fi

  mv -f "$staged" "$install_dir/ask"
  staged=""
  printf 'installed ask %s to %s\n' "$installed_version" "$install_dir/ask" >&2

  case ":${PATH:-}:" in
    *":$install_dir:"*) ;;
    *)
      printf '\nAdd ask to your PATH:\n\n' >&2
      printf '  export PATH="%s:%s"\n\n' "$install_dir" "\$PATH" >&2
      printf 'Then add that line to your shell configuration.\n' >&2
      ;;
  esac

  if ! command -v codex >/dev/null 2>&1 &&
    ! command -v claude >/dev/null 2>&1 &&
    ! command -v pi >/dev/null 2>&1 &&
    ! command -v opencode >/dev/null 2>&1; then
    printf '\nNo supported coding agent was found on PATH.\n' >&2
    printf 'Install and authenticate Codex, Claude Code, Pi, or OpenCode.\n' >&2
  fi
}

main "$@"
