## 简体中文

MacinFormat ADM Core **v0.1.0** 首次发布三个 Rust 库与命令行工具 **mfadm**，提供 ADM 元数据编辑、
WAVE 容器读写，以及 Atmos ADM BWF / DAMF 母版写入与转换。

### 获取与安装

在本页下方的 **Assets** 中选择适合电脑的 CLI 下载包，解压后在终端运行。

| 你的电脑 | 下载哪个文件 |
| --- | --- |
| Apple 芯片 Mac | `mfadm-v0.1.0-aarch64-apple-darwin.tar.gz` |
| Windows，Intel / AMD 64 位电脑 | `mfadm-v0.1.0-x86_64-pc-windows-msvc.zip` |
| Linux，Intel / AMD 64 位电脑 | `mfadm-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` |

macOS 和 Linux 解压后运行 `./mfadm --help`，Windows PowerShell 运行 `.\mfadm.exe --help`。
也可将可执行文件所在目录加入 `PATH`，随后使用 `mfadm` 命令。
Linux 下载包在 Ubuntu 24.04 构建与验证，使用 GNU libc。

下载包包含中英文说明、格式参考、许可证和构建信息。`.sha256` 与 `SHA256SUMS` 用于核对文件完整性，
`.json` 记录版本、平台和源码提交。

Rust 项目可通过 Cargo 集成三个库，要求 Rust 1.98 或更新版本。

```toml
[dependencies]
macin-adm = "=0.1.0"
macin-bwf = "=0.1.0"
macin-atmos = { version = "=0.1.0", features = ["read"] }
```

`macin-atmos` 的 `read` feature 启用 ADM / DAMF 母版读取与转换，默认配置提供母版写入。
三个 `.crate` 附件提供相应库的源码归档。

### 本次发布

- **ADM 元数据**支持解析 AXML、解析引用、生成 Objects / DirectSpeakers / HOA 快照，以及保留原文的语义回写。
- **WAVE 容器**支持 RIFF / RF64 / BW64、PCM 流式读写、帧定位及 `chna` / `bext` 等 chunk 的读取与编辑。
- **四种母版配置**提供 Standard BW64、Logic RF64、DAMF Home 0.5.1 和 DAMF 3DoF 0.6.0。
  支持固定 7.1.2 bed、最多 118 个对象、48 / 96 kHz、24-bit PCM 和按整数采样时间更新的对象元数据。
- **读取与转换**支持 ADM 与 DAMF 互转、替换 WAV 音频，以及通过 `MultiWriter` 从同一输入一次写出多种配置。
  ADM 母版读取逐块校验和投影，DAMF 读取使用两遍扫描。
- **mfadm 命令**提供 `chunks`、`info`、`adm`、`axml` 和 `convert`，检查与转换结果可输出为 JSON。
- **互操作验证**覆盖 EBU EAR、Conversion Tool、atmos_info，并记录了 Logic Pro 与 Dolby Atmos Renderer 的实机兼容结果。

### 第一次使用

```sh
mfadm info input.wav --json
mfadm adm input.wav --json
mfadm convert input.wav --to home=output-damf
mfadm convert input.atmos --to standard=output.wav
```

完整示例、格式规则与兼容性记录见对应版本文档。
[中文上手说明](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.1.0/README.md) ·
[格式参考](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.1.0/docs/REFERENCE.md) ·
[验证记录](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.1.0/docs/VALIDATION.md)

### 支持范围

- Atmos 母版读取支持格式参考列出的子集，包含一个 programme、至多一个完整 7.1.2 bed，以及单轨 Cartesian 对象。
- 默认采用 `LossPolicy::Reject`。显式选择 `Allow` 或 `--allow-loss` 后，转换会记录降级诊断。
- DAMF 3DoF 0.6.0 需要支持该版本的工具，Conversion Tool 2.1.2 的 DAMF 版本上限为 0.5。

反馈问题时请提供系统版本、执行命令和错误信息。
[提交 Issue](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/issues)

## English

MacinFormat ADM Core **v0.1.0** is the first release of three Rust libraries and the **mfadm** command-line
tool, covering ADM metadata editing, WAVE containers, and Atmos ADM BWF / DAMF master writing and conversion.

### Download and install

Choose a CLI archive from **Assets** below, extract it and run the executable in a terminal.

| Your computer | File to download |
| --- | --- |
| Apple silicon Mac | `mfadm-v0.1.0-aarch64-apple-darwin.tar.gz` |
| Windows, 64-bit Intel / AMD PC | `mfadm-v0.1.0-x86_64-pc-windows-msvc.zip` |
| Linux, 64-bit Intel / AMD PC | `mfadm-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` |

On macOS and Linux, run `./mfadm --help` after extraction. In Windows PowerShell, run `.\mfadm.exe --help`.
Adding the executable's directory to `PATH` makes the `mfadm` command available from other directories.
The Linux archive is built and verified on Ubuntu 24.04 and uses GNU libc.

Archives include Chinese and English documentation, the format reference, licenses and build information.
Use `.sha256` or `SHA256SUMS` to check file integrity. The `.json` files record the version, target and source commit.

Rust projects can use the libraries through Cargo with Rust 1.98 or newer.

```toml
[dependencies]
macin-adm = "=0.1.0"
macin-bwf = "=0.1.0"
macin-atmos = { version = "=0.1.0", features = ["read"] }
```

The `macin-atmos/read` feature enables ADM / DAMF master reading and conversion. The default configuration
provides master writing. The three `.crate` attachments contain the corresponding library sources.

### Included in this release

- **ADM metadata** provides AXML parsing, reference resolution, Objects / DirectSpeakers / HOA snapshots,
  generation and source-preserving semantic edits.
- **WAVE containers** support RIFF / RF64 / BW64, streaming PCM I/O, frame seeking, and reading or editing
  chunks including `chna` and `bext`.
- **Four master profiles** provide Standard BW64, Logic RF64, DAMF Home 0.5.1 and DAMF 3DoF 0.6.0, with a
  fixed 7.1.2 bed, up to 118 objects, 48 / 96 kHz, 24-bit PCM and object updates timed in integer samples.
- **Reading and conversion** cover ADM / DAMF conversion, replacement WAV audio, and multiple outputs from
  one input through `MultiWriter`. ADM masters are validated and projected block by block, and DAMF reading uses two passes.
- **The mfadm command** provides `chunks`, `info`, `adm`, `axml` and `convert`, with JSON inspection and conversion reports.
- **Interoperability validation** covers EBU EAR, Conversion Tool and atmos_info, with recorded device-side
  compatibility results for Logic Pro and Dolby Atmos Renderer.

### First use

```sh
mfadm info input.wav --json
mfadm adm input.wav --json
mfadm convert input.wav --to home=output-damf
mfadm convert input.atmos --to standard=output.wav
```

Examples, format rules and compatibility results are available in the versioned documentation.
[English quickstart](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.1.0/README.en.md) ·
[Format reference](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.1.0/docs/REFERENCE.en.md) ·
[Validation record](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.1.0/docs/VALIDATION.md)

### Supported scope

- Atmos master reading supports the subset in the format reference, including one programme, up to one
  complete 7.1.2 bed, and single-track Cartesian objects.
- `LossPolicy::Reject` is the default. Selecting `Allow` or `--allow-loss` records conversion degradations as diagnostics.
- DAMF 3DoF 0.6.0 requires tools supporting that version. Conversion Tool 2.1.2 supports DAMF through 0.5.

When reporting a problem, include the operating system, command and error message.
[Open an issue](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/issues)
