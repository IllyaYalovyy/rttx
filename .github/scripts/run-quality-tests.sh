#!/usr/bin/env bash
set -euo pipefail

export GTK_A11Y="${GTK_A11Y:-none}"

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
bash "${script_dir}/ensure-sibling-rttxd.sh"

broadway_cmd=""
broadway_pid=""
readonly LIBRARY_SKIP_PATTERNS=(
    "window::tests::"
    "sidebar::tests::"
    "session::tests::apply_initial_paned_ratios_restores_nested_non_sentinel_positions"
)

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
    local expected_tests
    local completed_tests

    expected_tests=$(sed -n 's/^running \([0-9][0-9]*\) tests$/\1/p' "${logfile}" | tail -n 1)
    completed_tests=$(grep -Ec '^test .+ \.\.\. (ok|ignored)$' "${logfile}" || true)

    grep -q "signal: 11, SIGSEGV: invalid memory reference" "${logfile}" &&
        [[ -n "${expected_tests}" ]] &&
        [[ "${completed_tests}" -eq "${expected_tests}" ]] &&
        ! grep -q "test result: FAILED" "${logfile}" &&
        ! grep -q "^failures:$" "${logfile}"
}

run_logged_command() {
    local label=$1
    shift

    local logfile
    logfile=$(mktemp -t rttx-quality-tests-XXXXXX.log)

    echo "::group::${label}"

    set +e
    "$@" 2>&1 | tee "${logfile}"
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

list_isolated_library_tests() {
    cargo test --lib -- --list |
        awk -F': test' '
            /^(window::tests::|sidebar::tests::|session::tests::apply_initial_paned_ratios_restores_nested_non_sentinel_positions)/ {
                print $1
            }
        '
}

run_library_tests() {
    local stable_args=(
        cargo test --lib -- --nocapture
    )
    local isolated_test

    for skip_pattern in "${LIBRARY_SKIP_PATTERNS[@]}"; do
        stable_args+=(--skip "${skip_pattern}")
    done

    run_logged_command "Library tests (stable subset)" "${stable_args[@]}"

    while IFS= read -r isolated_test; do
        [[ -n "${isolated_test}" ]] || continue
        run_logged_command \
            "Library test ${isolated_test}" \
            cargo test --lib "${isolated_test}" -- --exact --nocapture
    done < <(list_isolated_library_tests)
}

trap cleanup EXIT

start_broadway_if_available

run_library_tests
run_logged_command "Binary tests" cargo test --bins -- --nocapture

while IFS= read -r integration_test; do
    run_logged_command "Integration test ${integration_test}" \
        cargo test --test "${integration_test}" -- --nocapture
done < <(find tests -maxdepth 1 -type f -name '*.rs' -printf '%f\n' | sed 's/\.rs$//' | sort)

run_logged_command "Doc tests" cargo test --doc -- --nocapture
