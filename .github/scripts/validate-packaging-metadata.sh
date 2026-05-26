#!/usr/bin/env bash
# Validate AppStream metainfo.xml and .desktop file for Flathub submission.
set -euo pipefail

app_id="io.github.IllyaYalovyy.rttx"
data_dir="clients/rttx/data"

metainfo="${data_dir}/${app_id}.metainfo.xml"
desktop="${data_dir}/${app_id}.desktop"

errors=0

if command -v appstreamcli &>/dev/null; then
    echo "=== Validating ${metainfo} ==="
    if ! appstreamcli validate "${metainfo}"; then
        errors=$((errors + 1))
    fi
else
    echo "WARNING: appstreamcli not found, skipping metainfo validation"
fi

if command -v desktop-file-validate &>/dev/null; then
    echo "=== Validating ${desktop} ==="
    if ! desktop-file-validate "${desktop}"; then
        errors=$((errors + 1))
    fi
else
    echo "WARNING: desktop-file-validate not found, skipping desktop file validation"
fi

if [[ ${errors} -gt 0 ]]; then
    echo >&2 "Packaging metadata validation failed with ${errors} error(s)"
    exit 1
fi

echo "Packaging metadata validation passed"
