#!/usr/bin/env sh
# install.sh — Installs `tk` (taskmd) without requiring Rust or Cargo.
# Usage: curl -fsSL https://raw.githubusercontent.com/ktreharrison/taskmd/main/install.sh | sh

set -e

REPO="ktreharrison/taskmd"
BIN_NAME="tk"

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
    echo "Warning: Could not fetch latest release from GitHub API; falling back to v0.1.0"
    LATEST_TAG="v0.1.0"
fi

ARCHIVE_NAME="${BIN_NAME}-${LATEST_TAG}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/ktreharrison/taskmd/releases/download/${LATEST_TAG}/${ARCHIVE_NAME}"

echo "Downloading ${BIN_NAME} (${LATEST_TAG}) for ${TARGET}..."
TMP_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_DIR}"' EXIT

curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${ARCHIVE_NAME}"
tar -xzf "${TMP_DIR}/${ARCHIVE_NAME}" -C "${TMP_DIR}"

if [ -w "${INSTALL_DIR}" ]; then
    mv "${TMP_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
else
    echo "Installing to ${INSTALL_DIR} requires sudo privileges..."
    sudo mv "${TMP_DIR}/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
fi

chmod +x "${INSTALL_DIR}/${BIN_NAME}"

echo ""
echo "Successfully installed ${BIN_NAME} to ${INSTALL_DIR}/${BIN_NAME}!"
echo ""
"${INSTALL_DIR}/${BIN_NAME}" --version || true
echo ""
echo "Run 'tk init' to get started."
