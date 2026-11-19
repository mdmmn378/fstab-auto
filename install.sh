#!/bin/sh
# fstab-auto installer.
#
#   curl -fsSL https://raw.githubusercontent.com/mdmmn378/fstab-auto/main/install.sh | sh
#
# Installs the binary and, for the shells found on this machine (bash, zsh,
# fish), their completion scripts.
#
# Options (environment variables):
#   FSTAB_AUTO_VERSION         version to install, e.g. 0.2.0 (default: latest release)
#   FSTAB_AUTO_INSTALL_DIR     where to put the binary (default: /usr/local/bin)
#   FSTAB_AUTO_NO_COMPLETIONS  set to 1 to skip the shell completions
#   FSTAB_AUTO_BASE_URL        download from here instead of GitHub (mirror / offline use);
#                              must contain the release .tar.gz and .sha256 files
#
# Uninstall:
#   curl -fsSL https://raw.githubusercontent.com/mdmmn378/fstab-auto/main/install.sh | sh -s -- --uninstall
set -eu

REPO="mdmmn378/fstab-auto"
BIN="fstab-auto"
VERSION="${FSTAB_AUTO_VERSION:-latest}"
INSTALL_DIR="${FSTAB_AUTO_INSTALL_DIR:-/usr/local/bin}"

say() { printf '%s\n' "$*"; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

# The closest existing ancestor of a path: what decides whether we may write.
existing_parent() {
    dir=$1
    while [ ! -e "$dir" ]; do dir=$(dirname "$dir"); done
    printf '%s\n' "$dir"
}

# write_in DIR COMMAND...: runs the command directly when DIR is writable,
# through sudo otherwise.
write_in() {
    dir=$1
    shift
    if [ -w "$(existing_parent "$dir")" ] || [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif have sudo; then
        [ -n "${SUDO_NOTED:-}" ] || say "Writing to $dir needs root, using sudo."
        SUDO_NOTED=1
        sudo "$@"
    else
        die "cannot write to $dir and sudo is not available. Set FSTAB_AUTO_INSTALL_DIR to a writable directory."
    fi
}

download() {
    if have curl; then
        curl --fail --silent --show-error --location --output "$2" "$1"
    elif have wget; then
        wget --quiet --output-document "$2" "$1"
    else
        die "curl or wget is required"
    fi
}

# Prints "<shell> <file in archive> <destination>" for each completion script,
# using directories the shells search without any user configuration.
completion_targets() {
    case "$(uname -s)" in
        Linux)
            printf 'bash fstab-auto.bash /usr/local/share/bash-completion/completions/fstab-auto\n'
            printf 'zsh _fstab-auto /usr/local/share/zsh/site-functions/_fstab-auto\n'
            # fish only looks under /usr/local/share when XDG_DATA_DIRS says
            # so, which SSH sessions usually lack; /etc/fish is always read.
            printf 'fish fstab-auto.fish /etc/fish/completions/fstab-auto.fish\n'
            ;;
        Darwin)
            have brew || return 0
            share="$(brew --prefix)/share"
            printf 'bash fstab-auto.bash %s/bash-completion/completions/fstab-auto\n' "$share"
            printf 'zsh _fstab-auto %s/zsh/site-functions/_fstab-auto\n' "$share"
            printf 'fish fstab-auto.fish %s/fish/vendor_completions.d/fstab-auto.fish\n' "$share"
            ;;
    esac
}

install_completions() {
    source_dir=$1
    [ "${FSTAB_AUTO_NO_COMPLETIONS:-}" != "1" ] || return 0
    [ -d "$source_dir" ] || return 0
    installed=""
    while read -r shell file dest; do
        have "$shell" || continue
        [ -f "$source_dir/$file" ] || continue
        write_in "$(dirname "$dest")" mkdir -p "$(dirname "$dest")" </dev/null
        write_in "$(dirname "$dest")" install -m 0644 "$source_dir/$file" "$dest" </dev/null
        installed="$installed $shell"
    done <<TARGETS
$(completion_targets)
TARGETS
    if [ -n "$installed" ]; then
        say "Installed completions for:$installed (open a new shell to use them)"
    else
        say "No shell completions installed. Generate one with: $BIN completions <bash|zsh|fish>"
    fi
}

uninstall() {
    target="$INSTALL_DIR/$BIN"
    [ -e "$target" ] || die "$target is not installed"
    write_in "$INSTALL_DIR" rm -f "$target"
    say "Removed $target."
    while read -r _shell _file dest; do
        if [ -n "$dest" ] && [ -e "$dest" ]; then
            write_in "$(dirname "$dest")" rm -f "$dest" </dev/null
            say "Removed $dest."
        fi
    done <<TARGETS
$(completion_targets)
TARGETS
    say "Mounts and fstab entries were left as they are."
}

install_release() {
    case "$(uname -m)" in
        x86_64 | amd64) arch="x86_64" ;;
        aarch64 | arm64) arch="aarch64" ;;
        *) die "unsupported CPU architecture: $(uname -m)" ;;
    esac
    case "$(uname -s)" in
        Linux) platform="unknown-linux-musl" ;;
        Darwin)
            [ "$arch" = "aarch64" ] || die "on macOS only Apple silicon is supported"
            platform="apple-darwin"
            ;;
        *) die "unsupported operating system: $(uname -s)" ;;
    esac
    have tar || die "tar is required"

    asset="$BIN-$arch-$platform"
    if [ -n "${FSTAB_AUTO_BASE_URL:-}" ]; then
        base="${FSTAB_AUTO_BASE_URL%/}"
    elif [ "$VERSION" = "latest" ]; then
        base="https://github.com/$REPO/releases/latest/download"
    else
        base="https://github.com/$REPO/releases/download/v${VERSION#v}"
    fi

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "Downloading $asset.tar.gz ($VERSION)"
    download "$base/$asset.tar.gz" "$tmp/$asset.tar.gz" ||
        die "download failed: $base/$asset.tar.gz"
    download "$base/$asset.tar.gz.sha256" "$tmp/$asset.tar.gz.sha256" ||
        die "download failed: $base/$asset.tar.gz.sha256"

    if have sha256sum; then
        (cd "$tmp" && sha256sum -c "$asset.tar.gz.sha256" >/dev/null) ||
            die "checksum mismatch, refusing to install"
        say "Checksum verified."
    elif have shasum; then
        (cd "$tmp" && shasum -a 256 -c "$asset.tar.gz.sha256" >/dev/null) ||
            die "checksum mismatch, refusing to install"
        say "Checksum verified."
    else
        say "warning: no sha256 tool found, skipping checksum verification" >&2
    fi

    tar -xzf "$tmp/$asset.tar.gz" -C "$tmp"
    [ -f "$tmp/$asset/$BIN" ] || die "archive does not contain $BIN"

    write_in "$INSTALL_DIR" mkdir -p "$INSTALL_DIR"
    write_in "$INSTALL_DIR" install -m 0755 "$tmp/$asset/$BIN" "$INSTALL_DIR/$BIN"
    say "Installed $("$INSTALL_DIR/$BIN" --version) to $INSTALL_DIR/$BIN"
    install_completions "$tmp/$asset/completions"

    if [ "$(uname -s)" = "Darwin" ]; then
        say ""
        say "Note: mounting is supported on Linux only for now; on macOS $BIN runs in --dry-run mode."
    elif ! have mount.cifs && [ ! -x /sbin/mount.cifs ] && [ ! -x /usr/sbin/mount.cifs ]; then
        say ""
        say "Note: mounting CIFS/SMB shares needs the cifs-utils package:"
        if have apt-get; then
            say "  sudo apt-get install -y cifs-utils"
        elif have dnf; then
            say "  sudo dnf install -y cifs-utils"
        elif have pacman; then
            say "  sudo pacman -S cifs-utils"
        else
            say "  install cifs-utils with your package manager"
        fi
    fi
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) say "Note: $INSTALL_DIR is not in your PATH." ;;
    esac
    say ""
    say "Run '$BIN' to mount a share, or '$BIN --dry-run' to look around first."
}

# Wrapped in a function and called on the last line, so a download that is cut
# off halfway can never run a partial script.
main() {
    case "${1:-}" in
        "") install_release ;;
        --uninstall) uninstall ;;
        -h | --help) say "See the comments at the top of install.sh, or the README." ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
}

main "$@"
