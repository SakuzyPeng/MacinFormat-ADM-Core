# MacinFormat-ADM-Core

**简体中文** · [English](README.en.md)

**ADM/BWF 读写与 Atmos 母版写入的 Rust 基础库** · Rust 2024 · MSRV 1.98 · MIT

一个**只管文件格式**的 Rust 库：解析、投影和回写 ADM XML，读写 RIFF/RF64/BW64 容器，并把对象音频流式写成
ADM BWF 或 DAMF 形式的 Atmos 母版，也能把 DAMF 和 ADM BWF 读回同一个模型、转换到其他配置。它把渲染、解码导出、对象聚类和编码工具原本各写一份的格式代码集中到一处；
场景模型和算法仍由各消费方自己负责。

> 它不是渲染器、解码器或编码器：不含任何音频算法，不导出 C ABI，也不依赖 AC-4、聚类实现或规范派生表。
> FFI 由调用方自己封装。

本文只讲能做什么、怎么接入和上手。**每一步写入怎么处理、读取时什么按损失处理、什么直接拒绝，见
[格式参考](docs/REFERENCE.md)**（[English](docs/REFERENCE.en.md)）。

## 目录

- [能用它做什么](#能用它做什么)
- [四个 crate](#四个-crate)
- [获取](#获取)
- [快速上手](#快速上手)
- [命令行工具 mfadm](#命令行工具-mfadm)
- [选哪种母版配置](#选哪种母版配置)
- [支持范围与已知限制](#支持范围与已知限制)
- [外部工具兼容性](#外部工具兼容性)
- [从源码构建](#从源码构建)
- [文档](#文档)
- [来源与消费方](#来源与消费方)
- [许可证](#许可证)

## 能用它做什么

- **读取 ADM**：解析 AXML，解析引用关系，把 Objects / DirectSpeakers / HOA 投影成扁平的 `Snapshot`，
  不受 Atmos 母版子集的限制。
- **改写 ADM 且保留原文**：用 `Patch` 做语义修改，未触及的部分（包括未知扩展）原样回写。
- **生成 ADM**：为 DirectSpeakers、HOA3 和 Binaural 渲染结果生成 AXML 与配套 `chna`。
- **读写 WAVE 容器**：RIFF / RF64 / BW64 的流式读写、按帧跳转、`chna` / `bext` 等 chunk 的读取与编辑；
  既能沿用 libbw64 的量化，也能和母版写入用同一约定（[量化约定](docs/REFERENCE.md#pcm-量化约定)）。
- **写 Atmos 母版**：固定 7.1.2 bed 加最多 118 个对象，分块写入 PCM 和对象元数据更新，
  输出 Standard BW64、Logic RF64、DAMF home 或 DAMF 3DoF，完成后原子提交。
- **一次写出多份**：`MultiWriter` 把同一个模型在一次写入里同时输出成 ADM 和 DAMF 等多种配置。
- **读 DAMF / ADM、转换配置**：`damf::DamfReader` 和 `adm::AdmReader` 把 DAMF 或 ADM BWF 读回写入用的模型，
  转成任意配置（ADM 转 DAMF、DAMF 转 ADM、同类配置互转）；音频也可以换成一个 WAV，即“WAV 加元数据”写母版。
- **命令行**：`mfadm` 列出 chunk、音频格式、`chna` 和 ADM 摘要，并能做 DAMF / ADM 转换，可输出 JSON。

## 四个 crate

| crate | 入口 | 负责 |
| --- | --- | --- |
| `macin-adm` | `Document`、`Snapshot`、`Patch`、`generate` | ADM XML、引用解析、Objects/DirectSpeakers/HOA 投影、生成与保留原文的回写；按 `Snapshot` 节点号只读访问原始元素 |
| `macin-bwf` | `Reader`、`ChunkReader`、`Writer`、`Chna` | RIFF/RF64/BW64、PCM 与容器编辑；不解释 ADM XML |
| `macin-atmos` | `MasterSpec`、`MasterWriter`、`MultiWriter`；`damf::DamfReader`、`adm::AdmReader`（`read` feature） | 来源无关的 ADM BWF / DAMF 母版写入，以及 DAMF / ADM 读取与转换；依赖前两者 |
| `macin-format`（CLI） | 命令行工具 `mfadm` | 检查与转换的命令行外壳 |

全部禁止 unsafe；三个库只依赖 `quick-xml`、`libm` 和 `renamore`，版本锁定。读取器在默认关闭的 `read` feature 后面，
只写母版的消费方依赖图不变。详见[依赖与 feature](docs/REFERENCE.md#依赖与-feature)。

## 获取

三个库通过 crates.io 分发，要求 Rust 1.98 或更新版本。按需添加依赖。

```toml
[dependencies]
macin-atmos = "=0.2.0"
# 只需要读写 ADM 或容器时，按需引入：
macin-adm = "=0.2.0"
macin-bwf = "=0.2.0"
```

读取 DAMF / ADM 母版或做转换时启用 `read`。

```toml
macin-atmos = { version = "=0.2.0", features = ["read"] }
```

源码也可通过 Git 依赖固定到具体提交。

```toml
macin-atmos = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
```

`mfadm` 提供 macOS Apple 芯片、Windows x64 和 Linux x64 下载包，解压后在终端运行。
[v0.2.0 发布页](https://github.com/SakuzyPeng/MacinFormat-ADM-Core/releases/tag/v0.2.0)

## 快速上手

### 读取一个 ADM BWF

```rust
use macin_adm::{Document, Snapshot, TrackUidDefinition};
use macin_bwf::{Chna, Reader};
use std::fs::File;

fn inspect(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut wav = Reader::new(File::open(path)?)?;
    let rate = wav.info().format.sample_rate;
    // 元数据按调用方给定的预算读取。
    let axml = wav.metadata(*b"axml", 64 << 20)?.ok_or("没有 axml chunk")?;
    // 导入侧解码：容忍末尾的预留记录；track index 0 表示未使用的条目。
    let entries = match wav.metadata(*b"chna", 4 + 40 * u64::from(u16::MAX))? {
        Some(data) => Chna::decode_import(&data)?.entries,
        None => Vec::new(),
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim_end_matches([' ', '\0']).to_owned();
    let mut channels = Vec::new();
    let mut definitions = Vec::new();
    for entry in entries.iter().filter(|e| e.track_index != 0) {
        let uid = text(&entry.uid);
        let mut format_id = text(&entry.track_format);
        // CHNA 里引用 AC 时带 `_00` 填充。
        if format_id.to_ascii_uppercase().starts_with("AC_") && format_id.ends_with("_00") {
            format_id.truncate(format_id.len() - 3);
        }
        let pack = text(&entry.pack_format);
        channels.push((uid.clone(), entry.track_index));
        definitions.push(TrackUidDefinition {
            uid,
            format_id,
            pack_id: (!pack.is_empty()).then_some(pack),
        });
    }
    // AXML 里缺少的 audioTrackUID 由 CHNA 补全，两者冲突时报错。
    let doc = Document::parse_with_track_uids(std::str::from_utf8(&axml)?, &definitions)
        .map_err(|e| e.message)?;
    let scene = Snapshot::extract(&doc, rate, &channels).map_err(|e| e.message)?;
    for object in &scene.objects {
        println!("{}：{} 条轨道", scene.strings[object.name as usize], object.tracks.len);
    }
    Ok(())
}
```

有的文件只在 CHNA 里定义 `audioTrackUID`，AXML 里并不写出来，所以示例把 CHNA 的轨道定义交给
`parse_with_track_uids` 补全，而不是只用它做轨道号映射。

`Snapshot` 是扁平表：名称和 ID 都是 `strings` 的下标，子元素用 `Span` 指向对应的表。`warnings` 同样是
`strings` 的下标，说明哪些不支持的 `typeDefinition` 被跳过了。

### 写一个 Atmos 母版

```rust
use macin_atmos::*;

fn example() -> Result<()> {
    let spec = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(42), "Cluster 0")],
        ..Default::default()
    };
    let mut master = MasterWriter::create("result.wav", spec)?;
    // 十路 7.1.2 bed，然后是 spec.objects 声明顺序。
    let pcm = vec![0.0f32; 512 * 11];
    master.write_block(PcmBlock::F32Interleaved(&pcm), &[])?;
    let report = master.finish()?;
    assert_eq!(report.objects[0].track_index, 11);
    Ok(())
}
```

带位置 ramp、耳机策略事件、四种配置和两种采样率的完整示例是
[`synthetic_master`](crates/macin-atmos/examples/synthetic_master.rs)：

```sh
cargo run -p macin-atmos --example synthetic_master -- result.wav standard 96000
```

第二个参数可选 `standard` / `logic` / `home` / `3dof`，第三个可选 `48000` / `96000`。

### 同时写 ADM 和 DAMF

```rust
use macin_atmos::*;

fn both() -> Result<()> {
    let spec = MasterSpec {
        objects: vec![ObjectSpec::new(TrackId(42), "Cluster 0")],
        ..Default::default()
    };
    let targets = [
        Target { path: "result.wav".into(), profile: Profile::Logic },
        Target { path: "result-damf".into(), profile: Profile::DamfHome },
    ];
    // 同一份模型、同一次写入；PCM 只量化一次，两份输出要么都提交，要么都不提交。
    let mut master = MultiWriter::create(&spec, &targets)?;
    let pcm = vec![0.0f32; 512 * 11];
    master.write_block(PcmBlock::F32Interleaved(&pcm), &[])?;
    let reports = master.finish()?; // 每个 target 一份 WriteReport
    assert_eq!(reports.len(), 2);
    Ok(())
}
```

### 把 DAMF 转成 ADM

```rust
use macin_atmos::damf::DamfReader;
use macin_atmos::{LossPolicy, Profile, Result, Target};

fn convert() -> Result<()> {
    // 读取时按同一个 LossPolicy 检查模型装不下的内容。
    let reader = DamfReader::open("result-damf/master.atmos", LossPolicy::Allow)?;
    let targets = [Target { path: "from-damf.wav".into(), profile: Profile::Standard }];
    // 换成 reader.wave_audio("mix.wav")? 就是“WAV 加 DAMF 元数据”。
    let conversion = reader.convert(reader.audio()?, &targets)?;
    for loss in conversion.read.iter().chain(&conversion.outputs[0].diagnostics) {
        eprintln!("{:?} {}: {}", loss.track, loss.field, loss.message);
    }
    Ok(())
}
```

### 把 ADM 转成 DAMF

```rust
use macin_atmos::adm::AdmReader;
use macin_atmos::{FrameRate, LossPolicy, Profile, Result, Target};

fn adm_to_damf() -> Result<()> {
    // 默认从 DBMD 恢复帧率；需要覆盖时显式指定。
    let reader = AdmReader::open("result.wav", LossPolicy::Reject)?.with_frame_rate(FrameRate::Fps25);
    let targets = [
        Target { path: "from-adm-home".into(), profile: Profile::DamfHome },
        Target { path: "from-adm-3dof".into(), profile: Profile::DamfThreeDof },
    ];
    let conversion = reader.convert(reader.audio()?, &targets)?;
    assert_eq!(conversion.outputs.len(), 2);
    Ok(())
}
```

## 命令行工具 mfadm

`mfadm` 用来查看一个 RIFF/RF64/BW64 文件里有什么，以及转换 DAMF 和 ADM 母版。它从不修改输入，也不覆盖已存在的输出。
安装到 `~/.cargo/bin`：

```sh
cargo install --locked --path crates/macin-format
```

| 子命令 | 输出 |
| --- | --- |
| `mfadm chunks <file>` | 容器类型，以及每个 chunk 的 ID、负载偏移和大小；不解码 `fmt ` 和音频 |
| `mfadm info <file>` | 采样率、声道数、位深、帧数与时长，`chna` 里的轨道对应关系 |
| `mfadm adm <file>` | 按文件自身的 `chna` 和采样率解析 ADM，汇总 programme、content、object、轨道与 block，列出被跳过的 `typeDefinition` |
| `mfadm axml <file>` | 把 `axml` chunk 原样写到 stdout，方便和外部工具的结果做对比 |
| `mfadm convert <x.atmos 或 x.wav> --to PROFILE=PATH …` | 把 DAMF 或 ADM BWF 一次写成一种或多种配置；`--audio <wav>` 改用 WAV 作音频，`--fps` 指定 ADM 来源的帧率，`--allow-loss` 允许降级并列出损失 |

```text
$ mfadm adm result.wav
1 programmes, 1 contents, 3 objects, 0 HOA packs; 4 object blocks, 10 DirectSpeakers blocks
...
object AO_100b "Moving object"  Objects
  start 0:00:00.000, duration 0:00:02.000, gain 1
  ATU_0000000b -> track 11, 3 object blocks
```

```text
$ mfadm convert film/master.atmos --to standard=film.wav --to 3dof=film-3dof --allow-loss
film/master.atmos (home): 2 objects, 3 updates
-> film.wav (standard): 96000 frames, 12 channels, 48000 Hz
   lost binaural_mode (track 10, sample 36000): Property has no exact mapping in this ADM profile
-> film-3dof (3dof): 96000 frames, 12 channels, 48000 Hz
```

要把 Logic 母版交给 Dolby 工具，建议加上 `--dbmd-originator "Created using Dolby equipment"`，否则 Conversion Tool
会提示文件不是 Dolby 认可的软件生成的。`--json`、元数据预算、退出码等其余参数见 [mfadm 参考](docs/REFERENCE.md#mfadm-参考)。

## 选哪种母版配置

| 配置 | 文件 | 适合 |
| --- | --- | --- |
| `Standard`（默认） | BW64 ADM BWF | 通用的 ADM 工具链，例如 EBU EAR |
| `Logic` | RF64 ADM BWF，带 DBMD | 导入 Logic Pro，或交给只认 RIFF/RF64 的 Dolby 工具 |
| `DamfHome` | DAMF 0.5.1（`.atmos`、`.atmos.metadata`、`.atmos.audio`），可附 `.atmos.dbmd` | Dolby Atmos Renderer 与 Conversion Tool |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | 需要 3DoF DAMF 的较新 Dolby 工具 |

不确定选哪个时，用 `MultiWriter` 或 `mfadm convert --to … --to …` 一次写出几份，PCM 只量化一次、各份音频完全相同。
每种配置能表达哪些属性、用什么时钟，见[四种母版配置](docs/REFERENCE.md#四种母版配置)。

## 支持范围与已知限制

- **默认拒绝损失**：`LossPolicy::Reject` 下，目标配置无法精确表达的语义直接报错并指明字段；
  可用 `--approximate` 按字段授权近似，其余降级仍需 `Allow`（或 `--allow-loss`），两者均记入诊断。
  见[语义损失与诊断](docs/REFERENCE.md#语义损失与诊断)。
- **母版只有一种形状**：固定 7.1.2 bed 加最多 118 个对象，48/96 kHz、24-bit；不做多 bed 制作和重采样。
- **读取只支持 Atmos 子集**：一个 programme、至多一个 7.1.2 bed、单轨 Cartesian 对象。极坐标、HOA、多 bed、
  嵌套对象与多个 presentation 直接拒绝。逐项清单见 [DAMF 读取](docs/REFERENCE.md#damf-读取与转换) 与
  [ADM 读取](docs/REFERENCE.md#adm-读取与转换)。通用的 `Document` / `Snapshot` 解析不受此限。
- **原生映射**：ADM 可保留对象参数、bed 分段及轨道扩展属性。DAMF 可保留 bed 状态和渲染事件。兼容性受限的原值可用 `--preserve-literal` 逐项选择。
  具体范围与限制见[格式参考](docs/REFERENCE.md#原生-adm-元数据与容器保留)。
- **帧率**：从可识别 DBMD 恢复；未提供时默认 24 fps，可用 `with_frame_rate` 或 `--fps` 覆盖。
- **原子提交，不覆盖**：写入完成才发布，已存在的目标不会被覆盖；`MultiWriter` 发布失败会撤回已发布的部分，
  但挡不住进程恰好在两次 rename 之间崩溃。
- 不做渲染、聚类或编解码；不导出 C ABI。

## 外部工具兼容性

| 配置 | 已通过的独立检查 | 已知工具限制 |
| --- | --- | --- |
| Standard BW64 | EBU EAR | Dolby Conversion Tool / atmos_info 要求 RIFF/RF64，不识别 BW64 |
| Logic RF64 | Conversion Tool（48/96 kHz，PCM 逐字节一致）、EAR | 默认创建来源会触发 Dolby 工具的来源提示，见上面的 `--dbmd-originator` |
| DAMF home | Conversion Tool、atmos_info | — |
| DAMF 3DoF | 较新的 atmos_info | Conversion Tool 2.1.2 的 DAMF 版本上限为 0.5 |

另外，2026-11-15 实测 48 kHz / 24-bit 的 Logic RF64 与 DAMF home 母版可以在 Logic Pro 和 Dolby Atmos Renderer 中使用
（Logic 样本使用上面的创建来源）。
工具版本、样本参数和逐项结果见[验证记录](docs/VALIDATION.md)。

## 从源码构建

`rust-toolchain.toml` 锁定 Rust 1.98，rustup 会自动安装。

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo clippy --locked -p macin-atmos --all-targets -- -D warnings
cargo test --locked -p macin-atmos
```

后两条单独检查不开 `read` 的 `macin-atmos`。测试只用仓库内的合成夹具，不需要媒体或外部工具；
CI 在 macOS arm64、Windows x64 与 Linux x64 上跑同一套检查。外部工具互操作检查器的用法见
[验证记录](docs/VALIDATION.md#互操作检查器用法)，开发约定见 [AGENTS.md](AGENTS.md)。

## 文档

[格式参考](docs/REFERENCE.md)（[English](docs/REFERENCE.en.md)）是读写规则的全集。

[验证记录](docs/VALIDATION.md) ·
[crates.io 发布准备](docs/CRATES_IO_RELEASE.md) ·
[CLI 下载包与 GitHub 发布](docs/BINARY_RELEASE.md) ·
[来源清单（SOURCES.json）](docs/SOURCES.json) ·
[互操作摘要（interop-summary.json）](docs/interop-summary.json)

## 来源与消费方

ADM/BWF 实现迁自 MacinRender-ADM-Core，Atmos 序列化承接 MacinDecode-AC4-Core 已验证的写侧。
每个迁入文件的来源提交与 SHA-256 记录在 [SOURCES.json](docs/SOURCES.json)。

| 仓库 | 与本库的关系 |
| --- | --- |
| [MacinRender-ADM-Core](https://github.com/SakuzyPeng/MacinRender-ADM-Core) | ADM/BWF 读写的来源；渲染端消费方 |
| [MacinDecode-AC4-Core](https://github.com/SakuzyPeng/MacinDecode-AC4-Core) | Atmos 母版写侧的来源；解码导出消费方 |

对象聚类和编码工具也是本库的消费方。

## 许可证

本项目采用 [MIT License](LICENSE)。

`macin-adm` 内置的 ADM common definitions 来自 [libadm](https://github.com/ebu/libadm) 0.14.0，未经修改，
采用 Apache-2.0 许可；许可证、NOTICE 和源文件哈希随资源保留在
[`crates/macin-adm/assets/`](crates/macin-adm/assets/)。
