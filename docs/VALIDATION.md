# 验证记录

日期：2026-11-14。生成媒体均为确定性合成信号，产物不进入 Git。

## 新库

- macOS arm64：fmt、Clippy（所有 target、警告视为错误）、49 项测试通过；1 项实际写入超过 4 GiB 的历史测试保持 ignored。新增虚拟输出测试覆盖该边界而不占用巨型磁盘文件。
- Windows x64 / Rust MSVC 1.98：49 项通过、同一历史测试 ignored；采用代码提交 bf3fdca597efef6f9baec5d0adfee317fcb53d76。首次 checkout 的换行转换已按 .gitattributes 的 LF 策略刷新，资源哈希与锁定清单一致。
- 覆盖独立 XML/WAVE 输入、未知扩展回写、四种配置与两种采样率、分块不变性、独立 ramp、局部耳机更新、量化端点、微小 gain、原子失败和超大尺寸。

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
- GitHub Actions 的 macOS/Windows 矩阵在代码提交 bf3fdca597efef6f9baec5d0adfee317fcb53d76 上通过；run 43612603716。
