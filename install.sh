#!/usr/bin/env sh
# install.sh — Installs `tkmd` (with optional `tk` alias) without requiring Rust or Cargo.
# Usage: curl -fsSL https://raw.githubusercontent.com/ktreharrison/tkmd/main/install.sh | sh

set -e

REPO="ktreharrison/tkmd"
BIN_NAME="tkmd"

# Detect OS
OS="$(uname -s)"
case "${OS}" in
    Darwin) TARGET_OS="apple-darwin" ;;
    Linux)  TARGET_OS="unknown-linux-gnu" ;;
    *)
        echo "Error: Unsupported operating system: ${OS}" >&2
        exit 1
        ;;
esac

# Detect Architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64|amd64) TARGET_ARCH="x86_64" ;;
    arm64|aarch64) TARGET_ARCH="aarch64" ;;
    *)
        echo "Error: Unsupported architecture: ${ARCH}" >&2
        exit 1
        ;;
esac

TARGET="${TARGET_ARCH}-${TARGET_OS}"
echo "Detected platform: ${TARGET}"

# Determine install directory
if [ -w "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
elif [ -n "${HOME}" ] && [ -d "${HOME}/.local/bin" ]; then
    INSTALL_DIR="${HOME}/.local/bin"
elif [ -n "${HOME}" ]; then
    INSTALL_DIR="${HOME}/.local/bin"
    mkdir -p "${INSTALL_DIR}"
else
    INSTALL_DIR="/usr/local/bin"
fi

# Fetch latest release tag
echo "Fetching latest release tag for ${REPO}..."
LATEST_TAG=$(curl -sSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "${LATEST_TAG}" ]; then
    echo "Warning: Could not fetch latest release from GitHub API; falling back to v0.1.1"
    LATEST_TAG="v0.1.1"
fi

ARCHIVE_NAME="${BIN_NAME}-${LATEST_TAG}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/ktreharrison/tkmd/releases/download/${LATEST_TAG}/${ARCHIVE_NAME}"

CHECKSUM_NAME="${ARCHIVE_NAME}.sha256"
CHECKSUM_URL="https://github.com/ktreharrison/tkmd/releases/download/${LATEST_TAG}/${CHECKSUM_NAME}"

echo "Downloading ${BIN_NAME} (${LATEST_TAG}) for ${TARGET}..."
TMP_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_DIR}"' EXIT

if ! curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${ARCHIVE_NAME}"; then
    echo "" >&2
    echo "Error: Prebuilt binary for platform '${TARGET}' not found in release ${LATEST_TAG}." >&2
    echo "You can build directly from source using Cargo:" >&2
    echo "  cargo install --git https://github.com/${REPO}.git" >&2
    echo "" >&2
    exit 1
fi

echo "Verifying SHA-256 checksum..."
if curl -fsSL "${CHECKSUM_URL}" -o "${TMP_DIR}/${CHECKSUM_NAME}" 2>/dev/null; then
    VERIFIED=0
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "${TMP_DIR}" && sha256sum -c "${CHECKSUM_NAME}" >/dev/null 2>&1) && VERIFIED=1
    elif command -v shasum >/dev/null 2>&1; then
        (cd "${TMP_DIR}" && shasum -a 256 -c "${CHECKSUM_NAME}" >/dev/null 2>&1) && VERIFIED=1
    fi

    if [ "${VERIFIED}" -eq 1 ]; then
        echo "✓ Checksum verified successfully."
    else
        echo "Error: Checksum verification failed for ${ARCHIVE_NAME}!" >&2
        exit 1
    fi
else
    echo "Notice: No checksum file found for ${ARCHIVE_NAME}; continuing install."
fi

tar -xzf "${TMP_DIR}/${ARCHIVE_NAME}" -C "${TMP_DIR}"

if [ -w "${INSTALL_DIR}" ]; then
    mv "${TMP_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    chmod +x "${INSTALL_DIR}/${BIN_NAME}"
    if [ -f "${TMP_DIR}/tk" ]; then
        mv "${TMP_DIR}/tk" "${INSTALL_DIR}/tk"
        chmod +x "${INSTALL_DIR}/tk"
    fi
else
    echo "Installing to ${INSTALL_DIR} requires sudo privileges..."
    sudo mv "${TMP_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
    sudo chmod +x "${INSTALL_DIR}/${BIN_NAME}"
    if [ -f "${TMP_DIR}/tk" ]; then
        sudo mv "${TMP_DIR}/tk" "${INSTALL_DIR}/tk"
        sudo chmod +x "${INSTALL_DIR}/tk"
    fi
fi

# If tk binary was not separately installed, create alias if tk is free
if [ ! -e "${INSTALL_DIR}/tk" ]; then
    if ! command -v tk >/dev/null 2>&1; then
        ln -s "${INSTALL_DIR}/${BIN_NAME}" "${INSTALL_DIR}/tk" 2>/dev/null || true
    fi
fi

# Install shell completions if directory exists
if [ -d "/opt/homebrew/share/zsh/site-functions" ] && [ -w "/opt/homebrew/share/zsh/site-functions" ]; then
    "${INSTALL_DIR}/${BIN_NAME}" completions zsh > "/opt/homebrew/share/zsh/site-functions/_tkmd" 2>/dev/null || true
    ln -sf "/opt/homebrew/share/zsh/site-functions/_tkmd" "/opt/homebrew/share/zsh/site-functions/_tk" 2>/dev/null || true
elif [ -d "/usr/local/share/zsh/site-functions" ] && [ -w "/usr/local/share/zsh/site-functions" ]; then
    "${INSTALL_DIR}/${BIN_NAME}" completions zsh > "/usr/local/share/zsh/site-functions/_tkmd" 2>/dev/null || true
    ln -sf "/usr/local/share/zsh/site-functions/_tkmd" "/usr/local/share/zsh/site-functions/_tk" 2>/dev/null || true
fi

echo ""
echo "Successfully installed ${BIN_NAME} to ${INSTALL_DIR}/${BIN_NAME}!"
echo ""
"${INSTALL_DIR}/${BIN_NAME}" --version || true
echo ""
echo "Run 'tkmd init' or 'tk init' to get started."
