# MacinFormat-ADM-Core

**简体中文** · [English](README.en.md)

**ADM/BWF 读写与 Atmos 母版写入的 Rust 基础库** · Rust 2024 · MSRV 1.98 · MIT

一个**只管文件格式**的 Rust 库：解析、投影和回写 ADM XML，读写 RIFF/RF64/BW64 容器，并把对象音频流式写成
ADM BWF 或 DAMF 形式的 Atmos 母版，也能把 DAMF 和 ADM BWF 读回同一个模型、转换到其他配置。它把渲染、解码导出、对象聚类和编码工具原本各写一份的格式代码集中到一处；
场景模型和算法仍由各消费方自己负责。

> 它不是渲染器、解码器或编码器：不含任何音频算法，不导出 C ABI，也不依赖 AC-4、聚类实现或规范派生表。
> FFI 由调用方自己封装。

## 目录

- [能用它做什么](#能用它做什么)
- [三个 crate](#三个-crate)
- [获取](#获取)
- [快速上手](#快速上手)
- [命令行工具 mfadm](#命令行工具-mfadm)
- [母版写入规则](#母版写入规则)
- [四种母版配置](#四种母版配置)
- [DAMF 读取与转换](#damf-读取与转换)
- [ADM 读取与转换](#adm-读取与转换)
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
  元数据按调用方给定的预算读取，打开文件时不分配未知 chunk。`Writer::write_frames` 保持 libbw64 0.10.0 的量化
  （乘 2^(b−1)−1、向零截断）；`write_frames_nearest` 与母版写入同一约定（乘 2^(b−1)、就近取整、中点远离零），
  `read_frames` 读出的 16/24-bit 样本经它写回逐字节不变。
- **写 Atmos 母版**：固定 7.1.2 bed 加最多 118 个对象，分块写入 PCM 和对象元数据更新，
  输出 Standard BW64、Logic RF64、DAMF home 或 DAMF 3DoF，完成后原子提交。
- **一次写出多份**：`MultiWriter` 把同一个模型在一次写入里同时输出成 ADM 和 DAMF 等多种配置。
- **读 DAMF / ADM、转换配置**：`damf::DamfReader` 和 `adm::AdmReader` 把 DAMF 或 ADM BWF 读回写入用的模型，
  转成任意配置（ADM 转 DAMF、DAMF 转 ADM、同类配置互转）；音频也可以换成一个 WAV，即“WAV 加元数据”写母版。
- **命令行**：`mfadm` 列出 chunk、音频格式、`chna` 和 ADM 摘要，并能做 DAMF 转换，可输出 JSON。

## 三个 crate

| crate | 入口 | 负责 |
| --- | --- | --- |
| `macin-adm` | `Document`、`Snapshot`、`Patch`、`generate` | ADM XML、引用解析、Objects/DirectSpeakers/HOA 投影、生成与保留原文的回写；按 `Snapshot` 节点号只读访问原始元素 |
| `macin-bwf` | `Reader`、`ChunkReader`、`Writer`、`Chna` | RIFF/RF64/BW64、PCM 与容器编辑；不解释 ADM XML |
| `macin-atmos` | `MasterSpec`、`MasterWriter`、`MultiWriter`；`damf::DamfReader`、`adm::AdmReader`（`read` feature） | 来源无关的 ADM BWF / DAMF 母版写入，以及 DAMF / ADM 读取与转换；依赖前两者 |

三者都禁止 unsafe（`unsafe_code = "forbid"`），依赖只有 `quick-xml`、`libm` 和 `renamore`，版本全部锁定。
两个读取器放在默认关闭的 `read` feature 后面。DAMF 读取需要 YAML 解析，只有打开它才会引入 `saphyr-parser`
（同样锁定版本）；ADM 读取不需要额外依赖，但与 DAMF 共用同一套转换流程。只写母版的消费方依赖图不变。

另有不发布的 `macin-format`，提供命令行工具 [`mfadm`](#命令行工具-mfadm)；它额外依赖 `clap` 和 `serde_json`，
这些依赖不会进入三个库。

## 获取

源码通过公开 GitHub 仓库分发，用 Git 依赖固定到某个提交即可：

```toml
[dependencies]
macin-atmos = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
# 只需要读写 ADM 或容器时，按需引入：
macin-adm = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
macin-bwf = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>" }
```

需要读 DAMF 或做转换时打开 `read`：

```toml
macin-atmos = { git = "https://github.com/SakuzyPeng/MacinFormat-ADM-Core", rev = "<commit>", features = ["read"] }
```

三个 crate 已准备 `0.1.0` 的包元数据，**尚未上传 crates.io**；上传步骤见
[crates.io 发布准备](docs/CRATES_IO_RELEASE.md)。

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
`parse_with_track_uids` 补全，而不是只用它做轨道号映射。`mfadm adm` 在此之外还会核对 CHNA 的轨道数与 `fmt `
一致、记录没有被截断。

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

## 命令行工具 mfadm

`mfadm` 用来查看一个 RIFF/RF64/BW64 文件里有什么，以及转换 DAMF。它从不修改输入，也不覆盖已存在的输出。
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
| `mfadm convert <x.atmos 或 x.wav> --to PROFILE=PATH …` | 把 DAMF 或 ADM BWF（按文件内容区分）一次写成一种或多种配置；`--audio <wav>` 改用 WAV 作音频，`--fps` 指定 ADM 来源的帧率，`--allow-loss` 允许降级并列出损失 |

`chunks`、`info`、`adm`、`convert` 加 `--json` 输出机器可读的 JSON，供脚本使用。`adm` 和 `axml` 读元数据时有预算，
默认 256 MiB，可用 `--max-metadata <MiB>` 调整。出错时退出码为 1，参数错误为 2。

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
   lost head_tracking (track 10, sample 36000): Property has no exact mapping in this ADM profile
   lost binaural_mode (track 10, sample 36000): Property has no exact mapping in this ADM profile
-> film-3dof (3dof): 96000 frames, 12 channels, 48000 Hz
```

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

**一次写出多份（`MultiWriter`）**

- 所有 target 共用一个 `MasterSpec`，只有配置不同；PCM 只量化一次，各份输出的音频完全相同。
  每份输出与单独用 `MasterWriter` 写出的结果逐字节一致。
- 更新先在每个 target 上校验和映射，全部通过才提交。某个 target 拒绝（例如 `Reject` 下 ADM 不能表达耳机策略）时，
  所有 target 都保持原样，可以改正后重试，和单个 writer 的行为一致。
- `finish` 依次发布各份输出；其中一份发布失败时，把已发布的移回暂存区再报错，不留下半套结果。
  这个回滚挡不住进程在两次 rename 之间崩溃：那种情况下可能只留下一部分输出。

## 四种母版配置

| 配置 | 容器与时钟 | 已验证的专属能力 |
| --- | --- | --- |
| `Standard`（默认） | BW64，ADM 九位时钟 | Objects 位置、gain、extent、snap、screenRef、离散 importance |
| `Logic` | RF64，ADM 五位时钟 | 相同 ADM 属性，固定兼容 DBMD 与实际轨数、请求帧率、创建工具字段 |
| `DamfHome` | DAMF 0.5.1，S24LE CAF | 单 bed、对象事件、区域、trimBypass、screen/depth factor、内容类别与耳机策略 |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | 相同音频与事件；不强制改变对象头追策略 |

`interpolationLength` 保留九位来源精度，不随 Logic 时钟降低。ADM 文件的 `ds64` 固定为首 chunk，
`chna` 在音频之前，流式 AXML 在音频之后；依据 ITU-R BS.2088-2 Annex 1 §2.1、§4、§5。

## DAMF 读取与转换

需要 `read` feature。`DamfReader::open` 读入 `.atmos` 清单，得到和写入时相同的 `MasterSpec`，
再由 `updates()` 逐条给出 `ObjectUpdate`；`convert` 经 `MultiWriter` 写成任意组合的配置。转换就是普通的写入，
所以上面每种配置的损失检查照常生效，读取时模型装不下的内容也按同一个 `LossPolicy` 处理。

- **内存有界**：元数据文件扫描两遍、从不整体载入。第一遍校验整条时间线，并把每个对象开头位于第 0 个采样、
  不带 ramp 的事件折叠成初始状态；第二遍按文件顺序给出其余更新。实测 118 个对象、70.8 万个事件、一次写出三份
  （约 1.1–1.4 GB 一份）时峰值内存 12 MiB。
- **ID 与声道**：DAMF 的 ID 就是音频声道号。bed 按 L、R、C、LFE、Lss、Rss、Lrs、Rrs、Lts、Rts 重新排列，
  对象按 ID 升序排在后面，读回的 `TrackId` 等于 DAMF ID。事件省略 `ID` 时沿用前一个事件的值，首个事件必须提供 `ID`。
  没有 bed 的 DAMF 会写出一组静音 bed。
- **WAV 作音频**：`wave_audio` 用 WAV 代替 `.atmos.audio`，第 n 个声道对应 DAMF ID n，采样率必须与元数据一致。
  16/24-bit PCM 精确转换；32-bit PCM 和浮点按已声明的格式行为量化到 24 bit，超出 ±1 的浮点样本报错。
- **按损失处理**（`Reject` 报错并指明字段，`Allow` 丢弃并记入诊断）：bed 声道上固定 bed 无法再现的值（`bed_*`）、
  未知字段（`unknown_field`）、没有连续属性却带非零 `rampLength` 的事件（`ramp`）、没有第 0 个采样处事件的对象
  （`initial_state`），以及与写入侧固定值不同的 `simplified`、`offset`、`scBedConfiguration`、`downmixType_5to2`、
  `51-to-20_LsRs90degPhaseShift` 和自定义 `trimMode`。
- **直接拒绝**：多个 presentation、多个 bed、不是完整 7.1.2 的 bed、0.5/0.6 以外的 DAMF 版本、
  YAML 锚点/别名/标签和多文档、非 S24LE 的 CAF、时间倒退或引用未声明 ID 的事件、元数据超出音频长度。
- 转换写入的是本库自己的创建工具字段；源文件的 `creationTool` 可通过 `creation_tool()` 查看，不会被冒用。

## ADM 读取与转换

需要 `read` feature。`AdmReader::open` 把 ADM BWF 读成与写入时相同的 `MasterSpec` 和一组 `ObjectUpdate`，
`convert` 同样经 `MultiWriter` 写出，所以 ADM 转 DAMF 就是普通的写入，DAMF 一侧的损失检查照常生效。
支持的是 Atmos 子集：本库写出的结构，以及其他工具写出的同类结构。

- **支持的结构**：一个 programme；至多一个 7.1.2 `DirectSpeakers` bed，声道用 `RC_L`…`RC_Rts`（或不带 `RC_`）
  标记，顺序不限；任意个单轨、Cartesian 坐标的 `Objects`，省略 Z 时取 0，布尔字段接受 `0` / `1` 和 `false` / `true`；
  轨道定义可以只写在 CHNA 里。Logic RF64（带 DBMD）读成
  `Logic`，其余读成 `Standard`。
- **数值精度**：位置、尺寸等从文档原文按 `f64` 解析，而不是取 `Snapshot` 里的 `f32` 投影，所以本库写出的
  ADM 读回再写，Standard 与原文件逐字节一致。Logic 的 DBMD 帧率和创建者字段不读回，按同样的帧率与字段写出时
  也逐字节一致。
- **时间线**：每个 block 自成一体，没写的元素取缺省值。block 起点成为一次更新，只包含与前一个 block 不同的字段；
  `jumpPosition` 为 1 时 ramp 取 `interpolationLength`（不超过 block 长度），没写 `jumpPosition` 时在整个 block 内插值。
  对象的 `start` / `duration` 变成激活与停用的更新，结束点还受最后一个有限 block 的结束时间约束。
  换用更长的 WAV 也保留停用边界；`updates_for_frames` 按实际音频长度提供更新，`updates` 返回原文件长度下的更新。
  对象级 `gain` 乘进每个 block，`mute` 读成静音。
  ADM 没有 active 标志：写入器把不活动的对象写成 gain 0，读回就是静音增益。
- **ID 与声道**：对象按轨道号排序，`TrackId` 是对象在文件里的轨道号（从 1 起）；`wave_audio` 用 WAV 代替音频时，
  第 n 个声道对应第 n + 1 条轨道。没有 bed 的文件写出一组静音 bed。
- **按损失处理**：`zoneExclusion` 等没有映射的 block 元素（`unknown_field`）、非零 `diffuse`、`objectDivergence`、
  `headLocked`（`head_tracking`）、带 `maxDistance` 的 `channelLock`、对象的 `positionOffset`、`dialogue`、
  非 10 的对象级 `importance`、`interact` / `disableDucking`，programme / content 的响度、语言、标签和对白分类，
  非零的 programme 起点，以及 bed 声道上的增益、非默认优先级、`headLocked`、位置边界和屏幕边缘锁定。
  bed 各声道未连续覆盖整个节目、延长替换音频超出 bed 的有限结束点，以及声道的频率限制同样按损失处理。
- **直接拒绝**：多个 programme、多个 bed、不完整或非 7.1.2 的 bed、极坐标对象、多声道 `Objects`、嵌套对象、
  HOA 等其他 `typeDefinition`、对象 block 之间有空隙或重叠，以及没有 `axml` / `chna` 的文件。
- **帧率**：ADM 不带帧率，Logic 的 DBMD 帧率也不读回；输出默认 24 fps，用 `with_frame_rate`（或 `mfadm convert --fps`）指定。
  DBMD 的创建者字段同样不读回，输出写本库自己的值。
- **DAMF 装不下的**：DAMF 没有节目名字段。节目名只是描述信息，写 DAMF 时直接省略、不按损失处理，
  所以 ADM 转 DAMF 再转回时节目名回到默认值。`screenRef` 写 DAMF 时则按损失处理。
  反过来，DAMF 里延伸到音频结尾之后的 ramp 在 ADM 中被截在结尾，读回是截短的 ramp 与当时的插值位置。
- **内存**：AXML 由 `macin-adm` 整体解析，内存随元数据规模增长。实测 57.8 MB 的 AXML（118 个对象、11.8 万个事件）
  峰值约 640 MiB；DAMF 读取则是流式的。

## 语义损失与诊断

默认 `LossPolicy::Reject`：无法精确表达的语义直接报错。只有选 `Allow` 才采用明确的降级，并记录在
`MappingDiagnostic` 里。

- ADM 不擅自映射未验证的 zones、elevation、trim、内容类别、耳机策略和连续 screen/depth factor。
- Logic DBMD 只写已验证的静态兼容字段，不宣称能保存动态耳机事件。
- 已验证的 DAMF 耳机值为 undefined / off / near / far；Mid 被显式拒绝或降为 undefined。
- ADM importance 量化为 0..10，非整十的分值会报告损失。
- 音频 24-bit 量化和所选时钟精度属于已声明的格式行为，不算损失。DAMF 不写节目名，同样不算损失。

## 不做什么

- 不做多 bed 制作、重采样、渲染或编码算法（首版）。
- ADM 读取只支持上面的 Atmos 子集；极坐标、HOA、多 bed 与嵌套对象不做转换。
- 不做渲染、聚类或编解码；不导出 C ABI。
- `mfadm` 不修改输入、不覆盖输出；转换的损失与写入一样按 `LossPolicy` 处理，默认拒绝。
- 不在文件里伪造创建工具来源：即使某个外部检查器因此拒绝，也保留真实创建者（见下文）。

## 开发与验收

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
# workspace 构建会经由 macin-format 打开 read，因此单独检查只写母版的配置：
cargo clippy --locked -p macin-atmos --all-targets -- -D warnings
cargo test --locked -p macin-atmos
```

`rust-toolchain.toml` 锁定 Rust 1.98。可以通过 `CARGO_TARGET_DIR` 或被忽略的 `.cargo/config.toml`
复用已有构建目录；不要复制消费方的 target 或媒体。

测试使用独立的 XML/WAVE 夹具，覆盖分块一致性、ramp、四种配置与两种采样率、原子失败，以及不创建巨型文件的
虚拟 4 GiB 边界检查。DAMF 读取的测试要求 DAMF 转 DAMF 三个成员逐字节复原、DAMF 转 ADM 与直接写 ADM 逐字节一致，
并覆盖手写的第三方 YAML 写法、声道重排、WAV 代替音频、各类损失与拒绝。ADM 读取的测试要求 ADM 转同一配置逐字节复原、
ADM 经 DAMF 再转回 ADM 逐字节一致，并覆盖只靠 CHNA 定义轨道的第三方写法、对象起止时间、整块插值、dB 与对象级增益、
声道重排、各类损失与拒绝，以及预读过或来自其他读取器的音频。原生验证平台为 macOS arm64 与 Windows x64，CI 在这两个平台上跑同一套检查。

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

**DAMF 读回**：加上 `--damf-format <名称>` 后，对 Conversion Tool 接受的每个输入，再让它输出一份 DAMF，
用 `mfadm convert` 读回并写成 Standard ADM。这份 DAMF 不是本库写的，正是读取器要面对的真实输入。
`<名称>` 是你的 Conversion Tool 版本里 `--output_format` 表示 DAMF 的取值（见它的 `--help`），脚本不猜测这个值。
读回要求能读出来，且写出的 ADM 与源文件 PCM 逐字节一致、采样率、帧数和对象数正确；读取时报告的损失字段与更新数
记入报告供人工核对，不作为验收条件。`mfadm` 默认从本仓库构建，也可以用 `--mfadm` 指定。
Conversion Tool 2.1.2 不支持输出 96 kHz DAMF，脚本识别这一明确错误后记录跳过原因，保留原有的 WAV 验收结果。
其他转换错误与读回错误仍使检查失败。

```sh
python scripts/check_interop.py --ear \
  --conversion-tool /path/to/cmdline_atmos_conversion_tool \
  --atmos-info /path/to/atmos_info \
  --damf-format <名称> \
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
