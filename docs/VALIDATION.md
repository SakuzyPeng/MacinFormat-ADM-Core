# 验证记录

前两节说明怎么验证：自动测试覆盖什么、外部工具检查器怎么跑。其后是按日期记录的实测结果。
生成媒体均为确定性合成信号，产物不进入 Git。

## 测试覆盖

`cargo test --locked --workspace` 只用仓库内独立的 XML/WAVE 夹具，不需要媒体或外部工具。

- **写入**：分块一致性、ramp、四种配置与两种采样率、原子失败，以及不创建巨型文件的虚拟 4 GiB 边界检查。
  1 项实际写入超过 4 GiB 的历史测试保持 ignored。
- **DAMF 读取**：DAMF 转 DAMF 三个成员逐字节复原、DAMF 转 ADM 与直接写 ADM 逐字节一致；覆盖手写的第三方 YAML 写法、
  声道重排、WAV 代替音频、各类损失与拒绝。
- **ADM 读取**：ADM 转同一配置逐字节复原、ADM 经 DAMF 再转回 ADM 逐字节一致；覆盖只靠 CHNA 定义轨道的第三方写法、
  对象起止时间、整块插值、dB 与对象级增益、声道重排、各类损失与拒绝，以及预读过或来自其他读取器的音频；
  另覆盖乱序 block、继承的命名空间、后续 block 校验和源文件被截断时的错误。
- **不开 `read` 的 `macin-atmos`**：workspace 构建会经由 `macin-format` 打开 `read`，所以只写母版的配置单独跑
  `cargo clippy --locked -p macin-atmos --all-targets -- -D warnings` 与 `cargo test --locked -p macin-atmos`。

已有实测覆盖 macOS arm64 与 Windows x64。CI（`.github/workflows/ci.yml`）在 macOS arm64、Windows x64 与 Linux x64
上跑同一套检查，另含互操作检查器、发布归档和产物核验的 Python 回归
（`python -B -m unittest discover -s scripts -p "test_*.py"`）。

## 互操作检查器用法

`scripts/check_interop.py` 用调用方提供的外部工具（以及可选的 mradm 读取器）验证合成输出。
同源读回只作为集成验证，不能替代独立的格式验收。已识别的工具限制在检查器里作为明确限制记录，
不把拒绝写成通过；未知失败仍使检查失败。

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

## 新库

日期：2026-11-14。

- macOS arm64：fmt、Clippy（所有 target、警告视为错误）、50 项测试通过；1 项实际写入超过 4 GiB 的历史测试保持 ignored。新增虚拟输出测试覆盖该边界而不占用巨型磁盘文件。
- Windows x64 / Rust MSVC 1.98：50 项通过、同一历史测试 ignored；采用代码提交 2fceb9dd3fc2d163ab1290a07f5be58fae233382。首次 checkout 的换行转换已按 .gitattributes 的 LF 策略刷新，资源哈希与锁定清单一致。
- 覆盖独立 XML/WAVE 输入、未知扩展回写、四种配置与两种采样率、分块不变性、独立 ramp、局部耳机更新、量化端点、微小 gain、原子失败和超大尺寸。

## DAMF 读取、转换与多份输出

日期：2026-11-14，Linux x64 / Rust 1.98；macOS 与 Windows 由 GitHub Actions 的 macos-15 / windows-2025 矩阵验证（见下）。

- workspace（`macin-atmos` 经 `macin-format` 打开 `read`）：fmt、Clippy（所有 target、警告视为错误）、82 项测试通过，1 项历史 4 GiB 测试保持 ignored。
- 不开 `read` 的 `macin-atmos`：Clippy 与 17 项测试通过；依赖图与改动前相同，不含 `saphyr-parser`。CI 增加了这一配置的独立步骤。
- `MultiWriter`：一次写出四种配置，每份与单独写出的结果逐字节一致；某个 target 在 `Reject` 下拒绝更新时所有 target 保持不变、可重试；发布中途失败时已发布的输出被撤回，不留暂存目录。
- DAMF 读取：home 与 3dof 转回同一配置时 `.atmos`、`.atmos.metadata`、`.atmos.audio` 三个成员逐字节复原；转 Standard/Logic/3dof 时与直接用同一模型写出的结果逐字节一致，损失诊断也相同。
- 手写的第三方写法：注释、flow/block 混用、引号风格、键顺序、bed ID 打乱、对象 ID 不连续、事件按时间交错、`sampleRate` 在 `events` 之后、`-.inf` 增益、`size3D`、缺省的 `rampLength`；按声道标记值核对了输出的声道顺序。
- WAV 代替音频：24-bit WAV 与原 CAF 转换结果逐字节一致；声道不足时报错并指明缺失的 DAMF ID。
- 规模：118 个对象、70.8 万个事件、60 秒 48 kHz 的 DAMF，一次转成 Standard、Logic、3dof 三份（约 1.1–1.4 GB 一份），release 构建 23 秒，峰值内存 12 MiB。
- 真实工具读回（2026-11-15，macOS arm64）：Conversion Tool 2.1.2_07765940 把 48 kHz Logic 与 home 转成 DAMF，
  `mfadm convert` 读回并写成 Standard，均保持 12 声道、96,000 帧、2 个对象与逐字节相同的 PCM，分别读到 1 条和 3 条更新。
  home 的耳机事件省略 `ID`，读取器已支持沿用前一个事件的 ID，覆盖初始状态折叠与更新迭代两遍扫描。
  Logic 来源的读取损失为 `unknown_field`，写出损失为 `depth_factor`。home 来源读取无损失，写出损失为
  `binaural_mode`、`depth_factor` 和 `head_tracking`，符合 Standard 的能力范围。
- 同次独立互操作矩阵的 8 项检查全部通过，使用 EBU EAR 2.1.0 与 atmos_info 校验对应配置。
  Conversion Tool 拒绝 96 kHz Logic/home 转 96 kHz DAMF，脚本识别其明确的采样率限制并记录跳过原因，保留 WAV 验收结果。
  未知转换错误与 DAMF 读回失败仍使验收失败，另有 2 项 Python 回归覆盖这两类情况。
- 本次修正的 workspace 格式与 Clippy 检查通过，114 项 Rust 测试通过，1 项历史 4 GiB 测试保持 ignored。

## ADM 读取与转换

日期：2026-11-15，Linux x64 / Rust 1.98；macOS 与 Windows 由 GitHub Actions 的 macos-15 / windows-2025 矩阵验证（见下）。

- workspace：fmt、Clippy（所有 target、警告视为错误）与全部测试通过；不开 `read` 的 `macin-atmos` 单独检查通过。
- `macin-adm` 新增按节点号只读访问原始元素的接口，不改动 `#[repr(C)]` 记录；越界节点号返回空值而不 panic。
- ADM → 同一配置：本库写出的 Standard 与 Logic（默认帧率与 DBMD 字段）读回再写，整个文件逐字节一致。
- ADM → DAMF → ADM：位置不超过 9 位小数、增益为 1 或静音、默认节目名时逐字节回到原文件；带 dB 增益时经 DAMF 的增益误差小于 1e-9。
- 手写的第三方 ADM：只在 CHNA 中定义轨道、bed 声道顺序颠倒、每个对象一个 content、对象 `start` / `duration`、未写 `jumpPosition` 的整块插值、dB 增益与对象级增益折算、`mute`；按声道标记值核对了输出声道顺序。
- 损失与拒绝：逐项覆盖[格式参考](REFERENCE.md#adm-读取与转换)列出的损失字段，以及极坐标、block 空隙、多个 programme、嵌套对象、非 7.1.2 bed 和没有 axml 的文件。
- 预读过的音频与来自 DAMF 读取器的音频，转换结果与直接转换逐字节相同。
- 规模：118 个对象、11.8 万个事件、10 秒的 ADM（AXML 57.8 MB），转 DAMF 用时 2.2 秒，峰值内存 642 MiB，约为 AXML 大小的 11 倍。同一 DAMF 经 ADM 再转回 DAMF，清单与音频逐字节一致；元数据里每个对象有一个事件不同，原因是其 ramp 延伸到音频结尾之后，ADM 只记录到结尾为止。
- 尚未进行：读取其他工具实际产出的 ADM BWF。上面的第三方写法是手写的。

## 跨平台 CI

GitHub Actions run 43717764695（代码提交 18031c8064f5dabe625be8b0b64785aeecd9dbdb，2026-11-15）在 macos-15 与
windows-2025 上全部通过：fmt、workspace Clippy（所有 target、警告视为错误）、workspace 测试、互操作检查器的
Python 回归，以及不开 `read` 的 `macin-atmos` Clippy 与测试。这次运行覆盖上面 DAMF 读取、多份输出、ADM 读取
各节的代码及其后续修正。

## 外部互操作

Dolby Atmos Conversion Tool 2.1.2_07765940、DME 随附 atmos_info、EBU EAR 2.1.0 与 mradm Release。

| 配置 | 48/96 kHz 的实际结果 | 工具限制 |
| --- | --- | --- |
| Standard BW64 | EAR 图/CHNA 校验与 mradm 导入通过 | 本机两款 Dolby 读取器要求 RIFF/RF64，不识别 BW64 |
| Logic RF64 | Conversion Tool 转换、EAR 校验、mradm 导入通过；转换后采样率/帧数保持，PCM 逐字节一致 | 默认 DBMD 创建来源会触发 atmos_info 来源检查。兼容来源值及应用实测结果见下文 |
| DAMF home | Conversion Tool 与 atmos_info 通过；转换后采样率/帧数保持，PCM 逐字节一致 | 无本次用例限制 |
| DAMF 3DoF | atmos_info 验证通过 | 本机 Conversion Tool 最高支持 DAMF 0.5，拒绝 0.6.0 |

检查器显式记录每个工具的拒绝及原因；只接受上表已识别的限制，未知失败仍使检查失败。
Standard 必须通过独立 EAR 验证，3DoF 必须通过 atmos_info，Logic/home 必须通过保持采样率与 PCM 的转换。
同源读回用于集成验收，不作为独立格式正确性依据。

## DBMD 创建来源自定义

日期：2026-11-15，macOS arm64。

- `mfadm convert --dbmd-originator` 与 `--dbmd-product` 支持分别设置 Logic DBMD 的 32/64 字节 ASCII 字段，
  ADM 与 DAMF 读取器同时提供 `with_dbmd_identity`。短帮助与完整帮助都说明 Conversion Tool 的来源提示与可用参数。
- CLI 回归覆盖 ADM/DAMF 两种输入、单独设置任一字段、同时设置、最大宽度与空字段，并检查 segment 9 校验和。
  超宽、非 ASCII 与控制字符在参数解析时拒绝。混合写出 Standard 与 DAMF 时，两者与使用默认 DBMD 参数的输出逐字节一致。
- Conversion Tool 2.1.2_07765940 的 16 组真实工具对照覆盖两种输入、48/96 kHz、默认值及三种字段设置组合。
  显式设置 `dbmd_originator = "Created using Dolby equipment"` 的 8 组均无 WARN/ERROR 日志。
  默认值与仅修改产品名的 8 组仍报告创建来源提示，退出码为 0。全部 16 组转换后的 PCM 均逐字节一致。
- workspace 格式与 Clippy 检查通过，117 项 Rust 测试与 2 项 Python 回归通过，1 项历史 4 GiB 测试保持 ignored。

## Logic Pro 与 Dolby Atmos Renderer 实机兼容验证

日期为 2026-11-15，生成环境为 macOS arm64，生成代码提交为 `73fc2c7`。
实测确认本次生成的两份母版兼容 Logic Pro 与 Dolby Atmos Renderer。

- 样本为同源合成测试信号，时长 2 秒、48 kHz、24-bit PCM、24 fps，包含一个 7.1.2 bed 和两个对象，
  其中一个对象带位置移动。输出为 `logic_48k24.wav`（Logic RF64 ADM BWF）与 DAMF Home 0.5.1 三件套
  `master.atmos`、`master.atmos.audio`、`master.atmos.metadata`。
- Logic 样本采用 `dbmd_originator = "Created using Dolby equipment"`，产品字段为 `MacinFormat ADM Core`。
- 两份均通过 DME 随附 `atmos_info --validate 1` 校验，Conversion Tool 2.1.2_07765940 转 WAV 成功，
  转换日志无 WARN/ERROR。转换前后均保持 12 声道、48 kHz、96,000 帧，PCM 逐字节一致。
- 两份原始输出的 PCM 同样逐字节一致，SHA-256 为
  `27a671d741556b728469479cc0a866bee5f12fb455780f0311735fc71d2bc1cb`。
  DAMF 经 `mfadm convert` 转回相同 DBMD 设置的 Logic 母版后，整个 WAV 与原样本逐字节一致，读写均无语义损失。

应用兼容结论对应上述样本与参数。

## 消费方迁移

- AC-4：全 workspace audio-decode 测试与 Clippy 通过；12 项真实向量 ADM/DAMF 集成回归通过。
- 同一 768K 向量迁移前后：Standard/Logic ADM 的 AXML、CHNA、DBMD（适用时）及 DAMF manifest/metadata 逐字节相同。三种输出的 25,920,000 字节 PCM 哈希均为 a685e79a7d370f30c5d19ae8ad3f3a4204cd5638cf6d2b9438bbfb20291cac37。
- 渲染端：Rust WAVE/Scene 相关测试、全 workspace Clippy、许可资源定位与哈希检查通过。Release 原生构建与 11 项定向 CTest 通过，公开库恰好导出 164 个 adm_*；FFI 头检查含 391 个函数、163 个结构及公开 C ABI 的 31 个结构。
- 聚类端：合成结果导出示例为 12 轨、96 kHz、1024 帧；四种配置均完成实际写入和读回/PCM 一致性检查。示例不宣称已实现聚类算法。

私有阶段消费方仅在迁移分支依赖固定 Git 提交。公开发布线与原 crates.io 发布检查保留。

- DAMF 局部耳机事件：已有独立检查器以新库生成的 home 包为输入，home/3DoF 验证通过；官方工具原生 DAMF 往返保持连续字段命令与耳机事件时间线。ADM 转换不作为 ramp 保真判据。
- GitHub Actions 的 macOS/Windows 矩阵在代码提交 2fceb9dd3fc2d163ab1290a07f5be58fae233382 上通过；run 43614066686。

## ADM 流式读取内存对照

日期：2026-11-15，macOS arm64 / Rust 1.98，release 构建。

本次优化针对 `macin-atmos::adm::AdmReader` 与 `mfadm convert`。通用 `Document` / `Snapshot`、
语义回写与 `mfadm adm` 检查命令保留原有整体解析方式。

- 打开时先扫描 AXML，保留引用结构、每个声道的首个 block、文件范围与结束边界；随后逐块验证选中的时间线，
  得到初始状态、更新计数和诊断。调用 `updates()` 时重新流式读取，未建立全部更新的列表。
- 有序声道不建立逐 block 索引。乱序声道使用每个 block 的采样起点和文件范围构建稳定排序索引，
  内存会随该声道的 block 数增长；索引通过共享引用复用。没有新增读取缓存或临时文件。
- `updates()` / `updates_for_frames()` 改为可失败迭代器，`update_count()` / `update_count_for_frames()`
  无需重新扫描。源文件必须保持可读且不变；提前 EOF 不会被拼接的 XML 结束标签掩盖。

合成输入使用 118 个单轨 Objects、48 kHz、24-bit 静音 PCM，音频统一为 6,000 帧。
每个对象有 100、1,000 或 6,000 个连续的单采样 block，X 坐标交替为 ±0.125，均匀尺寸为 0.25。
输入没有 bed，转成 Standard ADM 时补静音 bed。以 `/usr/bin/time -l` 读取转换进程峰值 RSS；
耗时为对照脚本的转换及输出 SHA-256 校验总时间，单次运行，不作为稳定的吞吐基准。
AXML 的 MB 为十进制，RSS 的 MiB 为二进制。

| block 总数 | AXML 大小 | 改进前峰值 | 改进后峰值 | 改进前耗时 | 改进后耗时 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 11,800 | 4.79 MB | 64.5 MiB | 18.2 MiB | 0.164 秒 | 0.337 秒 |
| 118,000 | 47.43 MB | 557.7 MiB | 19.4 MiB | 1.266 秒 | 2.999 秒 |
| 708,000 | 284.90 MB | 未运行 | 18.8 MiB | 未运行 | 18.210 秒 |

前两组转换输出的更新计数分别为 11,800、118,000，改进前后整个输出文件逐字节相同。
SHA-256 分别为 `16695cc0b97a3e9e5127bcb3a5a09f47f8acfe4ec09298c402252a84c2c08dee`、
`80961d91820401956b4b2631d523146ea050355f170101fc717ec8324f9b54a5`。
第三组输出 707,882 条更新；末块恰好到达音频末尾，不额外提交停用事件。

回归覆盖原有 Standard/Logic 逐字节往返、ADM/DAMF 转换、替换音频结束边界，以及新增的乱序稳定排序、
继承的默认命名空间与前缀、外部命名空间元素隔离、后续 block 校验、流读取被截断后返回结构化错误并终止。
声道片段投影还与完整文档投影比较，并覆盖内置 common definitions。

最终检查：`cargo fmt --all -- --check`、workspace 全 target Clippy（`-D warnings`）、
`cargo test --locked --workspace` 全部通过，122 项测试通过，1 项历史真实 4 GiB 测试保持 ignored；
不开 `read` feature 的检查与 release CLI 构建通过。基准生成的媒体文件已清理。
