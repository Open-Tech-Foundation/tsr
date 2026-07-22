#!/usr/bin/env bash
#
# tsr installer for Linux, macOS, and FreeBSD.
#
#   curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/tsr/main/install.sh | bash
#
# Downloads the latest released `tsr` binary for your platform, verifies its
# SHA-256 checksum when the release ships one, and installs it to ~/.tsr/bin.
# Override the version with TSR_VERSION=v0.1.0 and the install dir with
# TSR_INSTALL=/custom/path.
set -euo pipefail

REPO="Open-Tech-Foundation/tsr"
INSTALL_DIR="${TSR_INSTALL:-$HOME/.tsr}"
BIN_DIR="$INSTALL_DIR/bin"

red() { printf '\033[31m%s\033[0m\n' "$*"; }
bold() { printf '\033[1m%s\033[0m\n' "$*"; }
dim() { printf '\033[2m%s\033[0m\n' "$*"; }

err() {
  red "error: $*" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || err "curl is required"
command -v tar >/dev/null 2>&1 || err "tar is required"

# --- detect platform --------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) os_part="linux" ;;
  Darwin) os_part="macos" ;;
  FreeBSD) os_part="freebsd" ;;
  *) err "unsupported OS: $os (use install.ps1 on Windows)" ;;
esac
case "$arch" in
  x86_64 | amd64) arch_part="x86-64" ;;
  arm64 | aarch64) arch_part="arm64" ;;
  *) err "unsupported architecture: $arch" ;;
esac

target="${os_part}-${arch_part}"

# --- resolve version --------------------------------------------------------
version="${TSR_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" |
    grep -oE '"tag_name": *"[^"]+"' | head -1 | cut -d'"' -f4)"
  [ -n "$version" ] || err "could not determine the latest release (set TSR_VERSION)"
fi
name="tsr-${target}"
url="https://github.com/$REPO/releases/download/${version}/${name}.tar.gz"

bold "Installing tsr ${version} (${target})"
dim "  from $url"

# --- download + verify ------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fSL --progress-bar "$url" -o "$tmp/$name.tar.gz" ||
  err "download failed — is there a release asset for $target?"

# Checksums, when present, live in one `checksums.txt` per release
# (`<hash>  <archive>` lines); pull out the line for our archive and verify it.
sums_url="https://github.com/$REPO/releases/download/${version}/checksums.txt"
if curl -fsSL "$sums_url" -o "$tmp/checksums.txt" 2>/dev/null &&
  grep " ${name}.tar.gz\$" "$tmp/checksums.txt" > "$tmp/$name.tar.gz.sha256"; then
  if command -v shasum >/dev/null 2>&1; then
    (cd "$tmp" && shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null) ||
      err "checksum verification failed"
  elif command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && sha256sum -c "$name.tar.gz.sha256" >/dev/null) ||
      err "checksum verification failed"
  fi
  dim "  checksum verified"
else
  dim "  no checksums.txt for this release — skipping verification"
fi

# --- install ----------------------------------------------------------------
tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$BIN_DIR"
install -m 0755 "$tmp/tsr" "$BIN_DIR/tsr"

bold ""
bold "tsr was installed to $BIN_DIR/tsr"

# Suggest a PATH entry if it isn't already there.
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    if [ -t 0 ] || [ -c /dev/tty ]; then
      echo
      printf "Would you like to add tsr to your shell profile automatically? [y/N] "
      if read -r ans < /dev/tty && { [ "$ans" = "y" ] || [ "$ans" = "Y" ]; }; then
        shell_profile=""
        case "${SHELL:-}" in
          */zsh) shell_profile="$HOME/.zshrc" ;;
          */bash) shell_profile="$HOME/.bashrc" ;;
          *) 
            if [ -f "$HOME/.bashrc" ]; then shell_profile="$HOME/.bashrc"
            elif [ -f "$HOME/.zshrc" ]; then shell_profile="$HOME/.zshrc"
            elif [ -f "$HOME/.profile" ]; then shell_profile="$HOME/.profile"
            fi
            ;;
        esac
        if [ -n "$shell_profile" ]; then
          echo "" >> "$shell_profile"
          echo "# tsr" >> "$shell_profile"
          echo "export PATH=\"$BIN_DIR:\$PATH\"" >> "$shell_profile"
          echo
          bold "Added PATH to $shell_profile"
          dim "Restart your terminal or run: source $shell_profile"
        else
          echo
          echo "Could not determine shell profile. Please add it manually:"
          bold "  export PATH=\"$BIN_DIR:\$PATH\""
        fi
      else
        echo
        echo "Please add it manually:"
        bold "  export PATH=\"$BIN_DIR:\$PATH\""
      fi
    else
      echo "Please add it manually:"
      bold "  export PATH=\"$BIN_DIR:\$PATH\""
    fi
    ;;
esac
echo
dim "Run 'tsr --version' to verify."
