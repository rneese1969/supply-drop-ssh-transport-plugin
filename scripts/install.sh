#!/usr/bin/env sh
set -eu

REPO="${REPO:-rneese1969/supply-drop-ssh-transport-plugin}"
VERSION="${VERSION:-latest}"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
BIN_NAME="${BIN_NAME:-supply-drop-ssh}"
SOURCE_BIN="supply-drop-ssh"
BBS_BIN="${BBS_BIN:-supply-drop-bbs}"
REGISTER_PLUGIN="${REGISTER_PLUGIN:-true}"
MIN_BBS_VERSION="0.6.0"

info() {
  printf '%s\n' "$*"
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

detect_asset() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux)
      case "$arch" in
        x86_64 | amd64) printf 'linux-x86_64' ;;
        aarch64 | arm64) printf 'linux-arm64' ;;
		armv71 | armhf) printf 'linux-armhf' ;;
        *) fail "unsupported Linux architecture: $arch" ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64 | amd64) printf 'macos-x86_64' ;;
        aarch64 | arm64) printf 'macos-arm64' ;;
        *) fail "unsupported macOS architecture: $arch" ;;
      esac
      ;;
    *)
      fail "unsupported operating system: $os"
      ;;
  esac
}

detect_deb_arch() {
  command -v dpkg >/dev/null 2>&1 || return 1
  case "$(dpkg --print-architecture)" in
    amd64) printf 'amd64' ;;
    arm64) printf 'arm64' ;;
	armhf) printf 'armhf' ;;
    *) return 1 ;;
  esac
}

download() {
  url="$1"
  dest="$2"

  if command -v curl >/dev/null 2>&1; then
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      curl -fsSL -H "Authorization: Bearer ${GITHUB_TOKEN}" "$url" -o "$dest"
    else
      curl -fsSL "$url" -o "$dest"
    fi
  elif command -v wget >/dev/null 2>&1; then
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      wget -q --header="Authorization: Bearer ${GITHUB_TOKEN}" "$url" -O "$dest"
    else
      wget -q "$url" -O "$dest"
    fi
  else
    fail "missing required command: curl or wget"
  fi
}

run_install() {
  src="$1"
  dest="$2"

  mkdir -p "$INSTALL_DIR" 2>/dev/null || true

  if [ -w "$INSTALL_DIR" ]; then
    install -m 755 "$src" "$dest"
  elif command -v sudo >/dev/null 2>&1; then
    sudo mkdir -p "$INSTALL_DIR"
    sudo install -m 755 "$src" "$dest"
  else
    fail "$INSTALL_DIR is not writable and sudo is not available"
  fi
}

run_dpkg_install() {
  deb="$1"

  if [ "$(id -u)" -eq 0 ]; then
    dpkg -i "$deb"
  elif command -v sudo >/dev/null 2>&1; then
    sudo dpkg -i "$deb"
  else
    fail "dpkg install requires root and sudo is not available"
  fi
}

version_ge() {
  current="$1"
  minimum="$2"

  old_ifs="$IFS"
  IFS=.
  set -- $current
  current_major="${1:-0}"
  current_minor="${2:-0}"
  current_patch="${3:-0}"
  set -- $minimum
  minimum_major="${1:-0}"
  minimum_minor="${2:-0}"
  minimum_patch="${3:-0}"
  IFS="$old_ifs"

  [ "$current_major" -gt "$minimum_major" ] && return 0
  [ "$current_major" -lt "$minimum_major" ] && return 1
  [ "$current_minor" -gt "$minimum_minor" ] && return 0
  [ "$current_minor" -lt "$minimum_minor" ] && return 1
  [ "$current_patch" -ge "$minimum_patch" ]
}

detect_bbs_version() {
  "$BBS_BIN" --version 2>/dev/null |
    awk 'match($0, /[0-9]+\.[0-9]+\.[0-9]+/) { print substr($0, RSTART, RLENGTH); exit }'
}

run_bbs_plugin_add() {
  dest="$1"

  [ "$REGISTER_PLUGIN" = "true" ] || {
    info "Skipping Supply Drop plugin registration because REGISTER_PLUGIN=$REGISTER_PLUGIN"
    return
  }

  command -v "$BBS_BIN" >/dev/null 2>&1 ||
    fail "missing $BBS_BIN. Install or upgrade Supply Drop BBS v${MIN_BBS_VERSION}+ before registering the plugin, or rerun with REGISTER_PLUGIN=false to install only the binary."

  bbs_version="$(detect_bbs_version)"
  [ -n "$bbs_version" ] || fail "could not detect Supply Drop BBS version from '$BBS_BIN --version'"
  version_ge "$bbs_version" "$MIN_BBS_VERSION" ||
    fail "Supply Drop BBS v${MIN_BBS_VERSION}+ is required for plugins.d registration; found v${bbs_version}"

  info "Registering SSH process plugin with Supply Drop BBS v${bbs_version}..."
  if "$BBS_BIN" plugin add ssh "$dest"; then
    return
  fi

  if command -v sudo >/dev/null 2>&1; then
    info "Retrying plugin registration with sudo..."
    sudo "$BBS_BIN" plugin add ssh "$dest"
  else
    fail "plugin registration failed and sudo is not available"
  fi
}

need_cmd uname
need_cmd mktemp
need_cmd awk

asset="$(detect_asset)"
archive="supply-drop-ssh-transport-plugin-${asset}.zip"
deb_arch="$(detect_deb_arch || true)"
deb=""

if [ -n "$deb_arch" ] && [ "${INSTALL_FROM_DEB:-true}" = "true" ]; then
  deb="supply-drop-ssh-transport-plugin_${deb_arch}.deb"
fi

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/${REPO}/releases/latest/download/${archive}"
  deb_url="https://github.com/${REPO}/releases/latest/download/${deb}"
  tag=""
else
  case "$VERSION" in
    v*) tag="$VERSION" ;;
    *) tag="v$VERSION" ;;
  esac
  url="https://github.com/${REPO}/releases/download/${tag}/${archive}"
  deb_url="https://github.com/${REPO}/releases/download/${tag}/${deb}"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

if [ -n "$deb" ]; then
  info "Downloading ${deb} from ${VERSION}..."
  if download "$deb_url" "$tmp/plugin.deb"; then
    run_dpkg_install "$tmp/plugin.deb"
    info "Installed $BIN_NAME from Debian package."
    exit 0
  fi

  info "Debian package is not available for ${VERSION}; falling back to ZIP install."
fi

info "Downloading ${archive} from ${VERSION}..."
need_cmd unzip
need_cmd install
if ! download "$url" "$tmp/plugin.zip"; then
  if command -v gh >/dev/null 2>&1; then
    if [ -z "$tag" ]; then
      tag="$(gh release view --repo "$REPO" --json tagName -q .tagName)"
    fi
    gh release download "$tag" --repo "$REPO" --pattern "$archive" --dir "$tmp"
    mv "$tmp/$archive" "$tmp/plugin.zip"
  else
    fail "download failed. If this repository is private, install GitHub CLI and run gh auth login"
  fi
fi

unzip -q "$tmp/plugin.zip" -d "$tmp/unpacked"
binary="$(find "$tmp/unpacked" -type f -name "$SOURCE_BIN" | head -n 1)"
[ -n "$binary" ] || fail "archive did not contain $SOURCE_BIN"

dest="${INSTALL_DIR%/}/$BIN_NAME"
run_install "$binary" "$dest"
run_bbs_plugin_add "$dest"

info "Installed $BIN_NAME to $dest"
info ""
info "SSH plugin registration complete. Restart Supply Drop BBS to activate it:"
info "  sudo systemctl restart supply-drop-bbs"
info ""
info "The plugin listens on port 2222 and generates a persistent SSH host key"
info "on first start. Connect with:"
info "  ssh -p 2222 <bbs-host>"
