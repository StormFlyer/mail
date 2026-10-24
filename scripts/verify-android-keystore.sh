#!/usr/bin/env bash
# Check the single app signing identity used by every published Android APK.
set -euo pipefail

project_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
for name in FLECTAR_ANDROID_KEYSTORE FLECTAR_ANDROID_KEYSTORE_PASSWORD FLECTAR_ANDROID_KEY_ALIAS FLECTAR_ANDROID_KEY_PASSWORD; do
  if [[ -z "${!name:-}" ]]; then
    printf 'Android signing requires %s. Restore the original app signing key and its credentials; never generate a replacement release key.\n' "$name" >&2
    exit 1
  fi
done
if [[ ! -f "$FLECTAR_ANDROID_KEYSTORE" ]]; then
  printf 'The configured Android app signing keystore does not exist.\n' >&2
  exit 1
fi

# keytool silently ignores a different -keypass for PKCS12, while Gradle's
# KeyStore.getKey does not. Reject inconsistent credentials before compilation.
if [[ "$FLECTAR_ANDROID_KEYSTORE_PASSWORD" != "$FLECTAR_ANDROID_KEY_PASSWORD" ]]; then
  details="$(keytool -list -J-Duser.language=en -J-Duser.country=US \
    -keystore "$FLECTAR_ANDROID_KEYSTORE" \
    -storepass:env FLECTAR_ANDROID_KEYSTORE_PASSWORD)"
  if [[ "$details" == *"Keystore type: PKCS12"* ]]; then
    printf 'PKCS12 requires matching keystore and private-key passwords.\n' >&2
    exit 1
  fi
fi

expected="$(cat "$project_dir/platform/android/app-signing-certificate.sha256")"
if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
  printf 'The pinned Android app signing certificate SHA-256 is invalid.\n' >&2
  exit 1
fi
actual="$(keytool -exportcert -keystore "$FLECTAR_ANDROID_KEYSTORE" \
  -alias "$FLECTAR_ANDROID_KEY_ALIAS" \
  -storepass:env FLECTAR_ANDROID_KEYSTORE_PASSWORD | sha256sum)"
if [[ "${actual%% *}" != "$expected" ]]; then
  printf 'Android signing key differs from the pinned app identity. Alpha, beta, and stable must use the original key; do not replace the fingerprint.\n' >&2
  exit 1
fi
# Exporting a certificate alone also succeeds for certificate-only entries.
# A discarded CSR proves that the private key and its password are usable.
keytool -certreq -keystore "$FLECTAR_ANDROID_KEYSTORE" \
  -alias "$FLECTAR_ANDROID_KEY_ALIAS" \
  -storepass:env FLECTAR_ANDROID_KEYSTORE_PASSWORD \
  -keypass:env FLECTAR_ANDROID_KEY_PASSWORD >/dev/null
printf '%s\n' "$expected"
