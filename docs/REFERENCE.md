# 格式参考

**简体中文** · [English](REFERENCE.en.md)

本文是 [README](../README.md) 背后的规则全集：写入母版时每一步怎么处理、四种配置各能表达什么、
读取 DAMF 和 ADM 时什么照搬、什么按损失处理、什么直接拒绝。README 只讲能做什么和怎么上手；
这些规则的实测记录见[验证记录](VALIDATION.md)。

## 目录

- [PCM 量化约定](#pcm-量化约定)
- [母版写入规则](#母版写入规则)
- [四种母版配置](#四种母版配置)
- [DAMF 读取与转换](#damf-读取与转换)
- [ADM 读取与转换](#adm-读取与转换)
- [语义损失与诊断](#语义损失与诊断)
- [mfadm 参考](#mfadm-参考)
- [依赖与 feature](#依赖与-feature)

## PCM 量化约定

库里有两种浮点到整数的量化，分别服务两类调用方：

| 入口 | 约定 | 用途 |
| --- | --- | --- |
| `macin_bwf::Writer::write_frames` | 乘 2^(b−1)−1、向零截断 | 与 libbw64 0.10.0 保持一致 |
| `macin_bwf::Writer::write_frames_nearest` | 乘 2^(b−1)、就近取整、中点远离零 | 与母版写入同一约定 |
| 母版写入（`MasterWriter` / `MultiWriter`） | 乘 2^23、就近取整、中点远离零，正端点饱和至 8388607 | 24-bit 母版 |

`read_frames` 读出的 16/24-bit 样本经 `write_frames_nearest` 写回逐字节不变。

WAVE 元数据按调用方给定的预算读取，打开文件时不分配未知 chunk。

## 母版写入规则

**轨道布局**

- 固定 bed 顺序为 L、R、C、LFE、Lss、Rss、Lrs、Rrs、Lts、Rts；可以有真实信号或显式静音。
- 最多 118 个对象，槽位在创建时固定。`TrackId` 是调用方的身份标识，不是文件轨号；实际轨号见 `finish` 返回的报告。

**采样与量化**

- 支持 48/96 kHz、24-bit PCM；默认 48 kHz / 24 fps / Standard BW64。
- 接受交织或平面的 normalized f32，以及已量化的 S24LE。
- 浮点按[上表](#pcm-量化约定)量化；不加抖动、不自动归一化，非有限值和超出 ±1 的样本报错。S24LE 原字节保留。

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

**DBMD 创建来源**：Logic 输出的 DBMD 创建来源与产品名默认是 `Created by MacinFormat` 与 `MacinFormat ADM Core`，
分别接受最多 32 和 64 字节且不含控制字符的 ASCII 文本。写入调用方设置 `MasterSpec::dbmd_originator` 与
`dbmd_product`，读取器的转换入口用 `with_dbmd_identity(originator, product)`，命令行见
[`--dbmd-originator`](#mfadm-参考)。Conversion Tool 2.1.2 对默认值会输出 `not created by Dolby approved software`
和 `input file is not Dolby Atmos`（转换仍完成、退出码为 0）；设为 `Created using Dolby equipment` 后不再出现，
见[验证记录](VALIDATION.md#dbmd-创建来源自定义)。

## DAMF 读取与转换

需要 `read` feature。`DamfReader::open` 读入 `.atmos` 清单，得到和写入时相同的 `MasterSpec`，
再由 `updates()` 逐条给出 `ObjectUpdate`；`convert` 经 `MultiWriter` 写成任意组合的配置。转换就是普通的写入，
所以上面每种配置的损失检查照常生效，读取时模型装不下的内容也按同一个 `LossPolicy` 处理。

- **内存有界**：元数据文件扫描两遍、从不整体载入。第一遍校验整条时间线，并把每个对象开头位于第 0 个采样、
  不带 ramp 的事件折叠成初始状态；第二遍按文件顺序给出其余更新。实测规模见
  [验证记录](VALIDATION.md#damf-读取转换与多份输出)。
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

需要 `read` feature。`AdmReader::open` 流式校验 ADM BWF，得到与写入时相同的 `MasterSpec`，由 `updates()` 按需读取 `ObjectUpdate`，
`convert` 同样经 `MultiWriter` 写出，所以 ADM 转 DAMF 就是普通的写入，DAMF 一侧的损失检查照常生效。
支持的是 Atmos 子集：本库写出的结构，以及其他工具写出的同类结构。通用的 ADM 解析（`macin_adm::Document` /
`Snapshot`）不受这个子集限制。

- **支持的结构**：一个 programme；至多一个 7.1.2 `DirectSpeakers` bed，声道用 `RC_L`…`RC_Rts`（或不带 `RC_`）
  标记，顺序不限；任意个单轨、Cartesian 坐标的 `Objects`，省略 Z 时取 0，布尔字段接受 `0` / `1` 和 `false` / `true`；
  轨道定义可以只写在 CHNA 里。Logic RF64（带 DBMD）读成 `Logic`，其余读成 `Standard`。
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
  DBMD 的创建者字段同样不读回，输出默认使用本库的值，可用 `with_dbmd_identity` 或对应的 CLI 参数自定义。
- **DAMF 装不下的**：DAMF 没有节目名字段。节目名只是描述信息，写 DAMF 时直接省略、不按损失处理，
  所以 ADM 转 DAMF 再转回时节目名回到默认值。`screenRef` 写 DAMF 时则按损失处理。
  反过来，DAMF 里延伸到音频结尾之后的 ramp 在 ADM 中被截在结尾，读回是截短的 ramp 与当时的插值位置。
- **内存**：母版读取保留引用结构、每个声道的首个 block 和文件范围，逐块校验与投影；不会整体载入 AXML，
  也不缓存全部更新或创建临时文件。固定对象数和单条 block 大小时，有序时间线的内存基本不随事件总数增长。
  乱序声道仍支持稳定排序，但会额外保留该声道每个 block 的文件偏移索引；结构性元数据、超大单条 block 和不同诊断也会增加内存。
  代价是多一次扫描与解析，实测对照见[验证记录](VALIDATION.md#adm-流式读取内存对照)。
  通用 `Document` / `Snapshot` 与 `mfadm adm` 检查命令仍整体解析。
- **更新接口**：`updates()` 和 `updates_for_frames(frames)` 返回 `Result<Updates>`，迭代项为
  `Result<ObjectUpdate>`，与 DAMF 一致；调用方需同时处理创建和迭代错误。`update_count()` 与
  `update_count_for_frames(frames)` 使用打开时的统计，不额外扫描。需要完整列表时可显式
  `reader.updates()?.collect::<macin_atmos::Result<Vec<_>>>()`，此时内存由调用方收集的事件数决定。
  迭代期间源文件须保持可读且内容不变；读取或解析失败后迭代器返回错误并终止。

## 语义损失与诊断

默认 `LossPolicy::Reject`：无法精确表达的语义直接报错。只有选 `Allow` 才采用明确的降级，并记录在
`MappingDiagnostic` 里。

- ADM 不擅自映射未验证的 zones、elevation、trim、内容类别、耳机策略和连续 screen/depth factor。
- Logic DBMD 只写已验证的静态兼容字段，不宣称能保存动态耳机事件。
- 已验证的 DAMF 耳机值为 undefined / off / near / far；Mid 被显式拒绝或降为 undefined。
- ADM importance 量化为 0..10，非整十的分值会报告损失。
- 音频 24-bit 量化和所选时钟精度属于已声明的格式行为，不算损失。DAMF 不写节目名，同样不算损失。

## mfadm 参考

`mfadm` 从不修改输入，也不覆盖已存在的输出。子命令一览见 [README](../README.md#命令行工具-mfadm)。

- `chunks`、`info`、`adm`、`convert` 加 `--json` 输出机器可读的 JSON。
- `adm` 和 `axml` 读元数据时有预算，默认 256 MiB，可用 `--max-metadata <MiB>` 调整。
- `mfadm adm` 除了按文件自身的 `chna` 和采样率解析 ADM，还会核对 CHNA 的轨道数与 `fmt ` 一致、记录没有被截断。
- `convert` 按文件内容区分 DAMF 与 ADM BWF 输入；`--audio <wav>` 改用 WAV 作音频，`--fps` 指定 ADM 来源的帧率，
  `--allow-loss` 允许降级并列出损失（等同 `LossPolicy::Allow`，默认拒绝）。
- `convert --dbmd-originator <TEXT>` 与 `--dbmd-product <TEXT>` 自定义所有 Logic 输出的 DBMD 字段，
  取值范围见[四种母版配置](#四种母版配置)；`mfadm convert --help` 也说明了 Conversion Tool 的来源提示。
- 出错时退出码为 1，参数错误为 2。

```sh
mfadm convert input.wav --to logic=output.wav \
  --dbmd-originator "Created using Dolby equipment"
```

## 依赖与 feature

四个 crate 都禁止 unsafe（`unsafe_code = "forbid"`）。三个库的依赖只有 `quick-xml`、`libm` 和 `renamore`，版本全部锁定。

两个读取器放在默认关闭的 `read` feature 后面。DAMF 读取需要 YAML 解析，只有打开它才会引入 `saphyr-parser`
（同样锁定版本）；ADM 读取不需要额外依赖，但与 DAMF 共用同一套转换流程。只写母版的消费方依赖图不变。

`macin-format` 只提供命令行工具 `mfadm`，不上传 crates.io；它额外依赖 `clap` 和 `serde_json`，
这些依赖不会进入三个库。
