# MacinFormat-ADM-Core

通用 ADM/BWF 读写与 Atmos 母版写入 · Rust 2024 · MSRV 1.98 · MIT

本仓库统一文件格式能力，供渲染、解码导出、对象聚类与编码工具使用。各消费方拥有自己的场景与算法模型。
初版为私有 Git 依赖，所有 crate 设置 `publish = false`。

| crate | 入口与职责 |
| --- | --- |
| `macin-adm` | `Document`、`Snapshot`、`Patch`；XML、引用、Objects/DirectSpeakers/HOA 投影、生成与保留原文的回写 |
| `macin-bwf` | `Reader`、`ChunkReader`、`Writer`、`Chna`；RIFF/RF64/BW64、PCM 与容器编辑 |
| `macin-atmos` | `MasterSpec`、`MasterWriter`；来源无关的 ADM BWF / DAMF 母版写入 |

基础库禁止 unsafe，不导出 C ABI，不依赖渲染器、AC-4、聚类算法或规范派生表。
ADM/BWF 实现迁自 MacinRender-ADM-Core，Atmos 序列化承接 MacinDecode-AC4-Core 的已验证写侧。
来源提交与哈希见 [SOURCES.json](docs/SOURCES.json)；common definitions 的 Apache-2.0 许可证和来源随资源保留。

## 母版写入

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

- 固定 bed 顺序为 L、R、C、LFE、Lss、Rss、Lrs、Rrs、Lts、Rts；可以有真实信号或显式静音。最多 118 个对象，槽位创建时固定。
- 支持 48/96 kHz、24-bit PCM，默认 48 kHz / 24 fps / Standard BW64。TrackId 是调用方身份，不是文件轨号。
- 接受交织或平面 normalized f32 和已量化 S24LE。浮点乘 2^23、就近取整（中点远离零），正端点饱和至 8388607；不加抖动、不自动归一化，非有限值和超出 ±1 的样本报错。S24LE 原字节保留。
- ObjectUpdate 使用绝对整数采样位置。块内更新必须落在当前块内；已有完整时间线可预先通过 schedule_updates 提交，同一轨道不允许时间倒退。
- 连续属性更新显式提供 ramp_samples（0 表示立即变化）；纯离散更新使用 None，不重启位置、增益和尺寸的既有 ramp。属性缺省表示继承。
- PCM 按块借用并直接写入临时成品，元数据碎片位于同目录暂存文件。finish 追加 AXML、回填长度并原子提交；不覆盖已存在目标。Drop 不提交，负责清理任务自己的暂存文件。
- 低层 metadata / containers 入口供已完成语义映射的批量适配器使用；自动能力检查、诊断与提交事务由 MasterWriter 提供。

| 配置 | 容器与时钟 | 已验证的专属能力 |
| --- | --- | --- |
| Standard | BW64，ADM 九位时钟 | Objects 位置、gain、extent、snap、screenRef、离散 importance |
| Logic | RF64，ADM 五位时钟 | 相同 ADM 属性，固定兼容 DBMD 与实际轨数、请求帧率、创建工具字段 |
| DamfHome | DAMF 0.5.1，S24LE CAF | 单 bed、对象事件、区域、trimBypass、screen/depth factor、内容类别与耳机策略 |
| DamfThreeDof | DAMF 0.6.0 / 3dof | 相同音频与事件；不强制改变对象头追策略 |

interpolationLength 保留九位来源精度，不随 Logic 时钟降低精度。ADM 文件的 ds64 固定为首 chunk，
chna 在音频之前，流式 AXML 在音频之后；依据 ITU-R BS.2088-2 Annex 1 §2.1、§4、§5。

LossPolicy::Reject 默认拒绝无法精确表达的语义；Allow 才采用明确降级并记录 MappingDiagnostic。
ADM 不擅自映射未验证的 zones、elevation、trim、内容类别、耳机策略和连续 screen/depth factor。
Logic DBMD 只使用已验证的静态兼容字段，不宣称能保存动态耳机事件。已验证 DAMF 耳机值为
undefined/off/near/far；Mid 显式拒绝或降为 undefined。ADM importance 会量化为 0..10，非整十分值报告损失。
音频 24-bit 量化和选定时钟精度属于已声明的格式行为。

通用 ADM 读取不受上述 Atmos 写入子集限制。首版不增加 DAMF 读取、多 bed 制作、重采样、渲染或编码算法。

## 开发与验收

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo run -p macin-atmos --example synthetic_master -- result.wav standard 96000
```

通过 CARGO_TARGET_DIR 或被忽略的 .cargo/config.toml 复用已有构建目录；不复制消费方的 target 或媒体。
测试包括独立 XML/WAVE 夹具、分块一致性、ramp、四种配置和两种采样率、原子失败，以及不创建巨型文件的
虚拟 4 GiB 边界检查。原生验证平台为 macOS arm64 与 Windows x64。

scripts/check_interop.py 使用调用方提供的 Dolby Conversion Tool 和可选 mradm 读取器验证合成输出。
同源读回只作为集成验证，不能替代独立格式验收。验证记录见 [VALIDATION.md](docs/VALIDATION.md)。

### 本机工具兼容性

当前 Conversion Tool 2.1.2 接受 Logic RF64 与 DAMF home，48/96 kHz 转换保持 PCM 字节。
它不识别 BW64，且 DAMF 版本上限为 0.5；BW64 由 EAR 校验，3DoF 由较新的 atmos_info 验证。
atmos_info 对自行生成的 RF64 还执行创建工具来源检查，库不会因此改写为 Dolby 创建者。
这些边界在独立检查器中作为明确的工具限制记录，不把拒绝写成通过。

完整矩阵需要同一 Python 环境内安装 EBU EAR，并提供外部工具路径：

    python scripts/check_interop.py --ear --conversion-tool /path/to/cmdline_atmos_conversion_tool --atmos-info /path/to/atmos_info --report artifacts/interop.json
