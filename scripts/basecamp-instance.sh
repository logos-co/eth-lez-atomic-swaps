#!/usr/bin/env bash
#
# Drive an isolated LogosBasecamp instance for cross-node dogfooding.
#
# Usage:
#   scripts/basecamp-instance.sh init  <maker|taker>   # create dirs + install LGX
#   scripts/basecamp-instance.sh run   <maker|taker>   # launch Basecamp
#   scripts/basecamp-instance.sh clean <maker|taker>   # remove instance dir
#   scripts/basecamp-instance.sh paths <maker|taker>   # print resolved paths
#
# Environment overrides:
#   BASECAMP_INSTANCES_DIR   default: <repo>/.basecamp
#   BASECAMP_FLAKE           default: github:logos-co/logos-basecamp
#   BASECAMP_PACKAGE         default: bin-macos-app on macOS, portable on Linux
#   BASECAMP_SKIP_ENV_CHECK  set to 1 to launch without requiring .env files
#
# Each instance has fully isolated:
#   - --user-dir       -> .basecamp/<name>/data           (Basecamp's modules,
#                                                          plugins, module_data,
#                                                          logs all live here)
#   - HOME             -> .basecamp/<name>/home
#   - XDG_CONFIG_HOME  -> .basecamp/<name>/xdg/config
#   - XDG_CACHE_HOME   -> .basecamp/<name>/xdg/cache
#   - XDG_DATA_HOME    -> .basecamp/<name>/xdg/data
#   - XDG_RUNTIME_DIR  -> /tmp/lbc-<name>                 (must be short — the
#                                                          liblogos token sockets
#                                                          are bound by the macOS
#                                                          104-char sun_path
#                                                          limit)
#   - NSSA_WALLET_HOME_DIR -> .basecamp/<name>/wallet
#
# `swap-ui` already chooses a per-process random Delivery portsShift, so two
# concurrent instances will not collide on libp2p / discovery ports.

set -euo pipefail

CMD=${1:-}
NAME=${2:-}

if [ -z "${CMD}" ] || [ -z "${NAME}" ]; then
    sed -n '3,30p' "$0"
    exit 2
fi

case "${NAME}" in
    maker|taker) ;;
    *)
        echo "error: instance name must be 'maker' or 'taker' (got '${NAME}')" >&2
        exit 2
        ;;
esac

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
INSTANCES_DIR=${BASECAMP_INSTANCES_DIR:-${REPO_ROOT}/.basecamp}
INSTANCE_DIR=${INSTANCES_DIR}/${NAME}
DATA_DIR=${INSTANCE_DIR}/data
HOME_DIR=${INSTANCE_DIR}/home
XDG_CONFIG=${INSTANCE_DIR}/xdg/config
XDG_CACHE=${INSTANCE_DIR}/xdg/cache
XDG_DATA=${INSTANCE_DIR}/xdg/data
# Keep this short — liblogos creates Unix sockets like
# `<runtime>/logos_token_<module>_<pid>` and macOS caps sun_path at 104 chars.
RUNTIME_DIR=/tmp/lbc-${NAME}
WALLET_DIR=${INSTANCE_DIR}/wallet
LOG_FILE=${INSTANCE_DIR}/basecamp.log
case "${NAME}" in
    maker) ENV_FILE=${REPO_ROOT}/.env ;;
    taker) ENV_FILE=${REPO_ROOT}/.env.taker ;;
esac

BASECAMP_FLAKE=${BASECAMP_FLAKE:-github:logos-co/logos-basecamp}
if [ -z "${BASECAMP_PACKAGE:-}" ]; then
    if [ "$(uname -s)" = "Darwin" ]; then
        BASECAMP_PACKAGE=bin-macos-app
    else
        BASECAMP_PACKAGE=portable
    fi
fi

resolve_basecamp_bin() {
    local store
    store=$(nix build --no-link --print-out-paths "${BASECAMP_FLAKE}#${BASECAMP_PACKAGE}" 2>/dev/null | tail -1)
    if [ -z "${store}" ]; then
        echo "error: failed to resolve ${BASECAMP_FLAKE}#${BASECAMP_PACKAGE}" >&2
        exit 1
    fi
    if [ -x "${store}/bin/LogosBasecamp" ]; then
        echo "${store}/bin/LogosBasecamp"
    elif [ -x "${store}/LogosBasecamp.app/Contents/MacOS/LogosBasecamp" ]; then
        echo "${store}/LogosBasecamp.app/Contents/MacOS/LogosBasecamp"
    else
        echo "error: no LogosBasecamp binary inside ${store}" >&2
        exit 1
    fi
}

resolve_lgx() {
    # $1 = flake ref of the lgx package, prints the .lgx file path
    local store
    store=$(nix build --no-link --print-out-paths "$1" 2>/dev/null | tail -1)
    if [ -z "${store}" ]; then
        echo "error: failed to resolve $1" >&2
        exit 1
    fi
    local lgx
    lgx=$(find "${store}" -maxdepth 1 -name "*.lgx" | head -1)
    if [ -z "${lgx}" ]; then
        echo "error: no .lgx file inside ${store}" >&2
        exit 1
    fi
    echo "${lgx}"
}

# Manual LGX install. We can't use the host `lgpm` because the published
# `logos-package-manager` `lgpm` CLI is non-portable (rejects packages whose
# variant directory is not `darwin-arm64-dev`), while the bundled
# `bin-macos-app` Basecamp's in-process PackageManagerLib is portable
# (only resolves the bare `darwin-arm64` variant). Since we install
# `#lgx-portable` packages (variant = `darwin-arm64`) for Basecamp, lgpm
# refuses with `Package does not contain variant for platform: darwin-arm64-dev`.
#
# Layout produced matches what `lgpm install` would have written for a
# matching-variant package:
#
#   <dest_dir>/<name>/
#     manifest.json         (verbatim from the LGX archive)
#     variant               (text file: <variant>)
#     <libs / dylibs / qml / ...>   (flattened from variants/<variant>/)
#
# That is the layout the embedded PackageManagerLib enumerates and the layout
# we already verified in the bundled `capability_module` / `package_manager`
# directories shipped inside `bin-macos-app`.
#
# $1 = path to .lgx file
# $2 = expected variant (e.g. darwin-arm64)
# $3 = destination root (modules dir for type=core, plugins dir for type=ui_qml)
extract_lgx_variant() {
    local lgx_path=$1
    local want_variant=$2
    local dest_root=$3

    local tmp
    tmp=$(mktemp -d -t lgx-extract.XXXXXX)
    trap "rm -rf '${tmp}'" RETURN

    if ! tar -xzf "${lgx_path}" -C "${tmp}" 2>/dev/null; then
        echo "error: not a gzipped tar (lgx): ${lgx_path}" >&2
        return 1
    fi

    if [ ! -f "${tmp}/manifest.json" ]; then
        echo "error: lgx missing manifest.json: ${lgx_path}" >&2
        return 1
    fi

    if [ ! -d "${tmp}/variants/${want_variant}" ]; then
        echo "error: lgx ${lgx_path} has no variants/${want_variant}; available:" >&2
        ls "${tmp}/variants" >&2 2>/dev/null || true
        return 1
    fi

    local name
    name=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("name",""))' "${tmp}/manifest.json")
    if [ -z "${name}" ]; then
        echo "error: manifest.json in ${lgx_path} has no 'name' field" >&2
        return 1
    fi

    local install_dir=${dest_root}/${name}
    rm -rf "${install_dir}"
    mkdir -p "${install_dir}"

    cp "${tmp}/manifest.json" "${install_dir}/manifest.json"
    cp -R "${tmp}/variants/${want_variant}/." "${install_dir}/"
    printf '%s' "${want_variant}" > "${install_dir}/variant"

    echo "    -> ${name} (${want_variant}) installed at ${install_dir}"
}

cmd_paths() {
    cat <<EOF
instance:        ${NAME}
instance_dir:    ${INSTANCE_DIR}
user_dir:        ${DATA_DIR}     (passed as --user-dir to Basecamp)
home_dir:        ${HOME_DIR}
xdg_config_home: ${XDG_CONFIG}
xdg_cache_home:  ${XDG_CACHE}
xdg_data_home:   ${XDG_DATA}
runtime_dir:     ${RUNTIME_DIR}  (short path; macOS sun_path limit)
wallet_dir:      ${WALLET_DIR}
modules_dir:     ${DATA_DIR}/modules
ui_plugins_dir:  ${DATA_DIR}/plugins
log_file:        ${LOG_FILE}
env_file:        ${ENV_FILE}
basecamp_flake:  ${BASECAMP_FLAKE}#${BASECAMP_PACKAGE}
EOF
}

cmd_init() {
    mkdir -p \
        "${DATA_DIR}/modules" "${DATA_DIR}/plugins" \
        "${HOME_DIR}" "${XDG_CONFIG}" "${XDG_CACHE}" "${XDG_DATA}" \
        "${WALLET_DIR}"

    # Use the `#lgx-portable` variant for every package. The bundled
    # `bin-macos-app` Basecamp is built WITH `LGPM_PORTABLE_BUILD` defined,
    # so its PackageManagerLib `platformVariantsToTry()` returns ONLY the
    # bare host string (`darwin-arm64`, no `-dev` suffix) when resolving
    # `manifest.json` `main` keys (see upstream
    # `logos-package-manager/src/package_manager_lib.cpp`). The default
    # `#lgx` output of every Logos module flake bundles a manifest keyed
    # by `darwin-arm64-dev`, so installing `#lgx` into a `bin-macos-app`
    # Basecamp causes `mainFilePath` to resolve empty in the registry —
    # the module is silently dropped from `logos_core_get_known_modules`
    # even though `lgpm list` happily lists it from the same dir, and
    # opening any UI plugin that depends on it logs
    # `Cannot load unknown module: <name>`. `#lgx-portable` is the
    # downstream-correct artifact for this Basecamp.
    echo "==> resolving LGX packages (portable variant for bin-macos-app)"
    DELIVERY_LGX=$(resolve_lgx "github:logos-co/logos-delivery-module/v0.1.1#lgx-portable")
    SWAP_LGX=$(cd "${REPO_ROOT}/swap-module" && resolve_lgx ".#lgx-portable")
    UI_LGX=$(cd "${REPO_ROOT}/swap-ui" && resolve_lgx ".#lgx-portable")

    echo "    delivery_module: ${DELIVERY_LGX}"
    echo "    swap-module:     ${SWAP_LGX}"
    echo "    swap-ui:         ${UI_LGX}"

    # Pick the host variant. `bin-macos-app` PackageManagerLib only resolves
    # the bare host triple — see comment block above on the LGPM_PORTABLE_BUILD
    # mismatch. If you ever need to run inside a non-portable Basecamp build,
    # rebuild the LGX with `#lgx` (default `-dev` flavor) instead.
    local want_variant
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64)  want_variant=darwin-arm64 ;;
        Darwin/x86_64) want_variant=darwin-amd64 ;;
        Linux/aarch64) want_variant=linux-arm64 ;;
        Linux/x86_64)  want_variant=linux-amd64 ;;
        *)
            echo "error: unsupported host $(uname -s)/$(uname -m)" >&2
            exit 1
            ;;
    esac

    echo "==> installing LGX (variant ${want_variant}) into ${DATA_DIR}"
    extract_lgx_variant "${DELIVERY_LGX}" "${want_variant}" "${DATA_DIR}/modules"
    extract_lgx_variant "${SWAP_LGX}"     "${want_variant}" "${DATA_DIR}/modules"
    extract_lgx_variant "${UI_LGX}"       "${want_variant}" "${DATA_DIR}/plugins"

    echo "==> ${NAME} ready. launch with: scripts/basecamp-instance.sh run ${NAME}"
}

cmd_run() {
    if [ ! -d "${DATA_DIR}/modules" ] || [ ! -d "${DATA_DIR}/plugins" ]; then
        echo "error: instance not initialized. run: scripts/basecamp-instance.sh init ${NAME}" >&2
        exit 1
    fi
    if [ ! -f "${ENV_FILE}" ] && [ "${BASECAMP_SKIP_ENV_CHECK:-0}" != "1" ]; then
        echo "error: ${ENV_FILE} is missing. Run 'make infra' first and leave it running so Basecamp can use the generated ${NAME} config." >&2
        exit 1
    fi

    # Recreate the short runtime dir on each launch so previous sockets are gone.
    rm -rf "${RUNTIME_DIR}"
    mkdir -p "${RUNTIME_DIR}"
    chmod 700 "${RUNTIME_DIR}"

    BIN=$(resolve_basecamp_bin)
    echo "==> launching ${NAME} (${BIN})"
    echo "    --user-dir:     ${DATA_DIR}"
    echo "    runtime dir:    ${RUNTIME_DIR}"
    echo "    env file:       ${ENV_FILE}"
    echo "    log:            ${LOG_FILE}"
    echo "    (Ctrl-C to stop)"

    # Run from REPO_ROOT so relative paths in .env / .env.taker resolve.
    cd "${REPO_ROOT}"

    env -i \
        PATH="/usr/bin:/bin:/usr/sbin:/sbin:${HOME}/.nix-profile/bin" \
        HOME="${HOME_DIR}" \
        XDG_CONFIG_HOME="${XDG_CONFIG}" \
        XDG_CACHE_HOME="${XDG_CACHE}" \
        XDG_DATA_HOME="${XDG_DATA}" \
        XDG_RUNTIME_DIR="${RUNTIME_DIR}" \
        TMPDIR="${RUNTIME_DIR}" \
        NSSA_WALLET_HOME_DIR="${WALLET_DIR}" \
        SWAP_UI_AUTO_ENV_FILE="${ENV_FILE}" \
        SWAP_UI_AUTO_ROLE="${NAME}" \
        QT_LOGGING_RULES="*.debug=false;qt.qpa.*=false" \
        "${BIN}" --user-dir "${DATA_DIR}" 2>&1 | tee "${LOG_FILE}"
}

cmd_clean() {
    if [ -d "${INSTANCE_DIR}" ]; then
        echo "==> removing ${INSTANCE_DIR}"
        rm -rf "${INSTANCE_DIR}"
    else
        echo "==> ${INSTANCE_DIR} does not exist; nothing to clean"
    fi
    if [ -d "${RUNTIME_DIR}" ]; then
        echo "==> removing ${RUNTIME_DIR}"
        rm -rf "${RUNTIME_DIR}"
    fi
}

case "${CMD}" in
    paths) cmd_paths ;;
    init)  cmd_init ;;
    run)   cmd_run ;;
    clean) cmd_clean ;;
    *)
        echo "error: unknown command '${CMD}' (expected init|run|clean|paths)" >&2
        exit 2
        ;;
esac
