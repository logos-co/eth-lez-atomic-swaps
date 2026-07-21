#!/usr/bin/env bash
#
# macOS launch bridge for a scaffold Basecamp profile.
#
# Replicates what `lgs basecamp launch <profile>` does, BUT with an ABSOLUTE
# LOGOS_DATA_DIR. The pinned bin-macos-app Basecamp (a746cdbc / v0.1.1) ignores
# XDG_DATA_HOME on macOS and reads its installed modules from LOGOS_DATA_DIR.
# That path must be absolute: a relative LOGOS_DATA_DIR loads the backend modules
# but breaks @rpath resolution for the dlopen'd main_ui / package_manager_ui
# dylibs ("shared library was not found"), so the shell UI never renders. An
# absolute path loads everything.
#
# Scaffold cannot portably express an absolute per-profile path in a committed
# scaffold.toml, so this app-owned helper computes it at launch. It is a bridge
# until `lgs basecamp launch` sets an absolute LOGOS_DATA_DIR for a bin-macos-app
# attr on macOS upstream (candidate Scaffold ask). On Linux, XDG isolation works
# and `lgs basecamp launch <profile>` needs no bridge.
#
# Prerequisites:
#   - `lgs basecamp setup` built the portable Basecamp under .scaffold/lez-cache/basecamp/
#   - `lgs basecamp install` populated the profile under .scaffold/basecamp/profiles/<profile>/
#   - `make infra` wrote the .env / .env.taker files (keep it running)
#
# Usage: scripts/basecamp-launch.sh <maker|taker>

set -euo pipefail

NAME=${1:-}
case "${NAME}" in
    maker|taker) ;;
    *)
        echo "usage: scripts/basecamp-launch.sh <maker|taker>" >&2
        exit 2
        ;;
esac

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE_DIR=${REPO_ROOT}/.scaffold/basecamp/profiles/${NAME}
# Absolute — required by bin-macos-app on macOS (see header).
DATA_DIR=${PROFILE_DIR}/xdg-data/Logos/LogosBasecamp
# Short runtime dir: liblogos binds Unix sockets under sun_path's 104-char cap.
RUNTIME_DIR=/tmp/lgs-${NAME}
case "${NAME}" in
    maker) ENV_FILE=${REPO_ROOT}/.env ;;
    taker) ENV_FILE=${REPO_ROOT}/.env.taker ;;
esac

if [ ! -d "${PROFILE_DIR}" ]; then
    echo "error: profile '${NAME}' not found at ${PROFILE_DIR}." >&2
    echo "       Run 'lgs basecamp setup' and 'lgs basecamp install' first." >&2
    exit 1
fi
if [ ! -f "${ENV_FILE}" ]; then
    echo "error: ${ENV_FILE} missing. Run 'make infra' and leave it running." >&2
    exit 1
fi

# Resolve the pinned bin-macos-app Basecamp built by `lgs basecamp setup`.
BIN=$(find -L "${REPO_ROOT}/.scaffold/lez-cache/basecamp" -maxdepth 6 -type f \
        \( -path '*/app-result/*/Contents/MacOS/LogosBasecamp' \
           -o -path '*/bin/LogosBasecamp' \) 2>/dev/null | head -1)
if [ -z "${BIN}" ] || [ ! -x "${BIN}" ]; then
    echo "error: could not find the portable Basecamp binary under" >&2
    echo "       ${REPO_ROOT}/.scaffold/lez-cache/basecamp. Run 'lgs basecamp setup' first." >&2
    exit 1
fi

# Recreate the short runtime dir on each launch so previous sockets are gone.
rm -rf "${RUNTIME_DIR}"
mkdir -p "${RUNTIME_DIR}"
chmod 700 "${RUNTIME_DIR}"

# Run from REPO_ROOT so relative paths in .env / .env.taker resolve.
cd "${REPO_ROOT}"

# Mirror output into the profile's basecamp.log (the path scaffold's log_file
# would use) so diagnostics survive the session; tee keeps it on the terminal.
exec > >(tee -a "${PROFILE_DIR}/basecamp.log") 2>&1

exec env -i \
    PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
    HOME="${HOME}" \
    XDG_CONFIG_HOME="${PROFILE_DIR}/xdg-config" \
    XDG_CACHE_HOME="${PROFILE_DIR}/xdg-cache" \
    XDG_DATA_HOME="${PROFILE_DIR}/xdg-data" \
    LOGOS_DATA_DIR="${DATA_DIR}" \
    TMPDIR="${RUNTIME_DIR}" \
    XDG_RUNTIME_DIR="${RUNTIME_DIR}" \
    LOGOS_PROFILE="${NAME}" \
    SWAP_UI_AUTO_ENV_FILE="${ENV_FILE}" \
    SWAP_UI_AUTO_ROLE="${NAME}" \
    QT_LOGGING_RULES="qt.qpa.*=false" \
    "${BIN}"
