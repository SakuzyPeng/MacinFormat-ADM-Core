"""核验三个平台的 CLI 与三个 crate 归档，汇总校验和并按需创建发布草稿。"""

import argparse
import json
from pathlib import Path
import subprocess
import tarfile
import tomllib

from package_release import CRATES, ROOT, TARGETS, check_tag, output, sha256, workspace_version


def verify_assets(folder, version, commit):
    assets = []
    for target, suffix in TARGETS.items():
        base = f"mfadm-v{version}-{target}"
        manifest_path = folder / f"{base}.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        archive = folder / f"{base}{suffix}"
        expected = {
            "version": version,
            "target": target,
            "source_commit": commit,
            "working_tree_dirty": False,
            "archive": archive.name,
            "binary": "mfadm.exe" if target.endswith("windows-msvc") else "mfadm",
        }
        if any(manifest.get(key) != value for key, value in expected.items()):
            raise ValueError(f"{manifest_path.name} 的版本、平台或源码信息不一致")
        digest = sha256(archive)
        checksum = folder / f"{archive.name}.sha256"
        if manifest.get("archive_sha256") != digest:
            raise ValueError(f"{archive.name} 的 SHA-256 不一致")
        if checksum.read_text(encoding="utf-8").strip() != f"{digest}  {archive.name}":
            raise ValueError(f"{checksum.name} 内容不一致")
        assets.extend([archive, checksum, manifest_path])
    for name in CRATES:
        path = folder / f"{name}-{version}.crate"
        prefix = f"{name}-{version}"
        with tarfile.open(path) as archive:
            provenance = json.load(archive.extractfile(f"{prefix}/.cargo_vcs_info.json"))
            manifest_text = archive.extractfile(f"{prefix}/Cargo.toml").read().decode("utf-8")
            manifest = tomllib.loads(manifest_text)
        if provenance["git"]["sha1"] != commit or provenance["git"].get("dirty", False):
            raise ValueError(f"{path.name} 的源码提交不一致或工作树有改动")
        if manifest["package"]["name"] != name or manifest["package"]["version"] != version:
            raise ValueError(f"{path.name} 的 crate 名称或版本不一致")
        assets.append(path)
    checksums = folder / "SHA256SUMS"
    checksums.write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in sorted(assets)), encoding="utf-8",
    )
    return [*assets, checksums]


def create_draft(tag, assets):
    title = f"MacinFormat ADM Core {tag}"
    notes = ROOT / "packaging/release-notes.md"
    existing = subprocess.run(
        ["gh", "release", "view", tag, "--json", "isDraft"],
        cwd=ROOT, capture_output=True, text=True, encoding="utf-8",
    )
    if existing.returncode == 0:
        if not json.loads(existing.stdout)["isDraft"]:
            raise ValueError(f"{tag} 已公开发布，请使用新的版本准备发布")
        subprocess.run(
            ["gh", "release", "upload", tag, "--clobber", *map(str, assets)],
            cwd=ROOT, check=True,
        )
        subprocess.run(
            ["gh", "release", "edit", tag, "--title", title, "--notes-file", str(notes)],
            cwd=ROOT, check=True,
        )
    else:
        subprocess.run(
            ["gh", "release", "create", tag, "--verify-tag", "--draft", "--title", title,
             "--notes-file", str(notes), *map(str, assets)],
            cwd=ROOT, check=True,
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--tag", default="")
    parser.add_argument("--draft", action="store_true")
    arguments = parser.parse_args()
    version = workspace_version()
    tag = arguments.tag or f"v{version}"
    check_tag(tag, version)
    commit = output("git", "rev-parse", "HEAD")
    assets = verify_assets(arguments.directory.resolve(), version, commit)
    if arguments.draft:
        create_draft(tag, assets)
    print(f"已核验 {len(assets)} 个发布附件")
