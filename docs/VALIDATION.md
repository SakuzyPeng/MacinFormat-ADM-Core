# 验证记录

日期：2026-11-14。生成媒体均为确定性合成信号，产物不进入 Git。

## 新库

- macOS arm64：fmt、Clippy（所有 target、警告视为错误）、50 项测试通过；1 项实际写入超过 4 GiB 的历史测试保持 ignored。新增虚拟输出测试覆盖该边界而不占用巨型磁盘文件。
- Windows x64 / Rust MSVC 1.98：50 项通过、同一历史测试 ignored；采用代码提交 2fceb9dd3fc2d163ab1290a07f5be58fae233382。首次 checkout 的换行转换已按 .gitattributes 的 LF 策略刷新，资源哈希与锁定清单一致。
- 覆盖独立 XML/WAVE 输入、未知扩展回写、四种配置与两种采样率、分块不变性、独立 ramp、局部耳机更新、量化端点、微小 gain、原子失败和超大尺寸。

## DAMF 读取、转换与多份输出

日期：2026-11-14，Linux x64 / Rust 1.98；macOS arm64 与 Windows x64 待 CI。

- workspace（`macin-atmos` 经 `macin-format` 打开 `read`）：fmt、Clippy（所有 target、警告视为错误）、82 项测试通过，1 项历史 4 GiB 测试保持 ignored。
- 不开 `read` 的 `macin-atmos`：Clippy 与 17 项测试通过；依赖图与改动前相同，不含 `saphyr-parser`。CI 增加了这一配置的独立步骤。
- `MultiWriter`：一次写出四种配置，每份与单独写出的结果逐字节一致；某个 target 在 `Reject` 下拒绝更新时所有 target 保持不变、可重试；发布中途失败时已发布的输出被撤回，不留暂存目录。
- DAMF 读取：home 与 3dof 转回同一配置时 `.atmos`、`.atmos.metadata`、`.atmos.audio` 三个成员逐字节复原；转 Standard/Logic/3dof 时与直接用同一模型写出的结果逐字节一致，损失诊断也相同。
- 手写的第三方写法：注释、flow/block 混用、引号风格、键顺序、bed ID 打乱、对象 ID 不连续、事件按时间交错、`sampleRate` 在 `events` 之后、`-.inf` 增益、`size3D`、缺省的 `rampLength`；按声道标记值核对了输出的声道顺序。
- WAV 代替音频：24-bit WAV 与原 CAF 转换结果逐字节一致；声道不足时报错并指明缺失的 DAMF ID。
- 规模：118 个对象、70.8 万个事件、60 秒 48 kHz 的 DAMF，一次转成 Standard、Logic、3dof 三份（约 1.1–1.4 GB 一份），release 构建 23 秒，峰值内存 12 MiB。
- 尚未进行：用 Dolby Conversion Tool 产出的 DAMF 作为第三方输入做读回验证。上面的第三方写法是手写的，不能替代真实工具的输出。

## ADM 读取与转换

日期：2026-11-15，Linux x64 / Rust 1.98；macOS arm64 与 Windows x64 待 CI。

- workspace：fmt、Clippy（所有 target、警告视为错误）与全部测试通过；不开 `read` 的 `macin-atmos` 单独检查通过。
- `macin-adm` 新增按节点号只读访问原始元素的接口，不改动 `#[repr(C)]` 记录；越界节点号返回空值而不 panic。
- ADM → 同一配置：本库写出的 Standard 与 Logic（默认帧率与 DBMD 字段）读回再写，整个文件逐字节一致。
- ADM → DAMF → ADM：位置不超过 9 位小数、增益为 1 或静音、默认节目名时逐字节回到原文件；带 dB 增益时经 DAMF 的增益误差小于 1e-9。
- 手写的第三方 ADM：只在 CHNA 中定义轨道、bed 声道顺序颠倒、每个对象一个 content、对象 `start` / `duration`、未写 `jumpPosition` 的整块插值、dB 增益与对象级增益折算、`mute`；按声道标记值核对了输出声道顺序。
- 损失与拒绝：逐项覆盖 README 列出的损失字段，以及极坐标、block 空隙、多个 programme、嵌套对象、非 7.1.2 bed 和没有 axml 的文件。
- 预读过的音频与来自 DAMF 读取器的音频，转换结果与直接转换逐字节相同。
- 规模：118 个对象、11.8 万个事件、10 秒的 ADM（AXML 57.8 MB），转 DAMF 用时 2.2 秒，峰值内存 642 MiB，约为 AXML 大小的 11 倍。同一 DAMF 经 ADM 再转回 DAMF，清单与音频逐字节一致；元数据里每个对象有一个事件不同，原因是其 ramp 延伸到音频结尾之后，ADM 只记录到结尾为止。
- 尚未进行：读取其他工具实际产出的 ADM BWF。上面的第三方写法是手写的。

## 外部互操作

Dolby Atmos Conversion Tool 2.1.2_07765940、DME 随附 atmos_info、EBU EAR 2.1.0 与 mradm Release。

| 配置 | 48/96 kHz 的实际结果 | 工具限制 |
| --- | --- | --- |
| Standard BW64 | EAR 图/CHNA 校验与 mradm 导入通过 | 本机两款 Dolby 读取器要求 RIFF/RF64，不识别 BW64 |
| Logic RF64 | Conversion Tool 转换、EAR 校验、mradm 导入通过；转换后采样率/帧数保持，PCM 逐字节一致 | atmos_info 的严格校验拒绝非 Dolby 创建工具标记；保留真实创建者，不伪造来源 |
| DAMF home | Conversion Tool 与 atmos_info 通过；转换后采样率/帧数保持，PCM 逐字节一致 | 无本次用例限制 |
| DAMF 3DoF | atmos_info 验证通过 | 本机 Conversion Tool 最高支持 DAMF 0.5，拒绝 0.6.0 |

检查器显式记录每个工具的拒绝及原因；只接受上表已识别的限制，未知失败仍使检查失败。
Standard 必须通过独立 EAR 验证，3DoF 必须通过 atmos_info，Logic/home 必须通过保持采样率与 PCM 的转换。
同源读回用于集成验收，不作为独立格式正确性依据。

## 消费方迁移

- AC-4：全 workspace audio-decode 测试与 Clippy 通过；12 项真实向量 ADM/DAMF 集成回归通过。
- 同一 768K 向量迁移前后：Standard/Logic ADM 的 AXML、CHNA、DBMD（适用时）及 DAMF manifest/metadata 逐字节相同。三种输出的 25,920,000 字节 PCM 哈希均为 a685e79a7d370f30c5d19ae8ad3f3a4204cd5638cf6d2b9438bbfb20291cac37。
- 渲染端：Rust WAVE/Scene 相关测试、全 workspace Clippy、许可资源定位与哈希检查通过。Release 原生构建与 11 项定向 CTest 通过，公开库恰好导出 164 个 adm_*；FFI 头检查含 391 个函数、163 个结构及公开 C ABI 的 31 个结构。
- 聚类端：合成结果导出示例为 12 轨、96 kHz、1024 帧；四种配置均完成实际写入和读回/PCM 一致性检查。示例不宣称已实现聚类算法。

私有阶段消费方仅在迁移分支依赖固定 Git 提交。公开发布线与原 crates.io 发布检查保留。

- DAMF 局部耳机事件：已有独立检查器以新库生成的 home 包为输入，home/3DoF 验证通过；官方工具原生 DAMF 往返保持连续字段命令与耳机事件时间线。ADM 转换不作为 ramp 保真判据。
- GitHub Actions 的 macOS/Windows 矩阵在代码提交 2fceb9dd3fc2d163ab1290a07f5be58fae233382 上通过；run 43614066686。
