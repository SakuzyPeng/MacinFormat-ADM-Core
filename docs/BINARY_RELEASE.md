# mfadm 下载包与 GitHub 发布

当前发布版本为 `v0.2.0`。面向使用者的中英文发布说明保存在
[packaging/release-notes.md](../packaging/release-notes.md)，Rust 库上传步骤见
[crates.io 发布准备](CRATES_IO_RELEASE.md)。

## 下载包

| 平台 | Rust target | 格式 |
| --- | --- | --- |
| macOS Apple 芯片 | `aarch64-apple-darwin` | `.tar.gz` |
| Windows x64 | `x86_64-pc-windows-msvc` | `.zip` |
| Linux x64，GNU libc | `x86_64-unknown-linux-gnu` | `.tar.gz` |

文件名采用 `mfadm-v<版本>-<target>`。每个平台同时生成归档、SHA-256 文件和 JSON 构建记录。
归档包含可执行文件、中英文 README、格式参考、验证记录、MIT 许可证、依赖许可证和 ADM common definitions 的声明。
Windows 发布构建使用静态 CRT。

## 本地准备

使用 Rust 1.98 和 Python 3.11 或更新版本，在对应原生平台运行打包脚本。以下示例适用于 Apple 芯片 Mac。

```sh
python3 scripts/package_release.py --target aarch64-apple-darwin
```

输出位于被忽略的 `artifacts/release/`。脚本读取 Cargo 配置中的 target 目录，复用已有构建与全局缓存。
它验证 release 可执行文件的版本、帮助和 24-bit PCM 检查结果，并解包归档后再次执行相同验证。
解包后的可执行文件还会将仓库示例生成的合成 ADM 转为 DAMF 再转回 ADM，检查往返文件逐字节一致。
合成媒体使用临时目录，验证结束后清理。构建记录写明源码提交及工作树状态。

可用 `--output-dir` 指定输出目录。对已准备好的标签，增加 `--release-tag v0.2.0`，检查它与 workspace 版本一致。

## CI 准备

手动运行 **Prepare release** 工作流会执行三平台质量检查，构建和验证三个 CLI 下载包，
完成三个库的归档与发布演练，并将完整产物汇总到 `release-bundle` artifact。
可从 Actions 页面下载，或使用 GitHub CLI。

```sh
gh workflow run release.yml --ref main
gh run list --workflow release.yml
gh run download <run-id> --name release-bundle --dir artifacts/release
python3 scripts/release_assets.py --directory artifacts/release
```

汇总脚本核对三平台的版本、提交、工作树状态和校验和，检查三个 `.crate` 的版本与源码提交，生成 `SHA256SUMS`。
在检出相同源码提交的工作树中运行核验命令。

## 创建发布草稿

1. 更新 workspace 版本、内部依赖版本和中英文 README 的获取方式，同步发布说明中的版本、下载文件名与版本化链接。
2. 提交发布改动，确认三平台 CI 与 **Prepare release** 通过。
3. 在同一提交上按 [crates.io 发布步骤](CRATES_IO_RELEASE.md) 上传三个库，确认注册表依赖和 docs.rs 可用。
4. 在该提交上创建并推送版本标签。

   ```sh
   git tag -a v0.2.0 -m "MacinFormat ADM Core v0.2.0"
   git push origin v0.2.0
   ```

标签触发 **Prepare release**，全部检查通过后，工作流以版本化源码构建新的附件并创建 GitHub Release 草稿。
重跑可更新同一草稿的附件与说明。公开版本的后续更新使用新版本号。

在 Releases 页面检查发布说明、三个 CLI 下载包、三个库归档和 `SHA256SUMS`，然后发布草稿。
构建记录中的源码提交应与标签指向的提交一致。
