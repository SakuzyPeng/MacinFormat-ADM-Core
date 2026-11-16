"""构建、验证并打包 mfadm 的原生 release 下载包。"""

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import tomllib
import wave
import zipfile


ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-pc-windows-msvc": ".zip",
    "x86_64-unknown-linux-gnu": ".tar.gz",
}
CRATES = ("macin-adm", "macin-bwf", "macin-atmos")


def output(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def workspace_version():
    with (ROOT / "Cargo.toml").open("rb") as source:
        return tomllib.load(source)["workspace"]["package"]["version"]


def check_tag(tag, version):
    if tag and tag != f"v{version}":
        raise ValueError(f"标签 {tag} 与 workspace 版本 {version} 不一致")


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def third_party_licenses(metadata):
    sections = []
    packages = sorted(metadata["packages"], key=lambda item: (item["name"], item["version"]))
    for package in packages:
        if package["source"] is None:
            continue
        root = Path(package["manifest_path"]).parent
        files = set()
        for path in root.iterdir():
            prefixes = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE")
            if path.name.upper().startswith(prefixes):
                if path.is_dir():
                    files.update(child for child in path.rglob("*") if child.is_file())
                else:
                    files.add(path)
        if package.get("license_file"):
            files.add(root / package["license_file"])
        if not files:
            raise ValueError(f"{package['name']} {package['version']} 缺少许可证文件")
        sections.append(f"{package['name']} {package['version']} — {package['license']}\n")
        for path in sorted(files):
            label = path.relative_to(root).as_posix()
            sections.append(f"--- {label} ---\n{path.read_text(encoding='utf-8')}\n")
    return "\n".join(sections).encode("utf-8")


def smoke_test(binary, version):
    if output(str(binary), "--version") != f"mfadm {version}":
        raise ValueError("mfadm 二进制版本与 workspace 版本不一致")
    output(str(binary), "--help")
    output(str(binary), "convert", "--help")
    with tempfile.TemporaryDirectory(prefix="mfadm-smoke-") as temporary:
        fixture = Path(temporary) / "pcm.wav"
        with wave.open(str(fixture), "wb") as source:
            source.setparams((2, 3, 48000, 0, "NONE", "not compressed"))
            source.writeframes(bytes(4 * 2 * 3))
        report = json.loads(output(str(binary), "info", str(fixture), "--json"))
        expected = {"sample_rate": 48000, "channels": 2, "bits": 24, "frames": 4}
        if any(report[key] != value for key, value in expected.items()):
            raise ValueError("mfadm release 二进制的 PCM 检查结果不一致")


def write_archive(path, files):
    if path.suffix == ".zip":
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, payload, mode in files:
                info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = mode << 16
                archive.writestr(info, payload)
    else:
        with path.open("wb") as raw:
            with gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w") as archive:
                    for name, payload, mode in files:
                        info = tarfile.TarInfo(name)
                        info.size = len(payload)
                        info.mode = mode
                        archive.addfile(info, io.BytesIO(payload))


def verify_archive(path, binary_name, version, target):
    with tempfile.TemporaryDirectory(prefix="mfadm-unpack-") as temporary:
        binary = Path(temporary) / binary_name
        base = path.name.removesuffix(".zip").removesuffix(".tar.gz")
        member = f"{base}/{binary_name}"
        if path.suffix == ".zip":
            with zipfile.ZipFile(path) as archive:
                binary.write_bytes(archive.read(member))
        else:
            with tarfile.open(path) as archive:
                binary.write_bytes(archive.extractfile(member).read())
        binary.chmod(0o755)
        smoke_test(binary, version)
        source = Path(temporary) / "source.wav"
        output(
            "cargo", "run", "--locked", "--release", "-p", "macin-atmos",
            "--features", "read", "--example", "synthetic_master", "--target", target,
            "--", str(source), "standard",
        )
        damf = Path(temporary) / "damf"
        output(str(binary), "convert", str(source), "--to", f"home={damf}")
        manifest = next(damf.glob("*.atmos"))
        restored = Path(temporary) / "restored.wav"
        output(str(binary), "convert", str(manifest), "--to", f"standard={restored}")
        if restored.read_bytes() != source.read_bytes():
            raise ValueError("release 下载包的 ADM → DAMF → ADM 往返结果不一致")


def package(target, destination, tag):
    version = workspace_version()
    check_tag(tag, version)
    host = next(
        line.removeprefix("host: ")
        for line in output("rustc", "-vV").splitlines()
        if line.startswith("host: ")
    )
    if host != target:
        raise ValueError(f"需要在 {target} 原生主机运行下载包验证，当前主机为 {host}")
    subprocess.run(
        ["cargo", "build", "--locked", "--release", "-p", "macin-format",
         "--bin", "mfadm", "--target", target],
        cwd=ROOT, check=True,
    )
    metadata = json.loads(output(
        "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target,
    ))
    binary_name = "mfadm.exe" if target.endswith("windows-msvc") else "mfadm"
    binary = Path(metadata["target_directory"]) / target / "release" / binary_name
    smoke_test(binary, version)
    base = f"mfadm-v{version}-{target}"
    archive_name = base + TARGETS[target]
    manifest = {
        "version": version,
        "target": target,
        "source_commit": output("git", "rev-parse", "HEAD"),
        "working_tree_dirty": bool(output("git", "status", "--porcelain")),
        "rustc": output("rustc", "--version"),
        "binary": binary_name,
        "archive": archive_name,
    }
    files = [(f"{base}/{binary_name}", binary.read_bytes(), 0o755)]
    documents = [
        Path(name)
        for name in ("LICENSE", "README.md", "README.en.md", "AGENTS.md", "Cargo.lock")
    ]
    documents.extend(path.relative_to(ROOT) for path in sorted((ROOT / "docs").glob("*.md")))
    documents.extend(path.relative_to(ROOT) for path in sorted((ROOT / "docs").glob("*.json")))
    documents.append(Path("packaging/release-notes.md"))
    documents.extend(
        Path("crates/macin-adm/assets") / name
        for name in ("LICENSE", "NOTICE.txt", "manifest.json", "common_definitions.xml")
    )
    documents.append(Path("crates/macin-atmos/examples/synthetic_master.rs"))
    for relative in documents:
        files.append((f"{base}/{relative.as_posix()}", (ROOT / relative).read_bytes(), 0o644))
    files.append((f"{base}/THIRD_PARTY_LICENSES.txt", third_party_licenses(metadata), 0o644))
    build_info = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    files.append((f"{base}/BUILD_INFO.json", build_info, 0o644))
    destination.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination, prefix="package-") as temporary:
        folder = Path(temporary)
        archive = folder / archive_name
        write_archive(archive, files)
        verify_archive(archive, binary_name, version, target)
        manifest["archive_sha256"] = sha256(archive)
        (folder / f"{archive_name}.sha256").write_text(
            f"{manifest['archive_sha256']}  {archive_name}\n", encoding="utf-8",
        )
        (folder / f"{base}.json").write_text(
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8",
        )
        for path in folder.iterdir():
            path.replace(destination / path.name)
    return destination / archive_name


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "artifacts/release")
    parser.add_argument("--release-tag", default="")
    arguments = parser.parse_args()
    print(package(arguments.target, arguments.output_dir.resolve(), arguments.release_tag))
