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
- **显式近似**：逐字段授权、默认拒绝、与 `Allow` 的隔离、双向转换、动态归零与缺省值恢复、诊断分类与去重，
  以及多目标写入遇到冲突后的原子拒绝和重试；CLI 覆盖重复开关、JSON / 文本诊断和未知开关值。
- **原生映射**：DBMD 字节布局与完整性检查、静态与动态通道参数的边界、区域预设及严格边界匹配、
  bed 更新流与多目标原子性、节目范围裁切、content 分组及独立格式名称；原生 ADM 参数和分层元数据往返，
  WAVE/CAF 附加块的条件透传、预算、布局变化及跨格式损失检查。

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

## Dolby Conversion Tool 元数据映射对照

日期：2026-11-17，macOS 27.2.1 arm64；代码上下文为 `92b09ee`。
实测工具为 `cmdline_atmos_conversion_tool v2.1.2_07765940`，Python 3.13。
复用已有 release 合成生成器和 Cargo target，没有复制媒体或创建独立构建缓存。

- 工具 SHA-256：`21229c3d2855bf68f604ed4915e9832eac2c4e4fdd33e4bce3e3e7dcfa6eae21`。
- 合成生成器 SHA-256：`0e82b002fe279d28bf6bf9241ea688f95adff956c73d70ae7c8ebcdf0ade6511`。
- 夹具为 2 秒、48 kHz、24-bit、12 声道，包含完整 7.1.2 bed 和两个对象；每次修改一个字段或一组相关字段。
  动态对照在第 24,000 个采样改变状态。比较 DAMF→DAMF、DAMF→ADM BWF→DAMF 和 ADM→ADM/DAMF。
- 110 个场景共记录 281 次转换：273 次退出码为 0，8 次拒绝；退出码为 0 的结果中有 8 次带校验诊断，
  这些带诊断的输出不记作格式验收通过。位置和增益作为阳性对照，另核对 AXML、DBMD、事件时间线和 PCM 哈希。
- DAMF 元数据夹具保持原生缩进和行内向量，`off` 保持字符串。未通过位置阳性对照的 YAML 重排试验已排除，
  避免将夹具未被读入误判成参数不受支持。

以下是该版本工具的实际行为，不改变本库当前的能力声明，也不代表所有 Dolby 产品或所有 ADM 配置的行为。

| 实测项目 | 结果 |
| --- | --- |
| 静态头追 | `scene relative`、`head relative`、`undefined` 均经 DBMD segment 10 保留并读回；工具不生成 XML `headLocked` |
| 输入 XML `headLocked` | 对象级和 block 级真值均不保留；显式 BS.2076-2 仍如此。XML=0 与 DBMD=head relative 冲突时，输出采用 DBMD 的 head relative |
| 静态耳机模式 | `undefined` / `off` / `near` / `far` 经 DBMD 保留；`mid` 被明确拒绝，日志为 `Unsupported binauralRenderMode: mid` |
| 静态 `trimBypass` | 对象和 bed 均可经 DBMD 保留 |
| 动态头追、耳机模式、`trimBypass` | DAMF→DAMF 保留初始状态及第 24,000 个采样的变化；经 ADM BWF 往返只剩初始状态，没有对应损失警告 |
| `zones` / `elevation` | 五种非默认区域预设转换成 `zoneExclusion`；`elevation=false` 使用 ZB/ZT 排除区。预设、组合及动态更新均能往返 |
| 任意 `zoneExclusion` | 自定义名称 `left` 得到 `unrecognized zone named left` 校验错误；工具仍产生输出并删除该区，不能按预设成功外推任意区域 |
| `diffuse` / `decorr` | ADM `diffuse=0.5` 或 `1` 在输出中消失，DAMF `decorr=0`；DAMF `decorr=0.5` 及动态 `0.3→0.7` 在原生 DAMF 转换中也归零，无对应损失警告 |
| `screenFactor` / `depthFactor` | 实测 `screenFactor=0.4` 或 `1` 归零，`depthFactor=1.2` 回到 `0.25`；原生 DAMF 转换中已经发生 |
| `dialog` / `music` / `importance` | 对象 `dialog=1`、`music=1` 回到 `-1`；`importance=0`、`0.3`、`0.35` 回到 `1`。ADM 对象、pack、block 的非默认 importance 同样不保留 |
| ADM 其他对象参数 | `objectDivergence`、`positionOffset`、`screenRef`、`screenEdgeLock`、`headphoneVirtualise` 不保留；`channelLock` 保留布尔，丢弃 `maxDistance` |
| bed 的 gain / active | DAMF→DAMF 保留 `gain=-3 dB` / `active=false`；经 ADM 回到 `gain=0` / `active=true`，PCM 哈希不变，没有烘焙到音频。ADM bed `gain=0.5` 同样未保留 |
| 清单与帧率 | 25 fps、`offset=0.1`、`scBedConfiguration=[2]`、相移 true、`bedDistribution=true`、`LtRt_ProLogic` / `LtRt_PLII` 均可经 ADM BWF 往返；offset 对应 programme 的 start/end |
| warp 模式 | 官方 CLI 分别生成 `LoRo` / `ProLogicIIx` / `warping` / `normal`，均可往返；`None` 文本被该工具拒绝 |
| 自定义 `trimMode` | `centerTrim`、`surroundTrim`、`heightTrim` 及两个 `frontBackBalance` 字段能够保留；经 ADM 后平衡值出现量化，实测 `0.8→0.803149581`、`0.9→0.897637784` |
| `simplified` / `screenSizeRatio` | `simplified=true` 被明确拒绝；`screenSizeRatio=2` 在输出清单中省略 |
| programme 结束点 | 2 秒 PCM、programme/object/block 一致结束于 1 秒时，输出为 48,000 帧且 end=1 秒，无校验诊断；仅改 programme 的不一致对照会产生校验错误，但仍裁到 1 秒 |
| content 结构 | ADM→ADM 保留两个 content 的分组和名称；独立自定义的 pack/channel/stream/track 名称则重建 |
| 描述字段与附加块 | 实测语言、标签、响度、声道 frequency、interact、disableDucking 不保留；合法单一 content 对白分类重写为默认混合类别。未知 XML 扩展、WAVE `bext` / `iXML`、CAF `info` 也未保留 |

本次对照为接通静态 DBMD 参数、区域预设、elevation、帧率和清单设置提供了外部证据。
同时发现本库 `HeadTracking` 缺少工具使用的 `undefined`，`WarpMode` 的额外取值和拼写也需另行处理。
通用 ADM 的 `headLocked` 语义与这版杜比实际采用的 DBMD 通路应分别验证；不能只凭相近名称宣布工具间无损映射。
杜比未保留某字段，也不构成本库静默丢弃该字段的依据。

全部场景、输入变体、输出结构、日志、DBMD 差分和 PCM 哈希保存在本地被忽略的
`artifacts/audits/2026-11-17-dolby/`，合成音频已清理。此次仅增加验证记录，未修改读写实现。

## 按字段近似映射的回归验证

日期：2026-11-17，macOS arm64 / Rust 1.98.0 / Python 3.13；代码基线为 `92b09ee` 加本次工作树变更。

- 合成夹具覆盖 `diffuse-decorrelation` 与 `screen-ref-factor` 的逐项授权；默认拒绝仍生效，选择一项不会放开另一项。
  单独 `Allow` 不启用这两种近似，未选择近似的 screenFactor 不隐式折成 screenRef。
- Standard / Logic 写出保留近似 diffuse 的初始值、动态数值和归零；手写 ADM block 缺省 diffuse 时回到 0，
  经 DAMF 再返回 ADM 后，整数采样时间点及离散值保持。screenRef / screenFactor 的双向更新覆盖再次关闭屏幕参考。
- 近似诊断包含首次数值和映射方向，按轨道、字段去重；JSON 与文本区分近似和损失。
  原生目标值冲突在 Reject 下拒绝，Allow 保留原生目标值；多目标预校验失败后的重试不遗留写入或诊断。
- CLI 覆盖重复 `--approximate`、两个选项独立生效、无 `--allow-loss` 的近似转换，以及未知名称的参数错误。
  不开 `read` 的配置单独覆盖写出与冲突策略；共享 `#[repr(C)]` 记录布局未修改。

最终检查：fmt、workspace 与独立只写配置的 Clippy（`-D warnings`）通过；workspace 131 项测试通过，
1 项历史真实 4 GiB 测试保持 ignored；独立 `macin-atmos` 24 项、Python 13 项测试通过。

## 已验证映射的实现与独立复查

日期：2026-11-17，macOS 27.2.1 arm64 / Rust 1.98.0 / Python 3.13。
代码基线为 `92b09ee6afce3ed6d950e3073e904693273f3eec` 加本次工作树变更；外部工具为
`cmdline_atmos_conversion_tool v2.1.2_07765940`。本节记录前述工具对照之后的实现结果。

- 接通 Logic 的静态 DBMD 头追、耳机模式、trimBypass、帧率与节目级设置；支持 undefined 头追和额外 warp 模式。
  Standard 使用原生 XML headLocked，支持对象动态头锁；区域预设和 elevation 在两种 ADM 输出中支持动态更新。
- DAMF 的 bed 有独立初始状态与更新流，不占用对象 TrackId；gain、active 等事件按采样时间保留。
  Logic 动态头追/耳机/trimBypass 以及 ADM 无法表达的 bed 状态仍默认拒绝；Allow 保留初始静态值并诊断。
- ADM 同格式转换保留 content 分组、对白分类及独立 pack/channel/stream/track 名称；原音频转换遵守节目结束点，
  不再将声明的短节目扩为整份 PCM 长度。替换音频仍保留有限的来源停用边界。
- 非支持属性、pack importance、位置边界、屏幕边缘锁定和附加容器元数据不再静默丢弃。
  CAF 检查 data 前后的附加块；DBMD 的坏校验和、无效长度、声道数不匹配等损坏输入始终拒绝。

补充字节对照确认，LoRo 下 DBMD segment 9 的偏移 152 为 `0x03` / `0x43` 时分别是显式 false/true
的 bedDistribution，`0x83` / `0xc3` 则未指定。此前低层模板的 `0x83` 不能用于表达显式 false；
新高层入口区分这三种状态，旧低层入口保留其兼容契约。
布局码 0..7 对应偏移 135 的八个位，码 8 对应偏移 136 的 bit 2。全零 DBMD 掩码会导出缺省配置的 DAMF，
但工具再读缺省 DAMF 时采用 `[3]`，不能当作可逆映射：库保留未指定的原生状态，跨到 DAMF 时按损失处理。
四个 trim 槽位与正负平衡值也经过独立对照，平衡量化符合声明的 1/127 存储精度。

自动检查：fmt、workspace 与独立只写配置的 Clippy（`-D warnings`）通过；workspace 146 项测试通过，
1 项历史真实 4 GiB 测试保持 ignored；独立 `macin-atmos` 29 项、Python 13 项测试通过。
共享 `#[repr(C)]` 记录未修改，没有引入依赖、独立构建缓存或真实媒体夹具。

最终 CLI 的独立复查包含 16 个场景、50 次 mfadm 与 96 次 Dolby 转换调用。比较两条路径：
一条完全经 Dolby 工具，另一条经本库写入或导入后再由 Dolby 读取。静态通道参数、五种区域预设、
elevation 及动态组合、完整布局集合、warp 模式、帧率/offset/下混/相移/trim，以及 DAMF bed 事件均符合对照。
数值按原生精度比较（容差 1e-6），更新采样位置与 PCM SHA-256 核对一致。
两次 mfadm 严格拒绝是预期的动态 DBMD / ADM bed 能力边界；显式 Allow 后的诊断和初始值保留也通过检查。

实测的最终 mfadm SHA-256 为
`51959db903bc66a31f7c260798d77df7eb4194051e15e297de0aa60e9001fd73`。
原始结果、字节探针、六项检查与逐次日志保存在本地被忽略的
`artifacts/audits/2026-11-17-mapping-implementation/`；全部合成媒体在检查结束后清理。

## 剩余原生 ADM 字段与附加块保留

2026-11-17 在 macOS 27.2.1 arm64 实测。代码基线为 `92b09ee6afce3ed6d950e3073e904693273f3eec` 加本次工作树变更。
使用 Rust 1.98.0、Python 3.13.2 和 `cmdline_atmos_conversion_tool v2.1.2_07765940`。

用 48 kHz、12 声道、96,000 帧的合成母版做了 13 组对照。每组分别转换到 Standard 和 Logic，再由 mfadm 读回各写一次。
52 次转换均成功。逐组比较了被测字段的值和层级、测试时间线以及 PCM 字节，均与来源相符。

另外进行了 14 次 Dolby 转换。先用同规格合成母版生成一个参考 DBMD，再将各组 Logic 输出的五位时钟 AXML、CHNA、PCM 装入工具可读的 RIFF，
配上该参考 DBMD。工具此前直接拒绝 Standard BW64 和九位时钟输入，这两项属于容器和时钟限制。
13 组最终控制样本都生成了输出，日志均无校验诊断，输出 PCM 的 SHA-256 均与输入相同。逐项读取工具输出得到以下结果。

| 控制样本 | 本库 ADM 往返 | Dolby 2.1.2 输出 |
| --- | --- | --- |
| diffuse、divergence | 保留 0.25、0.75 及归零时间，保留 divergence 0.4 和 positionRange 0.2 | diffuse 回到 0，divergence 消失 |
| channelLock 距离（两组） | 0.3 与 3.125 分别在原 block 保留 | 两组都保留 channelLock=1，均省略 maxDistance |
| 对象偏移、三个层级的 importance | 保留 X 偏移 0.123456789012345，object/pack/block 分别保留 3/6/8 | 偏移消失，三个层级均回到默认 10 |
| 语言和标签、响度、参考屏幕 | 保留对应层级、响度值、屏幕宽高比 2.35 与对象边缘锁定 | 测试的属性和元素均省略 |
| 固定 bed 的位置 | 保留备用 X=-0.875、最小边界 -0.95 和左边缘锁定 | 回到规范左声道位置 [-1,1,0]，边界和锁定消失 |
| 交互和 ducking、声道频率、附加块 | 保留交互范围、禁用 ducking、highPass 80 Hz 与 bext/iXML 原始载荷 | interact 和 disableDucking 回到 0，范围、频率和两个附加块消失 |

自动回归还覆盖了高精度数值、独立 ramp 被离散事件分割、归零和缺省恢复、近似与显式目标值冲突，以及多目标拒绝后的重试。
检查了标准 aspectRatio 属性与旧子元素、旧 channelFrequency、仅由对象指定的 pack、静态 bed 极坐标和 Cartesian 坐标，以及几何发生变化的采样位置。
对语义无法保留的目标，CLI 把字段和轨道写入相应输出的诊断。覆盖了换音导致的响度失效、缺少原 CAF 时使用 WAV、
CAF info 位于 data 前后、同名 LIST 的不同子类型、声道重排、预算耗尽和无效重复块。

仓库六项检查通过。工作区 163 项测试通过，1 项真实 4 GiB 写盘测试保持 ignored。独立只写配置 32 项、Python 13 项测试通过。
两种配置的 Clippy 均通过 `-D warnings`。共享 C 记录布局保持原样，构建复用了已有 Cargo target。

最终 mfadm SHA-256 为 `597ab41772ba97b403374891b2c8a418a72600395d64bccb1affbff27f3fbce2`。
脚本、逐字段对照、PCM 哈希和检查日志保存在被忽略的 `artifacts/audits/2026-11-17-native-preservation/`，合成媒体已清理。

## 原审计 ADM 入口的同格式复核

本节保留修复前的测量值。相同夹具在新实现上的结果见[同格式缺口修复复测](#同格式缺口修复复测)。

2026-11-17，在 macOS 27.2.1 arm64 用当前 mfadm 0.1.0 复核。代码为 `92b09ee` 加工作树变更，
CLI 的 SHA-256 为 `597ab41772ba97b403374891b2c8a418a72600395d64bccb1affbff27f3fbce2`。
原审计的 A01–A20 是 ADM 读取侧的 20 个功能组。此次以 Standard BW64 为输入和输出，沿用原音频，关闭所有近似，比较各组所列字段的语义值。

| 结果 | 功能组 | 测到的行为 |
| --- | --- | --- |
| 已保留，13 组 | A01、A03–A13、A16 | diffuse、锁定距离、对象 headLocked、对象偏移与 importance、对白分类、响度、参考屏幕、语言、标签、节目起点、声道频率和 bed 对象对白分类均能写回。包含 block 覆盖对象头锁，以及旧 channelFrequency 写为 frequency 的样本。 |
| 部分保留，5 组 | A02、A15、A17、A18、A20 | Cartesian divergence 值与 positionRange 保留，旧 Cartesian 输入上的 azimuthRange 无法写回。bed 对象层 importance 保留，block 的非默认值丢失。bed 静态头锁和几何保留，动态变化与额外扬声器标签丢失。interact 保留，自定义区域和 headphoneVirtualise 丢失。 |
| 非默认值未保留，2 组 | A14、A19 | bed 声道 gain=0.5 回到 1，bed 对象 mute=1 消失。50 ms 结束的 bed 在 100 ms 节目中扩到 100 ms。 |

以上“丢失”项均实际检查了两种策略。默认模式拒绝且没有发布目标，Allow 完成转换并报告对应字段。另测普通对象 gain=0.5、mute=1 会分别折为 block gain=0.5、0，输出不再含原对象级元素，两种转换的损失列表都为空。

在 A01–A20 之外，给 audioStreamFormat、audioTrackFormat、audioTrackUID 分别添加供应商命名空间属性。三次默认转换都成功，输出均省略该属性，读取和写出诊断均为空。另测 DAMF→DAMF 的 bed snap=true、对象 binauralRenderMode=mid，默认均拒绝，Allow 分别报告 bed_snap、binaural_mode 并降级。

共 36 个短合成场景、48 次受测转换，另有两次转换用于建立基础夹具。复核脚本、实际输出值、退出状态及诊断保存在 `artifacts/audits/2026-11-17-current-readback/`，音频已清理。

## 同格式缺口修复复测

2026-11-17，在 macOS 27.2.1 arm64 实测。代码为 `92b09ee6afce3ed6d950e3073e904693273f3eec` 加当前工作树，工具为 Rust 1.98.0、Python 3.13.2 和 Dolby Conversion Tool `2.1.2_07765940`。最终 mfadm SHA-256 为 `2e4078e22b8f92f35f1ac3922a7b3b74c710efc41217ca6926238f94f2edbe6f`。

沿用上节 36 个短合成场景和相同的观察点，逐项读取输出 XML 或 DAMF YAML。34 个场景默认保留。旧 Cartesian azimuthRange 和 DAMF mid 两组在各自的 `--preserve-literal` 开关下保留原值，诊断类型均为 `preserved_literal`。两组也分别检查了默认拒绝以及单独 Allow 的降级结果。该组有 40 次主转换，另有 2 次 DAMF 再读回写和 2 次基础夹具准备。

| 上次观察到的缺口 | 同格式实测 |
| --- | --- |
| bed 增益、静音、优先级与时段 | block 的 gain=0.5、importance=3、对象层 mute=1 都写回原层级。50 ms 的 bed 写回仍为 50 ms，节目和 PCM 仍为 100 ms。普通对象的 gain/mute 也保留在对象层。 |
| 动态 bed 与扬声器标签 | 在第 2,400 个采样处发生的头锁或位置变化得到保留。额外 speakerLabel 与原标签按顺序写回。 |
| 区域与耳机虚拟化 | 自定义区域、headphoneVirtualise 的 bypass 与 DRR 均写回。回归另测区域名称、全部 Cartesian/极坐标边界、多个区域、动态更新与缺省恢复。 |
| 轨道扩展属性 | audioStreamFormat、audioTrackFormat、audioTrackUID 三处供应商属性在默认转换中均保留。回归还检查了祖先声明、局部前缀重绑定、Unicode 局部名、含 QName 的值及 XML 转义。 |
| DAMF bed 与受限拼写 | bed snap=true 默认保留。选择原值开关后，DAMF 对象 mid 和旧 Cartesian 方位角范围分别原样写出。额外回归覆盖全部十项可选 bed 渲染属性的继承、清除和带 ramp 更新。 |

针对目标能力，另测了 bed 活动窗口、空隙与对象级增益到 DAMF 的转换。活动窗口的起止采样正确，对象和 bed 各自的增益仅合并一次。ADM 保留重叠的原生 bed 块，向严格 DAMF 目标提交重叠块会拒绝。多目标预校验失败后重试，没有残留块或诊断。Logic 的 DBMD 仍只承载已经验证的静态通道设置。

本机六项检查均通过。工作区 170 项测试通过，1 项实际 4 GiB 写盘测试保持 ignored。独立只写 `macin-atmos` 33 项、Python 13 项测试通过。共享 C 记录布局保持原样。

另用本库的输出做了十组 Dolby 对照，另有一次参考母版生成。ADM 工具输入采用 RIFF、同规格合成母版的参考 DBMD 和五位时钟，所有测试时点在转换时均可精确表示。六组产生输出且无校验诊断，但测试的 bed 增益/静音/优先级、动态 bed 头锁、耳机虚拟化、供应商属性、旧角度范围或 DAMF bed snap 被省略或重写。两组带校验错误仍生成文件：短 bed 被扩展，自定义区域被省略。这两组未计入通过。带额外 `Alternate` 标签的 bed 以及使用 mid 的 DAMF 被该版本工具拒绝。八份实际生成的输出均核对了 PCM 哈希。

逐项输出、命令、工具日志与检查结果在 `artifacts/audits/2026-11-17-remaining-implementation/`。合成媒体已清理。

## 双耳中距离的显式值与默认值

2026-11-17 在 macOS 27.2.1 arm64 实测。基线为 `1fc19d8` 加本次修正，使用 Dolby Atmos Conversion Tool `2.1.2_07765940`、MediaInfoLib 26.01、Rust 1.98.0 和 Python 3.13.2。合成母版为 48 kHz、12 声道、96,000 帧。

| 对照输入 | 实测输出 |
| --- | --- |
| DAMF 首事件省略 binauralRenderMode，或显式写 undefined | 两组经 Dolby 转换后，普通 bed 声道和对象都写 undefined，DBMD 低三位为 4。LFE 都写 off，低三位为 0。near、far、off 对照分别得到 1、2、0。 |
| DAMF 对象写字面 mid | 转 DAMF 和转 ADM 都以 255 退出，日志为 `Unsupported binauralRenderMode: mid`，没有生成输出文件。 |
| DBMD segment 10 的同一普通声道分别取 3、4 | 保持头追和其他位不变，改写后重算校验和。MediaInfo 分别显示 Mid、Not Indicated (Mid)。Dolby 读取两种合法校验和的输入后都导出 undefined，再写 DBMD 时都采用 4。把 3 放在 LFE 上时，Dolby 导出 off。 |
| 对象先写 near，采样 24,000 处的事件只更新 snap，或同时写 undefined | Dolby 的 DAMF 输出分别省略后续事件的耳机字段、显式写入 undefined。前者沿用 near，后者重置到默认。 |

对三组不同的 DBMD 码位排列，把同一载荷放入 PCM 和声道布局一致的合成 Logic 容器，分别用 mfadm 默认严格模式读回再写两次。普通 bed 和对象的 3、4，以及对照 LFE 原始码均逐字节保留，PCM 一致，无映射诊断。这个对照隔离了 AXML 结构，直接读 Dolby 工具生成的参考 AXML 还有独立的 stream→pack 引用限制。测试观测文件元数据，渲染默认值的依据见[Logic Pro 用户指南](https://support.apple.com/zh-cn/guide/logicpro/lgcp789f000d/mac)和[Dolby Renderer 指南](https://professional.dolby.com/siteassets/content-creation/dolby-atmos/dolby_atmos_renderer_guide.pdf)。

共 9 个场景、19 次 Dolby 转换和 6 次 mfadm 转换。自动回归另测了保留码 5 的拒绝与诊断，以及显式 Mid 转便携 DAMF 时显式标记的损失。仓库六项检查通过，工作区 171 项通过、1 项真实 4 GiB 测试保持 ignored，独立只写配置 34 项、Python 13 项通过。最终 mfadm SHA-256 为 `d16f4aa4921d875b3c92889e88dd883258e695ca03ace29d6c32b8dad0044d2d`。对照脚本和结果存于 `artifacts/audits/2026-11-17-binaural-mid/`，合成媒体已清理。
