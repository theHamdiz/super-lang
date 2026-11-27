#!/bin/sh
# Super 0.1.0 — verified one-line installer.
#
#   sh -c 'f=$(mktemp) && trap "rm -f -- \"$f\"" 0 && curl --proto =https -fL https://thehamdiz.github.io/super-lang/install.sh -o "$f" && sh "$f" "$@"' sh
#
# Layout (outside ~/.cargo):
#   $SUPER_HOME/bin/{super,super-lsp,super-mcp}
#   $SUPER_HOME/lib/stdlib/
#   $SUPER_HOME/registry/   (pods)
#   $SUPER_HOME/cache/      (git deps)
#
# SUPER_HOME defaults to ~/.super (or $XDG_DATA_HOME/super when SUPER_USE_XDG=1).
set -eu

SUPER_VERSION="${SUPER_VERSION:-0.1.0}"
RELEASE_BASE="${SUPER_BOOTSTRAP_BASE_URL:-https://github.com/theHamdiz/super-lang/releases/latest/download}"
REPO_URL="${SUPER_REPO:-https://github.com/theHamdiz/super-lang.git}"
REPO_REF="${SUPER_REF:-v${SUPER_VERSION}}"

say() { printf '%s\n' "$*"; }
fail() { say "error: $*"; exit 1; }

resolve_home() {
    if [ -n "${SUPER_HOME:-}" ]; then
        printf '%s\n' "$SUPER_HOME"
        return
    fi
    if [ "${SUPER_USE_XDG:-0}" = "1" ] && [ -n "${XDG_DATA_HOME:-}" ]; then
        printf '%s\n' "$XDG_DATA_HOME/super"
        return
    fi
    home="${HOME:-}"
    [ -n "$home" ] || fail "HOME is unset"
    printf '%s\n' "$home/.super"
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "missing tool: $1"
}

# Prebuilt v0.1.0 release triples (must match GitHub release assets).
SHIPPED_TARGETS="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-unknown-freebsd"

detect_target() {
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    case "$arch" in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) fail "unsupported CPU: $arch — see the 0.1.0 platform support matrix" ;;
    esac
    case "$os" in
        linux)
            libc=gnu
            if [ -e /lib/ld-musl-x86_64.so.1 ] || [ -e /lib/ld-musl-aarch64.so.1 ] \
                || [ -e /usr/lib/ld-musl-x86_64.so.1 ] || [ -e /usr/lib/ld-musl-aarch64.so.1 ]; then
                libc=musl
            fi
            target="${arch}-unknown-linux-${libc}"
            ;;
        freebsd)
            [ "$arch" = x86_64 ] || fail "FreeBSD builds are x86_64-only for now"
            target="x86_64-unknown-freebsd"
            ;;
        darwin)
            target="${arch}-apple-darwin"
            ;;
        mingw*|msys*|cygwin*|windows*)
            fail "use install.ps1 on Windows"
            ;;
        *)
            fail "unsupported OS: $os — see the 0.1.0 platform support matrix"
            ;;
    esac
    case " $SHIPPED_TARGETS " in
        *" $target "*) printf '%s\n' "$target" ;;
        *)
            fail "no prebuilt bundle for $target in 0.1.0 (shipped: $SHIPPED_TARGETS). Build from a git checkout."
            ;;
    esac
}

fetch() {
    url=$1
    dest=$2
    if command -v curl >/dev/null 2>&1; then
        curl --proto =https --tlsv1.2 -fL --progress-bar -o "$dest" "$url"
    elif command -v fetch >/dev/null 2>&1; then
        fetch -o "$dest" "$url"
    else
        fail "need curl (or FreeBSD fetch)"
    fi
}

file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256 >/dev/null 2>&1; then
        sha256 -q "$1"
    else
        fail "need sha256sum, shasum, or sha256"
    fi
}

persist_path() {
    bin_dir=$1
    shell_name=$(basename "${SHELL:-/bin/sh}")
    marker_begin="# >>> super path >>>"
    marker_end="# <<< super path <<<"
    case "$shell_name" in
        zsh) profile="$HOME/.zshenv" ;;
        bash) profile="$HOME/.bashrc" ;;
        fish) profile="$HOME/.config/fish/config.fish" ;;
        *) profile="$HOME/.profile" ;;
    esac
    mkdir -p "$(dirname "$profile")" 2>/dev/null || true
    case ":${PATH:-}:" in
        *":$bin_dir:"*) return 0 ;;
    esac
    block=""
    case "$shell_name" in
        fish)
            block="$marker_begin
fish_add_path -g $bin_dir
$marker_end"
            ;;
        *)
            block="$marker_begin
case \":\$PATH:\" in
  *\":$bin_dir:\"*) ;;
  *) export PATH=\"$bin_dir:\$PATH\" ;;
esac
$marker_end"
            ;;
    esac
    if [ -f "$profile" ] && grep -q "$marker_begin" "$profile" 2>/dev/null; then
        say "PATH marker already present in $profile"
    else
        printf '\n%s\n' "$block" >> "$profile"
        say "Added $bin_dir to PATH via $profile (open a new shell)"
    fi
    export PATH="$bin_dir:${PATH:-}"
}

install_from_dir() {
    src=$1
    home=$2
    bin="$home/bin"
    std="$home/lib/stdlib"
    mkdir -p "$bin" "$std" "$home/registry" "$home/cache"
    if [ -x "$src/bin/super" ]; then
        cp -f "$src/bin/super" "$bin/super"
        chmod +x "$bin/super"
    elif [ -x "$src/super" ]; then
        cp -f "$src/super" "$bin/super"
        chmod +x "$bin/super"
    else
        fail "bundle missing bin/super"
    fi
    if [ -d "$src/stdlib" ]; then
        rm -rf "$std"
        mkdir -p "$std"
        cp -a "$src/stdlib/." "$std/"
    elif [ -d "$src/lib/stdlib" ]; then
        rm -rf "$std"
        mkdir -p "$std"
        cp -a "$src/lib/stdlib/." "$std/"
    else
        fail "bundle missing stdlib/"
    fi
    cat > "$bin/super-lsp" <<'EOF'
#!/bin/sh
exec super lsp "$@"
EOF
    cat > "$bin/super-mcp" <<'EOF'
#!/bin/sh
exec super mcp "$@"
EOF
    chmod +x "$bin/super-lsp" "$bin/super-mcp"
    {
        echo "version=$SUPER_VERSION"
        echo "installed=$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo unknown)"
        echo "home=$home"
    } > "$std/.sr-stdlib-install"
}

try_release() {
    home=$1
    target=$(detect_target)
    archive="super-${SUPER_VERSION}-${target}.tar.gz"
    work=$(mktemp -d 2>/dev/null || mktemp -d -t super-install)
    trap 'rm -rf -- "$work"' EXIT
    say "Trying release bundle: $archive"
    if ! fetch "$RELEASE_BASE/$archive" "$work/$archive"; then
        say "No prebuilt bundle for $target — will build from source."
        rm -rf -- "$work"
        trap - EXIT
        return 1
    fi
    fetch "$RELEASE_BASE/SHA256SUMS" "$work/SHA256SUMS" 2>/dev/null \
        || fail "SHA256SUMS is required; refusing unverified archive"
    expected=$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$work/SHA256SUMS")
    [ -n "$expected" ] || fail "SHA256SUMS has no entry for $archive"
    actual=$(file_sha256 "$work/$archive")
    [ "$actual" = "$expected" ] || fail "checksum mismatch for $archive"
    say "Checksum OK"
    mkdir -p "$work/out"
    tar -xzf "$work/$archive" -C "$work/out" || fail "failed to unpack $archive"
    # Find extracted root (either flat or single top-level dir).
    root="$work/out"
    if [ ! -x "$root/bin/super" ] && [ ! -x "$root/super" ]; then
        for cand in "$work"/out/*; do
            if [ -x "$cand/bin/super" ] || [ -x "$cand/super" ]; then
                root=$cand
                break
            fi
        done
    fi
    install_from_dir "$root" "$home"
    rm -rf -- "$work"
    trap - EXIT
    return 0
}

build_from_source() {
    home=$1
    need_cmd cargo
    need_cmd git
    work=$(mktemp -d 2>/dev/null || mktemp -d -t super-src)
    trap 'rm -rf -- "$work"' EXIT
    say "Cloning $REPO_URL ($REPO_REF)…"
    if ! git clone --depth 1 --branch "$REPO_REF" "$REPO_URL" "$work/src" 2>/dev/null; then
        say "Tag $REPO_REF missing — cloning default branch"
        git clone --depth 1 "$REPO_URL" "$work/src" || fail "git clone failed"
    fi
    say "Building release CLI (this may take a few minutes)…"
    (cd "$work/src" && cargo build --release -p super-cli --locked) \
        || (cd "$work/src" && cargo build --release -p super-cli) \
        || fail "cargo build failed"
    mkdir -p "$work/bundle/bin"
    cp -f "$work/src/target/release/super" "$work/bundle/bin/super"
    chmod +x "$work/bundle/bin/super"
    cp -a "$work/src/stdlib" "$work/bundle/stdlib"
    install_from_dir "$work/bundle" "$home"
    rm -rf -- "$work"
    trap - EXIT
}

main() {
    home=$(resolve_home)
    bin="$home/bin"
    say "Super ${SUPER_VERSION} → $home"
    mkdir -p "$home"
    # detect_target fails fast on unsupported hosts (non-zero exit).
    detect_target >/dev/null
    if ! try_release "$home"; then
        if [ "${SUPER_ALLOW_SOURCE:-0}" = "1" ]; then
            build_from_source "$home"
        else
            fail "no prebuilt release bundle for this host. Set SUPER_ALLOW_SOURCE=1 to build from git, or install on a shipped target (see platform docs)."
        fi
    fi
    persist_path "$bin"
    say ""
    say "Installed:"
    say "  $bin/super"
    say "  $bin/super-lsp"
    say "  $bin/super-mcp"
    say "  $home/lib/stdlib"
    say ""
    say "Resolution: SUPER_STD > project ./stdlib > \$SUPER_HOME/lib/stdlib > embedded"
    say "Override:   export SUPER_HOME=$home"
    say "            export SUPER_STD=$home/lib/stdlib"
    if command -v super >/dev/null 2>&1; then
        super --version 2>/dev/null || super --help | head -5 || true
    else
        say "Run: export PATH=\"$bin:\$PATH\" && super --help"
    fi
}

main "$@"
