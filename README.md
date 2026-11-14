# MacinFormat-ADM-Core

**简体中文** · [English](README.en.md)

**ADM/BWF 读写与 Atmos 母版写入的 Rust 基础库** · Rust 2024 · MSRV 1.98 · MIT

一个**只管文件格式**的 Rust 库：解析、投影和回写 ADM XML，读写 RIFF/RF64/BW64 容器，并把对象音频流式写成
ADM BWF 或 DAMF 形式的 Atmos 母版。它把渲染、解码导出、对象聚类和编码工具原本各写一份的格式代码集中到一处；
场景模型和算法仍由各消费方自己负责。

> 它不是渲染器、解码器或编码器：不含任何音频算法，不导出 C ABI，也不依赖 AC-4、聚类实现或规范派生表。
> FFI 由调用方自己封装。

## 目录

- [能用它做什么](#能用它做什么)
- [三个 crate](#三个-crate)
- [获取](#获取)
- [快速上手](#快速上手)
- [母版写入规则](#母版写入规则)
- [四种母版配置](#四种母版配置)
- [语义损失与诊断](#语义损失与诊断)
- [不做什么](#不做什么)
- [开发与验收](#开发与验收)
- [外部工具互操作](#外部工具互操作)
- [文档](#文档)
- [来源与消费方](#来源与消费方)
- [许可证](#许可证)

## 能用它做什么

- **读取 ADM**：解析 AXML，解析引用关系，把 Objects / DirectSpeakers / HOA 投影成扁平的 `Snapshot`，
  不受下面 Atmos 写入子集的限制。
- **改写 ADM 且保留原文**：用 `Patch` 做语义修改，未触及的部分（包括未知扩展）原样回写。
- **生成 ADM**：为 DirectSpeakers、HOA3 和 Binaural 渲染结果生成 AXML 与配套 `chna`。
- **读写 WAVE 容器**：RIFF / RF64 / BW64 的流式读写、按帧跳转、`chna` / `bext` 等 chunk 的读取与编辑，
  元数据按调用方给定的预算读取，打开文件时不分配未知 chunk。
- **写 Atmos 母版**：固定 7.1.2 bed 加最多 118 个对象，分块写入 PCM 和对象元数据更新，
  输出 Standard BW64、Logic RF64、DAMF home 或 DAMF 3DoF，完成后原子提交。

## 三个 crate

| crate | 入口 | 负责 |
| --- | --- | --- |
| `macin-adm` | `Document`、`Snapshot`、`Patch`、`generate` | ADM XML、引用解析、Objects/DirectSpeakers/HOA 投影、生成与保留原文的回写 |
| `macin-bwf` | `Reader`、`ChunkReader`、`Writer`、`Chna` | RIFF/RF64/BW64、PCM 与容器编辑；不解释 ADM XML |
| `macin-atmos` | `MasterSpec`、`MasterWriter` | 来源无关的 ADM BWF / DAMF 母版写入；依赖前两者 |

三者都禁止 unsafe（`unsafe_code = "forbid"`），依赖只有 `quick-xml`、`libm` 和 `renamore`，版本全部锁定。

## 获取

源码通过公开 GitHub 仓库分发，用 Git 依赖固定到某个提交即可：

```toml
[dependencies]
macin-atmos = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
# 只需要读写 ADM 或容器时，按需引入：
macin-adm = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
macin-bwf = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
```

三个 crate 已准备 `0.1.0` 的包元数据，**尚未上传 crates.io**；上传步骤见
[crates.io 发布准备](docs/CRATES_IO_RELEASE.md)。

## 快速上手

### 读取一个 ADM BWF

```rust
use macin_adm::{Document, Snapshot};
use macin_bwf::Reader;
use std::fs::File;

fn inspect(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut wav = Reader::new(File::open(path)?)?;
    let info = wav.info();
    // 元数据按调用方给定的预算读取。
    let axml = wav.metadata(*b"axml", 64 << 20)?.ok_or("没有 axml chunk")?;
    let chna: Vec<(String, u16)> = wav
        .chna()?
        .map(|chna| {
            chna.entries
                .iter()
                .map(|e| (String::from_utf8_lossy(&e.uid).into_owned(), e.track_index))
                .collect()
        })
        .unwrap_or_default();
    let doc = Document::parse(std::str::from_utf8(&axml)?).map_err(|e| e.message)?;
    let scene = Snapshot::extract(&doc, info.format.sample_rate, &chna).map_err(|e| e.message)?;
    for object in &scene.objects {
        println!("{}：{} 条轨道", scene.strings[object.name as usize], object.tracks.len);
    }
    Ok(())
}
```

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

## 母版写入规则

**轨道布局**

- 固定 bed 顺序为 L、R、C、LFE、Lss、Rss、Lrs、Rrs、Lts、Rts；可以有真实信号或显式静音。
- 最多 118 个对象，槽位在创建时固定。`TrackId` 是调用方的身份标识，不是文件轨号；实际轨号见 `finish` 返回的报告。

**采样与量化**

- 支持 48/96 kHz、24-bit PCM；默认 48 kHz / 24 fps / Standard BW64。
- 接受交织或平面的 normalized f32，以及已量化的 S24LE。
- 浮点乘 2^23、就近取整（中点远离零），正端点饱和至 8388607；不加抖动、不自动归一化，
  非有限值和超出 ±1 的样本报错。S24LE 原字节保留。

**元数据更新**

- `ObjectUpdate` 使用绝对整数采样位置。块内更新必须落在当前块内；已有完整时间线时，可预先通过
  `schedule_updates` 一次提交。同一轨道不允许时间倒退。
- 连续属性更新显式提供 `ramp_samples`（`0` 表示立即变化）；纯离散更新使用 `None`，
  不会重启位置、增益和尺寸上既有的 ramp。属性缺省表示继承。

**提交与原子性**

- PCM 按块借用并直接写入临时成品，元数据碎片放在同目录的暂存文件里。
- `finish` 追加 AXML、回填长度并原子提交，**不覆盖已存在的目标**。`Drop` 不提交，只清理本任务自己的暂存文件。
- 低层 `metadata` / `containers` 入口供已完成语义映射的批量适配器使用；自动能力检查、诊断与提交事务由
  `MasterWriter` 提供。

## 四种母版配置

| 配置 | 容器与时钟 | 已验证的专属能力 |
| --- | --- | --- |
| `Standard`（默认） | BW64，ADM 九位时钟 | Objects 位置、gain、extent、snap、screenRef、离散 importance |
| `Logic` | RF64，ADM 五位时钟 | 相同 ADM 属性，固定兼容 DBMD 与实际轨数、请求帧率、创建工具字段 |
| `DamfHome` | DAMF 0.5.1，S24LE CAF | 单 bed、对象事件、区域、trimBypass、screen/depth factor、内容类别与耳机策略 |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | 相同音频与事件；不强制改变对象头追策略 |

`interpolationLength` 保留九位来源精度，不随 Logic 时钟降低。ADM 文件的 `ds64` 固定为首 chunk，
`chna` 在音频之前，流式 AXML 在音频之后；依据 ITU-R BS.2088-2 Annex 1 §2.1、§4、§5。

## 语义损失与诊断

默认 `LossPolicy::Reject`：无法精确表达的语义直接报错。只有选 `Allow` 才采用明确的降级，并记录在
`MappingDiagnostic` 里。

- ADM 不擅自映射未验证的 zones、elevation、trim、内容类别、耳机策略和连续 screen/depth factor。
- Logic DBMD 只写已验证的静态兼容字段，不宣称能保存动态耳机事件。
- 已验证的 DAMF 耳机值为 undefined / off / near / far；Mid 被显式拒绝或降为 undefined。
- ADM importance 量化为 0..10，非整十的分值会报告损失。
- 音频 24-bit 量化和所选时钟精度属于已声明的格式行为，不算损失。

## 不做什么

- 不做 DAMF **读取**、多 bed 制作、重采样、渲染或编码算法（首版）。
- 不做渲染、聚类或编解码；不导出 C ABI。
- 不在文件里伪造创建工具来源：即使某个外部检查器因此拒绝，也保留真实创建者（见下文）。

## 开发与验收

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

`rust-toolchain.toml` 锁定 Rust 1.98。可以通过 `CARGO_TARGET_DIR` 或被忽略的 `.cargo/config.toml`
复用已有构建目录；不要复制消费方的 target 或媒体。

测试使用独立的 XML/WAVE 夹具，覆盖分块一致性、ramp、四种配置与两种采样率、原子失败，以及不创建巨型文件的
虚拟 4 GiB 边界检查。原生验证平台为 macOS arm64 与 Windows x64，CI 在这两个平台上跑同一套检查。

## 外部工具互操作

`scripts/check_interop.py` 用调用方提供的外部工具（以及可选的 mradm 读取器）验证合成输出。
同源读回只作为集成验证，不能替代独立的格式验收。

| 配置 | 独立验证 | 已识别的工具限制 |
| --- | --- | --- |
| Standard BW64 | EBU EAR | 本机的 Dolby Conversion Tool / atmos_info 要求 RIFF/RF64，不识别 BW64 |
| Logic RF64 | Conversion Tool 转换（48/96 kHz，PCM 逐字节一致）、EAR | atmos_info 对自行生成的 RF64 做创建工具来源检查；库不会因此改写为 Dolby 创建者 |
| DAMF home | Conversion Tool、atmos_info | — |
| DAMF 3DoF | 较新的 atmos_info | Conversion Tool 2.1.2 的 DAMF 版本上限为 0.5 |

这些边界在检查器里作为明确的工具限制记录，不把拒绝写成通过；未知失败仍使检查失败。
完整矩阵需要在同一 Python 环境里安装 EBU EAR，并提供外部工具路径：

```sh
python scripts/check_interop.py --ear \
  --conversion-tool /path/to/cmdline_atmos_conversion_tool \
  --atmos-info /path/to/atmos_info \
  --report artifacts/interop.json
```

工具版本、逐项结果和消费方迁移的逐字节对比见 [验证记录](docs/VALIDATION.md)。

## 文档

[验证记录](docs/VALIDATION.md) ·
[crates.io 发布准备](docs/CRATES_IO_RELEASE.md) ·
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
