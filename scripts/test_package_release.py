"""覆盖归档、版本和跨平台产物汇总的发布回归。"""

import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from package_release import (
    CRATES, TARGETS, check_tag, sha256, third_party_licenses, write_archive,
)
from release_assets import create_draft, verify_assets


class PackageTests(unittest.TestCase):
    def test_tag_matches_entire_version(self):
        check_tag("", "0.1.0")
        check_tag("v0.1.0", "0.1.0")
        for tag in ["0.1.0", "v0.1.1", "v0.1.0-rc.1"]:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                check_tag(tag, "0.1.0")

    def test_archives_preserve_contents_and_executable_mode(self):
        files = [("mfadm/mfadm", b"executable", 0o755), ("mfadm/LICENSE", b"license", 0o644)]
        with tempfile.TemporaryDirectory() as temporary:
            for suffix in [".zip", ".tar.gz"]:
                with self.subTest(suffix=suffix):
                    path = Path(temporary) / f"mfadm{suffix}"
                    write_archive(path, files)
                    first_digest = sha256(path)
                    write_archive(path, files)
                    self.assertEqual(sha256(path), first_digest)
                    if suffix == ".zip":
                        with zipfile.ZipFile(path) as archive:
                            self.assertEqual(archive.read("mfadm/mfadm"), b"executable")
                            self.assertEqual(archive.getinfo("mfadm/mfadm").external_attr >> 16, 0o755)
                    else:
                        with tarfile.open(path) as archive:
                            self.assertEqual(archive.extractfile("mfadm/mfadm").read(), b"executable")
                            self.assertEqual(archive.getmember("mfadm/mfadm").mode, 0o755)

    def test_licenses_include_nested_license_directories(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "LICENSES").mkdir()
            (root / "LICENSES/MIT.txt").write_text(
                "Copyright example\nPermission granted\n", encoding="utf-8",
            )
            package = {
                "name": "example", "version": "1.0.0", "license": "MIT",
                "source": "registry", "manifest_path": str(root / "Cargo.toml"),
            }
            text = third_party_licenses({"packages": [package]}).decode("utf-8")
            self.assertIn("example 1.0.0", text)
            self.assertIn("Copyright example", text)


class ReleaseAssetsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.folder = Path(self.temporary.name)
        self.version = "0.1.0"
        self.commit = "a" * 40
        for target, suffix in TARGETS.items():
            base = f"mfadm-v{self.version}-{target}"
            archive = self.folder / f"{base}{suffix}"
            archive.write_bytes(target.encode())
            manifest = {
                "version": self.version,
                "target": target,
                "source_commit": self.commit,
                "working_tree_dirty": False,
                "archive": archive.name,
                "binary": "mfadm.exe" if target.endswith("windows-msvc") else "mfadm",
                "archive_sha256": sha256(archive),
            }
            (self.folder / f"{base}.json").write_text(json.dumps(manifest), encoding="utf-8")
            (self.folder / f"{archive.name}.sha256").write_text(
                f"{sha256(archive)}  {archive.name}\n", encoding="utf-8",
            )
        for name in CRATES:
            self.write_crate(name, self.commit)

    def write_crate(self, name, commit):
        prefix = f"{name}-{self.version}"
        manifest = f'[package]\nname = "{name}"\nversion = "{self.version}"\n'
        files = [
            (f"{prefix}/Cargo.toml", manifest.encode(), 0o644),
            (f"{prefix}/.cargo_vcs_info.json",
             json.dumps({"git": {"sha1": commit}}).encode(), 0o644),
        ]
        write_archive(self.folder / f"{prefix}.crate", files)

    def test_complete_bundle_has_checksums_for_every_asset(self):
        assets = verify_assets(self.folder, self.version, self.commit)
        self.assertEqual(len(assets), 13)
        lines = (self.folder / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(lines), 12)
        for line in lines:
            digest, name = line.split("  ", 1)
            self.assertEqual(digest, sha256(self.folder / name))

    def test_changed_archive_is_rejected(self):
        path = next(self.folder.glob("*.zip"))
        path.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            verify_assets(self.folder, self.version, self.commit)

    def test_crate_from_another_commit_is_rejected(self):
        self.write_crate("macin-adm", "b" * 40)
        with self.assertRaisesRegex(ValueError, "源码提交"):
            verify_assets(self.folder, self.version, self.commit)

    def test_dirty_platform_build_is_rejected(self):
        path = next(self.folder.glob("*.json"))
        manifest = json.loads(path.read_text(encoding="utf-8"))
        manifest["working_tree_dirty"] = True
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "源码信息"):
            verify_assets(self.folder, self.version, self.commit)

    def test_missing_platform_is_rejected(self):
        next(self.folder.glob("*.json")).unlink()
        with self.assertRaises(FileNotFoundError):
            verify_assets(self.folder, self.version, self.commit)


class DraftTests(unittest.TestCase):
    def test_new_release_is_created_as_draft(self):
        with patch("release_assets.subprocess.run") as run:
            run.return_value.returncode = 1
            create_draft("v0.1.0", [Path("bundle.zip")])
            arguments = run.call_args.args[0]
            self.assertEqual(arguments[:4], ["gh", "release", "create", "v0.1.0"])
            self.assertIn("--draft", arguments)
            self.assertIn("--verify-tag", arguments)

    def test_existing_draft_is_updated(self):
        with patch("release_assets.subprocess.run") as run:
            run.return_value.returncode = 0
            run.return_value.stdout = '{"isDraft": true}'
            create_draft("v0.1.0", [Path("bundle.zip")])
            self.assertEqual(run.call_args_list[1].args[0][:3], ["gh", "release", "upload"])
            self.assertEqual(run.call_args_list[2].args[0][:3], ["gh", "release", "edit"])

    def test_published_release_is_preserved(self):
        with patch("release_assets.subprocess.run") as run:
            run.return_value.returncode = 0
            run.return_value.stdout = '{"isDraft": false}'
            with self.assertRaisesRegex(ValueError, "已公开发布"):
                create_draft("v0.1.0", [Path("bundle.zip")])
            run.assert_called_once()


if __name__ == "__main__":
    unittest.main()
