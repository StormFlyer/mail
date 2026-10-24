#!/usr/bin/env python3
"""Preserve the app signing key, change its password, and optionally configure CI."""

import argparse
import base64
import getpass
import json
import os
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import quote

SCRIPTS = Path(__file__).resolve().parent


def verify(keystore: Path, alias: str, store_password: str, key_password: str) -> dict[str, str]:
    environment = {
        **os.environ,
        "FLECTAR_ANDROID_KEYSTORE": str(keystore),
        "FLECTAR_ANDROID_KEY_ALIAS": alias,
        "FLECTAR_ANDROID_KEYSTORE_PASSWORD": store_password,
        "FLECTAR_ANDROID_KEY_PASSWORD": key_password,
    }
    subprocess.run(
        ["bash", str(SCRIPTS / "verify-android-keystore.sh")],
        env=environment, check=True, stdout=subprocess.DEVNULL,
    )
    return environment


def reprotect(source: Path, destination: Path, alias: str,
              store_password: str, key_password: str, new_password: str) -> dict[str, str]:
    """Re-encrypt the existing private key and certificate without replacing either."""
    if len(new_password) < 16:
        raise ValueError("Use a new keystore password of at least 16 characters.")
    if destination.exists() or destination.is_symlink():
        raise ValueError("The destination already exists; refusing to overwrite a signing key.")
    environment = verify(source, alias, store_password, key_password)
    environment["FLECTAR_ANDROID_NEW_PASSWORD"] = new_password
    destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".android-signing-", dir=destination.parent) as directory:
        staged = Path(directory) / "app-signing.p12"
        subprocess.run([
            "keytool", "-importkeystore", "-noprompt",
            "-srckeystore", str(source), "-srcalias", alias,
            "-srcstorepass:env", "FLECTAR_ANDROID_KEYSTORE_PASSWORD",
            "-srckeypass:env", "FLECTAR_ANDROID_KEY_PASSWORD",
            "-destkeystore", str(staged), "-deststoretype", "PKCS12",
            "-destalias", alias,
            "-deststorepass:env", "FLECTAR_ANDROID_NEW_PASSWORD",
            "-destkeypass:env", "FLECTAR_ANDROID_NEW_PASSWORD",
        ], env=environment, check=True, stdout=subprocess.DEVNULL)
        verify(staged, alias, new_password, new_password)
        staged.chmod(0o600)
        # Publish the verified file without ever overwriting an existing path.
        os.link(staged, destination)
    return verify(destination, alias, new_password, new_password)


def github_json(arguments: list[str], body: dict | None = None):
    command = ["gh", "api", *arguments]
    if body is not None:
        command += ["--input", "-"]
    result = subprocess.run(
        command, input=json.dumps(body) if body is not None else None,
        text=True, check=True, capture_output=True,
    )
    if "--paginate" not in arguments:
        return json.loads(result.stdout)
    # Older gh releases support --paginate but not --slurp. Each page is a
    # separate JSON value and may span multiple lines, so decode the stream.
    pages = []
    decoder = json.JSONDecoder()
    remaining = result.stdout.lstrip()
    while remaining:
        page, end = decoder.raw_decode(remaining)
        pages.append(page)
        remaining = remaining[end:].lstrip()
    return pages


def upload(environment: dict[str, str], repository: str, name: str) -> None:
    # Creating an environment alone does not restrict which refs may use it.
    # Start a new one closed to all refs, then allow main and release tags.
    endpoint = f"repos/{repository}/environments/{quote(name, safe='')}"
    pages = github_json([f"repos/{repository}/environments", "--paginate"])
    if not any(item["name"] == name for page in pages for item in page["environments"]):
        github_json(["--method", "PUT", endpoint], {
            "deployment_branch_policy": {"protected_branches": False, "custom_branch_policies": True},
        })
        branch = github_json([f"repos/{repository}"])["default_branch"]
        for pattern, kind in ((branch, "branch"), ("v*", "tag")):
            github_json(["--method", "POST", f"{endpoint}/deployment-branch-policies"],
                        {"name": pattern, "type": kind})
    else:
        settings = github_json([endpoint])
        if not settings.get("deployment_branch_policy"):
            raise ValueError(f"Restrict branches/tags for the {name} environment before uploading signing secrets.")
    secrets = {
        "FLECTAR_ANDROID_KEYSTORE_BASE64": base64.b64encode(
            Path(environment["FLECTAR_ANDROID_KEYSTORE"]).read_bytes()
        ).decode("ascii"),
        **{key: environment[key] for key in (
            "FLECTAR_ANDROID_KEYSTORE_PASSWORD", "FLECTAR_ANDROID_KEY_ALIAS", "FLECTAR_ANDROID_KEY_PASSWORD",
        )},
    }
    for key, value in secrets.items():
        # gh encrypts locally. Values go over stdin, never argv or terminal output.
        subprocess.run(["gh", "secret", "set", key, "--repo", repository, "--env", name],
                       input=value, text=True, check=True, stdout=subprocess.DEVNULL)
    print(f"Configured the four signing secrets in {repository} / {name}.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("keystore", type=Path, help="The existing keystore matching the committed certificate")
    parser.add_argument("--output", type=Path, help="Write a copy encrypted with a new password")
    parser.add_argument("--alias", default="androiddebugkey", help="Existing private-key alias")
    parser.add_argument("--upload", action="store_true", help="Store the verified key and credentials in GitHub Actions")
    parser.add_argument("--repo", default="flectar/mail")
    parser.add_argument("--environment", default="android-release")
    args = parser.parse_args()
    source = args.keystore.expanduser().resolve(strict=True)
    if source.is_relative_to(SCRIPTS.parent):
        raise ValueError("Keep private signing keys outside the repository.")
    if args.output:
        destination = args.output.expanduser().absolute()
        if destination.resolve().is_relative_to(SCRIPTS.parent):
            raise ValueError("Keep the re-encrypted signing key outside the repository.")
        if destination.exists() or destination.is_symlink():
            raise ValueError("The output already exists. To upload it, pass it as the keystore without --output.")
    store_password = getpass.getpass("Current keystore password: ")
    key_password = getpass.getpass("Current key password (Enter to use the keystore password): ") or store_password
    if args.output:
        new_password = getpass.getpass("New password (at least 16 characters; save it in your password manager): ")
        if new_password != getpass.getpass("Confirm new password: "):
            raise ValueError("The new passwords do not match.")
        environment = reprotect(source, destination, args.alias, store_password, key_password, new_password)
        print(f"Saved {destination}; the private key and certificate are unchanged.")
    else:
        environment = verify(source, args.alias, store_password, key_password)
    if args.upload:
        if min(len(environment[key]) for key in (
            "FLECTAR_ANDROID_KEYSTORE_PASSWORD", "FLECTAR_ANDROID_KEY_PASSWORD"
        )) < 16:
            raise ValueError("Use --output to protect the existing key with a password of at least 16 characters first.")
        upload(environment, args.repo, args.environment)
    print("Keep the keystore and its password in your secure backup. Do not generate a replacement key.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
