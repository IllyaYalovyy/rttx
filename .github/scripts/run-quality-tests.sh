#!/usr/bin/env bash
set -euo pipefail

export GTK_A11Y="${GTK_A11Y:-none}"

broadway_cmd=""
broadway_pid=""

cleanup() {
    if [[ -n "${broadway_pid}" ]]; then
        kill "${broadway_pid}" 2>/dev/null || true
        wait "${broadway_pid}" 2>/dev/null || true
    fi
}

start_broadway_if_available() {
    if command -v gtk4-broadwayd >/dev/null 2>&1; then
        broadway_cmd="gtk4-broadwayd"
    elif command -v gtkbroadwayd >/dev/null 2>&1; then
        broadway_cmd="gtkbroadwayd"
    else
        echo "Broadway server not found; continuing without an explicit GTK display server." >&2
        return
    fi

    export GDK_BACKEND="${GDK_BACKEND:-broadway}"
    export BROADWAY_DISPLAY="${BROADWAY_DISPLAY:-:5}"

    "${broadway_cmd}" "${BROADWAY_DISPLAY#:}" >/tmp/rttx-broadway.log 2>&1 &
    broadway_pid=$!
    sleep 1
}

known_teardown_sigsegv() {
    local logfile=$1

    grep -q "signal: 11, SIGSEGV: invalid memory reference" "${logfile}" &&
        grep -q "test result: ok\\." "${logfile}" &&
        ! grep -q "test result: FAILED" "${logfile}" &&
        ! grep -q "^failures:$" "${logfile}"
}

run_cargo_target() {
    local label=$1
    shift

    local logfile
    logfile=$(mktemp -t rttx-quality-tests-XXXXXX.log)

    echo "::group::${label}"

    set +e
    cargo test "$@" -- --nocapture 2>&1 | tee "${logfile}"
    local status=${PIPESTATUS[0]}
    set -e

    if [[ ${status} -eq 0 ]]; then
        echo "::endgroup::"
        rm -f "${logfile}"
        return 0
    fi

    if known_teardown_sigsegv "${logfile}"; then
        echo "Allowing known GTK teardown SIGSEGV after passing ${label} tests." >&2
        echo "::endgroup::"
        rm -f "${logfile}"
        return 0
    fi

    echo "::endgroup::"
    return "${status}"
}

trap cleanup EXIT

start_broadway_if_available

run_cargo_target "Library tests" --lib
run_cargo_target "Binary tests" --bins

while IFS= read -r integration_test; do
    run_cargo_target "Integration test ${integration_test}" --test "${integration_test}"
done < <(find tests -maxdepth 1 -type f -name '*.rs' -printf '%f\n' | sed 's/\.rs$//' | sort)

run_cargo_target "Doc tests" --doc
