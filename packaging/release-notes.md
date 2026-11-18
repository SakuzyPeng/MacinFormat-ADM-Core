## 简体中文

MacinFormat ADM Core **v0.2.0** 扩展了 ADM、Logic DBMD 与 DAMF 元数据往返，并修正了 96 kHz 母版的 bed 时间线。三个 Rust 库和命令行工具 **mfadm** 同步更新。

### 获取与安装

在本页下方的 **Assets** 中选择适合电脑的 CLI 下载包，解压后在终端运行。

| 你的电脑 | 下载哪个文件 |
| --- | --- |
| Apple 芯片 Mac | `mfadm-v0.2.0-aarch64-apple-darwin.tar.gz` |
| Windows，Intel / AMD 64 位电脑 | `mfadm-v0.2.0-x86_64-pc-windows-msvc.zip` |
| Linux，Intel / AMD 64 位电脑 | `mfadm-v0.2.0-x86_64-unknown-linux-gnu.tar.gz` |

macOS 和 Linux 解压后运行 `./mfadm --help`，Windows PowerShell 运行 `.\mfadm.exe --help`。
也可将可执行文件所在目录加入 `PATH`，随后使用 `mfadm` 命令。
Linux 下载包在 Ubuntu 24.04 构建与验证，使用 GNU libc。

下载包包含中英文说明、格式参考、许可证和构建信息。`.sha256` 与 `SHA256SUMS` 用于核对文件完整性，
`.json` 记录版本、平台和源码提交。

Rust 项目可通过 Cargo 集成三个库，要求 Rust 1.98 或更新版本。

```toml
[dependencies]
macin-adm = "=0.2.0"
macin-bwf = "=0.2.0"
macin-atmos = { version = "=0.2.0", features = ["read"] }
```

`macin-atmos` 的 `read` feature 启用 ADM / DAMF 母版读取与转换，默认配置提供母版写入。
三个 `.crate` 附件提供相应库的源码归档。

### 本次发布

- ADM 可按所属层级保存节目、内容、对象、pack 和声道元数据，包括对象增益、静音、自定义区域、耳机虚拟化及轨道扩展属性。bed 支持分段增益、几何和活动时段。
- Logic 可读写显式 Mid 与未指定的双耳模式、九种 trim 布局、FFOA、帧率及逐声道设置。DAMF 伴随文件的 DD+ 压缩配置可与 Logic 互转。
- 96 kHz Logic 的相邻块保持采样边界。bed 起点的增益生效，节目外的块会被裁掉。超出时段或时钟范围的 bed 更新可在修正后重试。
- `mfadm convert` 支持按字段选择 `--approximate` 和 `--preserve-literal`。JSON 报告区分损失、近似和原值保留。

### 从 v0.1.0 升级

- `ObjectState::gain` 和对象更新中的 gain 现在是 block 自身的增益。需要合并对象级增益和静音时，调用 `object.adm.effective_gain(block_gain)`。
- `MasterSpec::trim_mode` 从 4 项扩展为 9 项，前四个索引保持原义。请更新显式数组初始化，以及对 `TrimConfiguration`、`WarpMode`、`DownmixMode` 等枚举的穷尽匹配。
- 公开模型新增字段。使用结构体字面量的调用方可补齐字段，或在类型支持时使用 `..Default::default()`。手工往返含动态自定义区域的对象时，使用 `native_updates()` 与 `schedule_native_updates()`。

### 第一次使用

```sh
mfadm info input.wav --json
mfadm adm input.wav --json
mfadm convert input.wav --to home=output-damf
mfadm convert input.atmos --to standard=output.wav
```

完整示例、格式规则与兼容性记录见对应版本文档。
[中文上手说明](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.2.0/README.md) ·
[格式参考](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.2.0/docs/REFERENCE.md) ·
[验证记录](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.2.0/docs/VALIDATION.md)

### 支持范围

- Atmos 母版读取支持格式参考列出的子集，包含一个 programme、至多一个完整 7.1.2 bed，以及单轨 Cartesian 对象。
- 默认采用 `LossPolicy::Reject`。显式选择 `Allow` 或 `--allow-loss` 后，转换会记录降级诊断。
- DAMF 3DoF 0.6.0 需要支持该版本的工具，Conversion Tool 2.1.2 的 DAMF 版本上限为 0.5。

反馈问题时请提供系统版本、执行命令和错误信息。
[提交 Issue](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/issues)

## English

MacinFormat ADM Core **v0.2.0** expands metadata round trips across ADM, Logic DBMD and DAMF and fixes bed timing in 96 kHz masters. This release includes all three Rust libraries and the **mfadm** CLI.

### Download and install

Choose a CLI archive from **Assets** below, extract it and run the executable in a terminal.

| Your computer | File to download |
| --- | --- |
| Apple silicon Mac | `mfadm-v0.2.0-aarch64-apple-darwin.tar.gz` |
| Windows, 64-bit Intel / AMD PC | `mfadm-v0.2.0-x86_64-pc-windows-msvc.zip` |
| Linux, 64-bit Intel / AMD PC | `mfadm-v0.2.0-x86_64-unknown-linux-gnu.tar.gz` |

On macOS and Linux, run `./mfadm --help` after extraction. In Windows PowerShell, run `.\mfadm.exe --help`.
Adding the executable's directory to `PATH` makes the `mfadm` command available from other directories.
The Linux archive is built and verified on Ubuntu 24.04 and uses GNU libc.

Archives include Chinese and English documentation, the format reference, licenses and build information.
Use `.sha256` or `SHA256SUMS` to check file integrity. The `.json` files record the version, target and source commit.

Rust projects can use the libraries through Cargo with Rust 1.98 or newer.

```toml
[dependencies]
macin-adm = "=0.2.0"
macin-bwf = "=0.2.0"
macin-atmos = { version = "=0.2.0", features = ["read"] }
```

The `macin-atmos/read` feature enables ADM / DAMF master reading and conversion. The default configuration
provides master writing. The three `.crate` attachments contain the corresponding library sources.

### Changes in this release

- ADM retains programme, content, object, pack and channel metadata at the owning level, including object gain and mute, custom exclusion zones, headphone virtualisation and track extension attributes. Beds support block-level gain, geometry and active intervals.
- Logic reads and writes explicit Mid and unspecified binaural modes, nine trim layouts, first frame of action, frame rate and per-channel settings. DD+ compression profiles can move between Logic and DAMF sidecars.
- Adjacent blocks retain their sample boundaries in 96 kHz Logic masters. Bed updates at the object start take effect, and conversion clips beds to the programme. A batch with an out-of-range bed update can be corrected and retried.
- `mfadm convert` supports per-field `--approximate` and `--preserve-literal` options. JSON reports distinguish losses, approximations and literal preservation.

### Upgrading from v0.1.0

- `ObjectState::gain` and gain updates now hold the block’s own gain. Use `object.adm.effective_gain(block_gain)` to apply object gain and mute.
- `MasterSpec::trim_mode` now has nine slots. The first four retain their previous indices. Update explicit array initialisers and exhaustive matches on enums such as `TrimConfiguration`, `WarpMode` and `DownmixMode`.
- Public model structs have new fields. Supply them in struct literals or use `..Default::default()` where available. Manual round trips with changing custom zones use `native_updates()` and `schedule_native_updates()`.

### First use

```sh
mfadm info input.wav --json
mfadm adm input.wav --json
mfadm convert input.wav --to home=output-damf
mfadm convert input.atmos --to standard=output.wav
```

Examples, format rules and compatibility results are available in the versioned documentation.
[English quickstart](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.2.0/README.en.md) ·
[Format reference](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.2.0/docs/REFERENCE.en.md) ·
[Validation record](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/blob/v0.2.0/docs/VALIDATION.md)

### Supported scope

- Atmos master reading supports the subset in the format reference, including one programme, up to one
  complete 7.1.2 bed, and single-track Cartesian objects.
- `LossPolicy::Reject` is the default. Selecting `Allow` or `--allow-loss` records conversion degradations as diagnostics.
- DAMF 3DoF 0.6.0 requires tools supporting that version. Conversion Tool 2.1.2 supports DAMF through 0.5.

When reporting a problem, include the operating system, command and error message.
[Open an issue](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/issues)
