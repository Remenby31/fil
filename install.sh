#!/bin/sh
set -e

# Fil installer — https://fil.sh
# Usage: curl -fsSL https://fil.sh/install.sh | sh

REPO="Remenby31/fil"
VERSION="v0.1.0"
INSTALL_DIR="/usr/local/bin"

main() {
    echo ""
    echo "  \033[1mfil\033[32m.sh\033[0m installer"
    echo ""

    # Detect OS and architecture
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
    ARCH=$(uname -m)

    case "$ARCH" in
        x86_64)  ARCH="x86_64" ;;
        arm64)   ARCH="arm64" ;;
        aarch64) ARCH="arm64" ;;
        *)       error "Unsupported architecture: $ARCH" ;;
    esac

    case "$OS" in
        darwin) OS="darwin" ;;
        linux)  OS="linux" ;;
        *)      error "Unsupported OS: $OS" ;;
    esac

    BINARY="fil-${VERSION}-${OS}-${ARCH}.tar.gz"
    URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY}"

    echo "  Platform: ${OS}/${ARCH}"
    echo "  Version:  ${VERSION}"
    echo ""

    # Download
    echo "  Downloading..."
    TMPDIR=$(mktemp -d)
    if curl -fsSL "$URL" -o "${TMPDIR}/fil.tar.gz" 2>/dev/null; then
        tar xzf "${TMPDIR}/fil.tar.gz" -C "${TMPDIR}"

        # Install
        if [ -w "$INSTALL_DIR" ]; then
            mv "${TMPDIR}/fil" "${INSTALL_DIR}/fil"
        else
            echo "  Installing to ${INSTALL_DIR} (requires sudo)..."
            sudo mv "${TMPDIR}/fil" "${INSTALL_DIR}/fil"
        fi
        chmod +x "${INSTALL_DIR}/fil"

        rm -rf "${TMPDIR}"

        echo "  \033[32m✓\033[0m Installed to ${INSTALL_DIR}/fil"
        echo ""
        echo "  Get started:"
        echo "    \033[32m$\033[0m fil setup --hub https://YOUR_HUB_URL"
        echo "    \033[32m$\033[0m fil"
        echo ""
        offer_agent_notifications "${INSTALL_DIR}/fil"
    else
        rm -rf "${TMPDIR}"

        # Fallback to Homebrew
        echo "  Binary not available for ${OS}/${ARCH}, trying Homebrew..."
        if command -v brew >/dev/null 2>&1; then
            brew tap Remenby31/fil 2>/dev/null
            brew install fil
        else
            error "Download failed and Homebrew not found. Install manually from https://github.com/${REPO}"
        fi
    fi
}

# Opt-in: FIL_INSTALL_MCP=1 answers yes, FIL_INSTALL_MCP=0 skips the question.
# `curl | sh` has no stdin, so the prompt reads from the terminal directly.
offer_agent_notifications() {
    fil_bin="$1"
    case "${FIL_INSTALL_MCP:-}" in
        1|yes|true) answer=y ;;
        0|no|false) return 0 ;;
        *)
            [ -r /dev/tty ] || return 0
            printf "  Let coding agents (Claude Code, Codex, Cursor...) notify your phone? [y/N] "
            read -r answer < /dev/tty || answer=n
            ;;
    esac
    case "$answer" in
        y|Y|yes|YES) "$fil_bin" mcp install || true ;;
        *) echo "  Later: \033[32m$\033[0m fil mcp install" ;;
    esac
    echo ""
}

error() {
    echo "  \033[31m✗\033[0m $1" >&2
    exit 1
}

main
