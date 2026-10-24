#!/usr/bin/env bash
# Materialize the shared release secret on an ephemeral Actions runner.
set -euo pipefail
project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

if [[ -z "${FLECTAR_ANDROID_KEYSTORE_BASE64:-}" ]]; then
  printf 'FLECTAR_ANDROID_KEYSTORE_BASE64 is required in the android-release environment. Restore the original app signing key.\n' >&2
  exit 1
fi
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"
umask 077
keystore="$(mktemp "$RUNNER_TEMP/flectar-app-signing.XXXXXX.keystore")"
trap 'rm -f "$keystore"' EXIT
printf '%s' "$FLECTAR_ANDROID_KEYSTORE_BASE64" | base64 --decode > "$keystore"
export FLECTAR_ANDROID_KEYSTORE="$keystore"
bash "$project_dir/scripts/verify-android-keystore.sh"
printf 'FLECTAR_ANDROID_KEYSTORE=%s\n' "$keystore" >> "$GITHUB_ENV"
trap - EXIT
