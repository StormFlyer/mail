#!/usr/bin/env python3
"""Exercise real keystore migration, private-key access, and secret delivery."""

import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("configure_signing", SCRIPTS / "configure-android-signing.py")
configure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(configure)


@unittest.skipUnless(shutil.which("keytool"), "A JDK is required for real signing tests")
class KeystoreTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copyfile(SCRIPTS / "verify-android-keystore.sh", scripts / "verify-android-keystore.sh")
        patcher = patch.object(configure, "SCRIPTS", scripts)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.source = self.root / "original.p12"
        self.alias = "original-key"
        self.password = "original fixture password"
        self.new_password = "new fixture password with spaces $()"
        self.environment = {**os.environ, "FIXTURE_PASSWORD": self.password}
        self.keytool(
            "-genkeypair", "-noprompt", "-keystore", str(self.source),
            "-storetype", "PKCS12", "-storepass:env", "FIXTURE_PASSWORD",
            "-keypass:env", "FIXTURE_PASSWORD", "-alias", self.alias,
            "-dname", "CN=Flectar Mail Test,O=Android,C=US", "-keyalg", "RSA",
            "-keysize", "2048", "-validity", "10000",
        )
        self.certificate = self.keytool(
            "-exportcert", "-keystore", str(self.source), "-alias", self.alias,
            "-storepass:env", "FIXTURE_PASSWORD",
        )
        self.pin = self.root / "platform/android/app-signing-certificate.sha256"
        self.pin.parent.mkdir(parents=True)
        self.pin.write_text(hashlib.sha256(self.certificate).hexdigest() + "\n")

    def keytool(self, *arguments):
        return subprocess.run(
            ["keytool", *arguments], env=self.environment,
            check=True, capture_output=True,
        ).stdout

    def test_password_migration_preserves_certificate_and_private_key(self):
        original_bytes = self.source.read_bytes()
        destination = self.root / "protected.p12"
        result = configure.reprotect(self.source, destination, self.alias,
                                     self.password, self.password, self.new_password)
        self.assertEqual(self.source.read_bytes(), original_bytes)
        self.assertEqual(destination.stat().st_mode & 0o777, 0o600)
        self.assertEqual(result["FLECTAR_ANDROID_KEYSTORE_PASSWORD"], self.new_password)
        actual = subprocess.check_output([
            "keytool", "-exportcert", "-keystore", str(destination), "-alias", self.alias,
            "-storepass:env", "FLECTAR_ANDROID_KEYSTORE_PASSWORD",
        ], env=result)
        self.assertEqual(actual, self.certificate)
        # Verification includes signing a CSR, so a certificate-only copy or
        # incorrect private-key password cannot masquerade as a working backup.
        with self.assertRaises(subprocess.CalledProcessError):
            configure.verify(destination, self.alias, self.password, self.password)
        with self.assertRaises(subprocess.CalledProcessError):
            configure.verify(destination, self.alias, self.new_password, "wrong private-key password")
        with self.assertRaises(ValueError):
            configure.reprotect(self.source, destination, self.alias,
                                self.password, self.password, self.new_password)

    def test_replacement_key_and_certificate_only_entries_are_rejected(self):
        self.pin.write_text("00" * 32 + "\n")
        with self.assertRaises(subprocess.CalledProcessError):
            configure.verify(self.source, self.alias, self.password, self.password)
        self.pin.write_text(hashlib.sha256(self.certificate).hexdigest() + "\n")
        certificate = self.root / "public.der"
        certificate.write_bytes(self.certificate)
        public_store = self.root / "public-only.p12"
        self.keytool(
            "-importcert", "-noprompt", "-keystore", str(public_store),
            "-storetype", "PKCS12", "-storepass:env", "FIXTURE_PASSWORD",
            "-alias", self.alias, "-file", str(certificate),
        )
        with self.assertRaises(subprocess.CalledProcessError):
            configure.verify(public_store, self.alias, self.password, self.password)


class GitHubApiTests(unittest.TestCase):
    def test_paginated_json_supports_older_github_cli(self):
        pages = [
            {"environments": [{"name": "another-environment"}]},
            {"environments": [{"name": "android-release"}]},
        ]
        for selected in (pages[:1], pages):
            for separator in ("", "\n"):
                with self.subTest(pages=len(selected), separator=separator):
                    output = "\n " + separator.join(json.dumps(page, indent=2) for page in selected) + "\n"
                    with patch.object(configure.subprocess, "run") as run:
                        run.return_value.stdout = output
                        actual = configure.github_json(["repos/owner/repo/environments", "--paginate"])
                    self.assertEqual(actual, selected)
                    self.assertEqual(run.call_args.args[0], [
                        "gh", "api", "repos/owner/repo/environments", "--paginate",
                    ])

    def test_invalid_later_page_is_not_ignored(self):
        with patch.object(configure.subprocess, "run") as run:
            run.return_value.stdout = '{"environments": []}\n{"environments":'
            with self.assertRaises(json.JSONDecodeError):
                configure.github_json(["repos/owner/repo/environments", "--paginate"])

    def test_nonpaginated_json_stays_an_object(self):
        with patch.object(configure.subprocess, "run") as run:
            run.return_value.stdout = '{"default_branch": "main"}'
            self.assertEqual(configure.github_json(["repos/owner/repo"]), {"default_branch": "main"})


class SecretUploadTests(unittest.TestCase):
    def test_upload_scopes_all_secrets_and_uses_stdin(self):
        with tempfile.TemporaryDirectory() as directory:
            keystore = Path(directory) / "fixture.p12"
            keystore.write_bytes(b"fixture private key")
            environment = {
                "FLECTAR_ANDROID_KEYSTORE": str(keystore),
                "FLECTAR_ANDROID_KEYSTORE_PASSWORD": "secret store password",
                "FLECTAR_ANDROID_KEY_ALIAS": "fixture-alias",
                "FLECTAR_ANDROID_KEY_PASSWORD": "secret key password",
            }
            responses = [
                [{"environments": []}], {}, {"default_branch": "main"}, {}, {},
            ]
            with patch.object(configure, "github_json", side_effect=responses) as github:
                with patch.object(configure.subprocess, "run") as run:
                    configure.upload(environment, "owner/repo", "android-release")
            self.assertEqual(github.call_args_list[0].args[0], [
                "repos/owner/repo/environments", "--paginate",
            ])
            self.assertEqual(github.call_args_list[3].args[1], {"name": "main", "type": "branch"})
            self.assertEqual(github.call_args_list[4].args[1], {"name": "v*", "type": "tag"})
            self.assertEqual(run.call_count, 4)
            values = {}
            for call in run.call_args_list:
                command = call.args[0]
                self.assertEqual(command[:3], ["gh", "secret", "set"])
                self.assertEqual(command[4:], ["--repo", "owner/repo", "--env", "android-release"])
                self.assertNotIn(call.kwargs["input"], command)
                values[command[3]] = call.kwargs["input"]
            self.assertEqual(base64.b64decode(values["FLECTAR_ANDROID_KEYSTORE_BASE64"]), keystore.read_bytes())
            for key in ("FLECTAR_ANDROID_KEYSTORE_PASSWORD", "FLECTAR_ANDROID_KEY_ALIAS", "FLECTAR_ANDROID_KEY_PASSWORD"):
                self.assertEqual(values[key], environment[key])

    def test_upload_refuses_an_unrestricted_existing_environment(self):
        with patch.object(configure, "github_json", side_effect=[
            [{"environments": [{"name": "android-release"}]}], {"deployment_branch_policy": None},
        ]):
            with patch.object(configure.subprocess, "run") as run, self.assertRaises(ValueError):
                configure.upload({}, "owner/repo", "android-release")
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
