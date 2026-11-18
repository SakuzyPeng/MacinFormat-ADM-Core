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

WAVE 元数据按调用方给定的预算读取，打开文件时不分配未知 chunk。`metadata_at` 按块表下标读取，可区分同名 LIST 的不同子类型。

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
- 位置、增益、尺寸、原生 diffuse / divergence 的连续更新显式提供 `ramp_samples`（`0` 表示立即变化）；纯离散更新使用 `None`，
  不会重启其他属性上既有的 ramp。属性缺省表示继承。
- `MasterSpec::bed` 保存十个规范声道的初始状态。`BedUpdate::channel` 是 `BED_LABELS` 中从 0 开始的位置，
  不占用对象的 `TrackId`；通过 `schedule_bed_updates` 调度。gain、DAMF bed 的 position/extent 更新必须提供 ramp，纯离散更新不重启 ramp。
  单个调用会先校验全部更新；`MultiWriter` 会在全部目标通过后才提交。
- `MasterSpec::contents` 为空时生成默认单组；自定义分组必须引用所有对象和固定 bed，允许对象跨组引用，
  不允许单组重复或未知对象；生成的 content ID 空间最多容纳 0xefff 组。`ObjectSpec::pack_name` / `channel_names` 与 `bed_channel_names` 保存独立格式名称，
  未指定时按默认约定生成。共享的 `macin_adm::records` 布局不变。

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
| `Standard`（默认） | BW64，ADM 九位时钟 | Objects 位置、gain、extent、snap、screenRef、离散 importance、原生 headLocked、diffuse / divergence / maxDistance / screenEdgeLock、区域预设与 elevation；分层 ADM 元数据和节目起点 |
| `Logic` | RF64，ADM 五位时钟 | 相同位置与区域属性；通过 DBMD 保存静态头追、耳机、trimBypass、帧率及下列节目级配置 |
| `DamfHome` | DAMF 0.5.1，S24LE CAF | 单 bed 的状态与事件、对象事件、区域、trimBypass、screen/depth factor、内容类别与耳机策略 |
| `DamfThreeDof` | DAMF 0.6.0 / 3dof | 相同音频与事件；不强制改变对象头追策略 |

`interpolationLength` 保留九位来源精度，不随 Logic 时钟降低。ADM 文件的 `ds64` 固定为首 chunk，
`chna` 在音频之前，流式 AXML 在音频之后；依据 ITU-R BS.2088-2 Annex 1 §2.1、§4、§5。

**原生参数与目标限制**

- `HeadTracking` 支持 SceneRelative、HeadRelative、Undefined。Standard 用 XML `headLocked` 表达前两种，
  支持对象动态变化；Undefined 无布尔对应，按损失处理。Logic 在 DBMD 中表达三种静态值，不写 XML 头锁。
- Logic 的头追、耳机模式和 trimBypass 取第 0 个采样的有效状态，包含在写音频前提交的 sample=0 更新。
  之后的变化在 Reject 下拒绝，Allow 保留初始值并诊断。Standard 无耳机模式和 trimBypass 的精确通路。
- ADM bed 支持对象级 gain/mute 和独立时段，每路 DirectSpeakers block 可以保存 gain、0..10 的 importance、整个扬声器标签列表、几何和 headphoneVirtualise。Standard 可逐块保存 headLocked。Logic 头追、耳机和 trimBypass 使用静态 DBMD。
- DAMF bed 保留 active、gain、importance、头追、耳机和 trim，以及 `BedRendering` 中显式提供的 position、extent、snap、zone、elevation、decorrelation、dialog、music、screen_factor、depth_factor。动态清零/关闭也会写回。目标为 ADM 时，缺少精确表示的渲染属性按各自的 `bed_*` 诊断。active=false 在 Allow 下写成静音增益，非零 DAMF bed ramp 在 ADM 中按 `bed_ramp` 拒绝或诊断后写终值。
- `MasterSpec::start_sample` 是节目外部起点；对象和 bed 的事件仍从音频第 0 个采样计时。
  ADM 写 programme start/end，DAMF 写 offset；非负秒数输入按最近采样取整。
- Logic/DAMF 保存 `sc_bed_configuration`、`downmix_mode`、`surround_phase_shift`、`bed_distribution`、
  `warp_mode` 与 `trim_mode`。配置码为 0..=8 的不重复集合；模型空集合表示 DBMD 未指定，可在 Logic 保留。
  DAMF 输入缺省或空列表解释为 `[3]`，不能表达该未指定状态；写 DAMF 时 Reject 拒绝，Allow 诊断并采用 `[3]`。
  下混为 LoRo_Stereo / LtRt_ProLogic / LtRt_PLII；warp 为 LoRo / ProLogicIIx / warping / normal。
  `WarpMode::None` 保留为 Normal 的 API 别名，旧输入 `None` 也归一化为 normal。
- `bed_distribution_present` 区分未指定和显式 false/true，默认显式 false；未指定时数值必须为 false。
  可用 `set_bed_distribution(Option<bool>)` 同时设置两者。未指定在 DAMF 中省略，在 DBMD 中保留未指定状态。
- `trim_mode` 按 `TrimConfiguration::ALL` 对应四组配置。None 表示自动，Some(default) 表示空的手动配置；
  默认四组均为 Some(default)，读取时缺省的配置为 None。三项 trim 为有限且可存为有限 f32 的数值，
  两项 front/back balance 限于 -1..=1。Logic 使用原生 f32 精度，并将 balance 乘 127 后就近取整
  （中点远离零）；这是已声明的存储精度。DAMF 按文本数值精度写出。
- Standard 不承载非默认的上述 DBMD 节目级设置，按字段报告损失；未指定的 bedDistribution 和自动 trim
  与该配置默认的显式 false / 空手动设置不同，同样报告损失。screen_size_ratio 在两种 ADM 输出中均没有映射。
  三项静态通道字段、区域预设与节目级字段的外部验证见[验证记录](VALIDATION.md)。

**DBMD 创建来源**：Logic 输出的 DBMD 创建来源与产品名默认是 `Created by MacinFormat` 与 `MacinFormat ADM Core`，
分别接受最多 32 和 64 字节且不含控制字符的 ASCII 文本。写入调用方设置 `MasterSpec::dbmd_originator` 与
`dbmd_product`，读取器的转换入口用 `with_dbmd_identity(originator, product)`，命令行见
[`--dbmd-originator`](#mfadm-参考)。Conversion Tool 2.1.2 对默认值会输出 `not created by Dolby approved software`
和 `input file is not Dolby Atmos`（转换仍完成、退出码为 0）；设为 `Created using Dolby equipment` 后不再出现，
见[验证记录](VALIDATION.md#dbmd-创建来源自定义)。

## 原生 ADM 元数据与容器保留

已识别字段先保留在模型中；目标无法表达时才执行 `LossPolicy`。同一次转换可以在 ADM 中原生保存，
在 DAMF 中单独近似或报告损失，不会先从读取模型中删除。

- `ObjectState::diffuse` 与 `decorrelation` 独立，前者限于 0..=1。`ObjectDivergence` 保存 value、
  以及可选的 Cartesian position_range（0..=1）。读取旧输入时模型也能记录极坐标的 azimuth_range（0..=180）。
  Cartesian ADM 默认按 `divergence_azimuth_range` 拒绝这个角度，Allow 保留 divergence 值并丢弃角度。显式原值保留见下文。
  同时提供两种范围属于无效输入。`channel_lock_distance` 保存 maxDistance，
  None 表示未指定，显式值限于 0..=2√3。`screen_edge_lock` 的 X 接受 Left/Right，Z 接受 Top/Bottom，Y 不允许锁定。
  两种 ADM 输出均原生保存这些字段及更新，新增数值采用可往返 f64 的十进制写出；block 缺省值分别恢复 0、None、None 和无边缘锁定。
  diffuse / divergence 使用独立 ramp。decorr 近似为 diffuse 时直接复制事件处的值，即使该事件还包含位置 ramp。
  AXML 每个 block 共用一个插值时长，渲染器可能据此插值。
- `AdmMetadata` 由属性和 `AdmElement` 子树组成，按所属层级保存：`MasterSpec::adm` 是 programme，
  `ContentSpec::adm` 是 content，`ObjectSpec::adm` / `pack_adm` / `channel_adm` 分别是 object、pack、channel；
  bed 使用 `bed_adm` / `bed_pack_adm` / `bed_channel_adm`。不会把对象或 pack importance 覆盖到 block importance，
  对象层的 gain、mute 和 positionOffset 保留原所属层。读取的 `ObjectState::gain` 及更新中的 gain 是 block 自身的增益。`ObjectSpec::adm.effective_gain(block_gain)` 可计算实际增益。写 DAMF 时才合并这两层，PCM 样本保持原值。
- `MasterSpec::bed_timing` 保存 bed 相对节目的起点、可选时长。`bed_blocks[channel]` 保存该声道首个 `AdmBedBlock`。block 的 `timing.start` 是相对于 **bed 对象** 的 rtime。后续块经 `AdmReader::bed_blocks()` 流式读取，用 `schedule_adm_bed_blocks` 提交。同一声道选定原生块后，后续继续使用这个接口。单独用状态编写时可使用 `bed_geometry` 和 `schedule_bed_updates`。两种写法都支持单目标与多目标。
  ADM 输出保留逐块时长、空隙、重叠、备用坐标、bound、screenEdgeLock 及标签顺序。转 DAMF 时起止和空隙映射为各声道的 active 事件。重叠的块按 `bed_timing` 报告，Allow 采用后一个块。DAMF 无法保存的多标签或非规范扬声器位置按 `bed_position` 报告。
- `ObjectSpec::custom_zones` 保存自定义区域的初值。`native_updates()` / `native_updates_for_frames` 和 `schedule_native_updates` 传递 `NativeObjectUpdate`，含原数值事件以及可选区域列表。None 表示继承，空列表表示清除。原 `updates()` 提供数值部分，完整手工往返应使用原生更新接口。只改区域时数值 patch 可以为空。合法 Cartesian 六边界、极坐标四边界、标签和多区域顺序均可写回。自定义区域与预设或独立关闭 elevation 冲突会报错。
  对象和 bed 均可原生保存 headphoneVirtualise 的可选 bypass 和 DRR（−130..130 dB）。自定义区域或此耳机参数写 DAMF 时分别报告 `zone`、`headphone_virtualise`。
- 已识别静态元数据包括 programme/content 的语言、标签和 loudnessMetadata、programme 的参考屏幕与 maxDuckingDepth、
  object 的标签、positionOffset、importance、dialogue、interact、disableDucking 及 audioObjectInteraction 范围，
  pack importance、channel frequency。参考屏幕的 aspectRatio 写为属性，同时兼容读取旧的同名子元素。
  两种写法的数值冲突时拒绝。旧 channelFrequency 读取后按 frequency 写出。文本数值保持来源精度；输出统一 XML 转义。只接受这些已识别节点的属性和子元素，
  不接受通过扩展插入 ID 引用或任意 XML；未知扩展仍按损失处理。公开写入入口也校验数值、枚举、几何和重复单值字段。
- 上述静态元数据写 DAMF 时按 `language` / `labels` / `loudness` / `reference_screen` / `position_offset` /
  `importance` / `pack_importance` / `dialog` / `interaction` / `ducking` / `frequency` 报告损失；divergence、
  maxDistance 和边缘锁定分别报告 `divergence` / `channel_lock_distance` / `screen_edge_lock`。
  对象和 pack 的显式 importance=10 可作为默认值省略，不产生损失。选择 diffuse 近似不会放开其他损失。替换音频或改变原节目时长时，源 ADM 响度数据默认拒绝沿用；Allow 清除并报告 `loudness`。
- `ContainerMetadata` 支持 WAVE 的 bext、iXML、LIST/INFO 和 CAF 的 info；总载荷预算为 4 MiB，PCM 不进入缓存。
  同容器族、原 S24LE 音频、相同声道数量和顺序、采样率、节目起点及音频帧数时按原字节保留。
  改变这些条件、跨 WAVE/CAF、超过预算或其他附加块均按 `container_metadata` 处理；Allow 丢弃不适用的块并诊断。
  替换音频会使原母版及替换音频的附加块都按损失处理。替换已有 DAMF 音频时仅检查原 CAF 块头，原 CAF 缺失也可使用替换 WAV。
  直接向 writer 提交 PCM 或通过 `ContainerMetadata::new` 自行提供载荷时，调用方负责内容一致性。
  附加块可移到 data 之后，外层长度和对齐重新生成；重复的受支持附加块属于无效输入。

原生 ADM 字段定义参见 [ITU-R BS.2076-2 表 12、17 与 39](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.2076-2-201910-S%21%21PDF-E.pdf)、[EBU Objects](https://adm.ebu.io/guidelines/reference/adm_elements/audio_channel_format_objects.html)、
[programme](https://adm.ebu.io/guidelines/reference/adm_elements/audio_programme.html) 与
[object](https://adm.ebu.io/guidelines/reference/adm_elements/audio_object.html)。本库能保存不表示 Dolby Conversion Tool 会采用或继续保存；
工具行为与本库往返分别见[验证记录](VALIDATION.md)。Logic 的动态 DBMD 限制仍按前节执行。

## DAMF 读取与转换

需要 `read` feature。`DamfReader::open` 读入 `.atmos` 清单，得到和写入时相同的 `MasterSpec`，
再由 `updates()` 和 `bed_updates()` 分别逐条给出 `ObjectUpdate`、`BedUpdate`；`convert` 经 `MultiWriter` 写成任意组合的配置。转换就是普通的写入，
所以上面每种配置的损失检查照常生效，读取时模型装不下的内容也按同一个 `LossPolicy` 处理。

- **内存有界**：元数据从不整体载入。第一遍校验整条时间线，将对象及 bed 开头的 sample=0、
  无 ramp 事件折叠成初始状态；对象更新再扫描一遍，存在 bed 更新时额外扫描一遍。各自按文件顺序给出更新。
  `update_count()` 只计对象，`bed_update_count()` 单独计 bed。实测规模见
  [验证记录](VALIDATION.md#damf-读取转换与多份输出)。
- **ID 与声道**：DAMF 的 ID 就是音频声道号。bed 按 L、R、C、LFE、Lss、Rss、Lrs、Rrs、Lts、Rts 重新排列，
  对象按 ID 升序排在后面，读回的 `TrackId` 等于 DAMF ID。事件省略 `ID` 时沿用前一个事件的值，首个事件必须提供 `ID`。
  没有 bed 的 DAMF 会写出一组静音 bed。
- **WAV 作音频**：`wave_audio` 用 WAV 代替 `.atmos.audio`，第 n 个声道对应 DAMF ID n，采样率必须与元数据一致。
  16/24-bit PCM 精确转换；32-bit PCM 和浮点按已声明的格式行为量化到 24 bit，超出 ±1 的浮点样本报错。
- **读取损失**包括未知键（`unknown_field`）、没有连续属性却有非零 ramp 的事件（`ramp` / `bed_ramp`）、对象缺少采样 0 初始状态（`initial_state`）、simplified=true，以及未识别的 trim 配置或参数（`trim_mode`）。无初始事件的 bed 使用该声道的默认值。
- **容器元数据**：扫描 CAF 的 data 前后附加块；info 按[容器保留规则](#原生-adm-元数据与容器保留)处理，
  其余非 desc/data/free 块按 `container_metadata` 处理。只读取受支持且预算内的载荷，坏长度或重复 data 直接拒绝。
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
  `contents` 保留 content 分组、名称、对象归属与对白分类；独立 pack/channel/stream/track 名称也保留。
  无 bed 输入仍补十路静音，并将这个静音 bed 放入首个 content。
- **数值精度**：位置、尺寸等从文档原文按 `f64` 解析，而不是取 `Snapshot` 里的 `f32` 投影，所以本库写出的
  ADM 在默认精确子集内读回再写，Standard 与原文件逐字节一致。Logic 恢复已识别 DBMD 字段；创建者身份仍使用本库的值或显式覆盖值。
- **时间线**：每个 block 自成一体，没写的元素取缺省值。block 起点成为一次更新，只包含与前一个 block 不同的字段；
  `jumpPosition` 为 1 时 ramp 取 `interpolationLength`（不超过 block 长度），没写 `jumpPosition` 时在整个 block 内插值。
  对象的 `start` / `duration` 变成激活与停用的更新，结束点还受最后一个有限 block 的结束时间约束。
  换用更长的 WAV 也保留停用边界；`updates_for_frames` 按实际音频长度提供更新，`updates` 返回原文件长度下的更新。
  对象级 gain 和 mute 保存在对象元数据中，block 增益保持独立。
  ADM 没有 active 标志：写入器把不活动的对象写成 gain 0，读回就是静音增益。
- **节目范围**：programme start 进入 start_sample。显式 end 必须晚于 start，且持续时间不能超过音频。
  原音频的 `audio()` 只交付节目持续时间内的样本，转换不再把 end 扩成音频文件总长。
  显式替换 WAV 仍按替换长度输出，并保留来源节目、对象和 bed 的有限停用边界。缩短替换音频时 bed 时长截到输出范围，后续事件仍须有对应音频。
- **头锁与区域**：对象级 headLocked 为 block 的缺省值，block 可以覆盖；缺省的 block 不继承前一个 block 的覆盖。
  名称、边界和完整组合都匹配时，区域使用已验证预设（含 ZB/ZT elevation）。其他完整的自定义区域按原值保存。省略 zoneExclusion 时恢复 All / elevation=true 并清除自定义列表。
- **DBMD**：按来源声道号恢复静态通道参数，再按输出声道重排。XML 头锁与 DBMD 冲突时 Reject 报错，
  Allow 采用 DBMD 并诊断。未知版本（`dbmd`）、编码设置或 segment（`dbmd_encoding`）、未识别的帧率、
  布局位、trim 配置或通道模式按相应字段报告；Allow 可以保留已识别部分。坏校验和、重复 segment、
  无效长度或声道数不匹配属于无效输入，Allow 也拒绝。
- **ID 与声道**：对象按轨道号排序，`TrackId` 是对象在文件里的轨道号（从 1 起）；`wave_audio` 用 WAV 代替音频时，
  第 n 个声道对应第 n + 1 条轨道。没有 bed 的文件写出一组静音 bed。
- **读取阶段的损失**包括未映射元素和属性（`unknown_field`）、不完整或包含未识别字段的区域（`zone`）、对象位置的 bound（`position_bound`）、多个或嵌套 pack 引用（`pack_structure`）。可解析的完整原生字段留到目标写出时判断。附加块遵循前述容器规则。
- **轨道扩展属性**保存在 `ChannelNames::extensions` 的 stream、track、uid 中。ADM 输出在原所属元素写出前缀、命名空间 URI、局部名和值，并声明值中使用的已知命名空间前缀。换行、制表符按 XML 字符引用转义。值里的供应商私有标识符和相对路径作为不透明文本保存。写 DAMF 遇到这些属性时报告 `track_extensions`。
- **直接拒绝**：多个 programme、多个 bed、不完整或非 7.1.2 的 bed、极坐标对象、多声道 `Objects`、嵌套对象、
  HOA 等其他 `typeDefinition`、对象 block 之间有空隙或重叠，以及没有 `axml` / `chna` 的文件。
- **帧率**：ADM XML 不带帧率；读取已识别 DBMD 的帧率，未提供时默认 24 fps，用 `with_frame_rate`（或 `mfadm convert --fps`）覆盖。
  DBMD 的创建者字段同样不读回，输出默认使用本库的值，可用 `with_dbmd_identity` 或对应的 CLI 参数自定义。
- **DAMF 装不下的**：DAMF 没有节目名字段。节目名只是描述信息，写 DAMF 时直接省略、不按损失处理，
  单个 content 的名称及独立格式名称也作为描述信息省略；再转回时使用生成的名称。
  多 content 结构按 `content_structure` 处理，非默认 content 对白分类按 `dialog` 处理；默认 Mixed/0 可以省略。
  `screenRef` 写 DAMF 时默认按损失处理。
  `diffuse` 和 `screenRef` 可逐项选择下文的[近似映射](#语义损失与诊断)。
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

默认 `LossPolicy::Reject`：无法精确表达、也未获准近似的语义直接报错。`Allow` 允许其余字段采用已声明的降级。
近似映射由 `ApproximateMappings` 逐项授权，默认全部关闭；选定的近似可在 `Reject` 下执行，
不会放开其他字段的损失，也不会改变 PCM。

| 开关 / Rust 字段 | 方向与规则 |
| --- | --- |
| `--approximate diffuse-decorrelation` / `diffuse_decorrelation` | 读取保留原生 `diffuse`；仅写 DAMF 时将它映射为 `decorrelation` / `decorr`，反向写 ADM 时将 `decorrelation` 映射为 `diffuse`。数值直传，保留整数采样时间点，按离散更新处理；缺省或归零仍为 0 |
| `--approximate screen-ref-factor` / `screen_ref_factor` | ADM `screenRef` 写 DAMF 时 false/true 映射为 `screenFactor` 0/1；DAMF `screenFactor` 写 ADM 时 0 为 false，非零为 true。连续因子不保留，显式归零可以再次关闭屏幕参考 |

这些规则是调用方选择的近似，不声明参数的渲染语义等价，也不保证其他工具保留它们；工具实测见
[验证记录](VALIDATION.md#dolby-conversion-tool-元数据映射对照)。`--allow-loss` 本身不会开启上述近似：
未选择开关时，同格式的原生 diffuse / decorr 正常保存；跨格式未映射的 `diffuse` / `decorrelation` / `screenRef` / `screenFactor` 按损失策略处理，
其中 `screenFactor` 的降级是丢弃因子并保留已有的原生 `screenRef`，不自动把它设为 true。
若近似与同一次更新中显式提供的目标字段冲突，或与非默认目标初始值冲突，`Reject` 拒绝；
`Allow` 保留目标原值、丢弃冲突的来源字段并记录损失。

库调用方在 `MasterSpec::approximate_mappings` 中选择写出近似；读取转换时使用
`AdmReader::open_with_options` / `DamfReader::open_with_options`，传入 `MappingOptions` 的 `loss_policy`、`approximate_mappings` 和 `preserve_literal`。选项随读取模型传递到更新迭代和全部转换目标。
`open(path, policy)` 默认关闭近似和原值开关。低层 `metadata` 入口仍由调用方完成映射检查；
逐块序列化可使用 `write_adm_block_with_approximations`。
读取非零 `diffuse` 或再次写 ADM 均不需要近似开关；再次转 DAMF 时才需要授权该近似。

对于两个兼容性受限的取值，`--preserve-literal <FIELD>` 可以逐项重复。Rust 入口为 `MasterSpec::preserve_literal` 和 `MappingOptions::preserve_literal`，默认均关闭。

| 字段 | 已授权的写法 |
| --- | --- |
| `cartesian-azimuth-range` | 在 Cartesian 对象块上保留旧 azimuthRange 原值。这种拼写超出 BS.2076-2 对该坐标系的约定，不进行角度到距离换算。两种 range 并存仍拒绝。 |
| `damf-mid` | 在 DAMF 的对象或 bed 上原样写入 `binauralRenderMode: mid`。Dolby Conversion Tool 2.1.2 拒绝此文本。该开关不赋予 Logic DBMD 新编码。 |

`MappingDiagnostic::kind` 区分 Loss、Approximation 和 PreservedLiteral。CLI 在 `read_losses` 或 `outputs[].losses` 返回 `kind`，取值分别为 `loss`、`approximation`、`preserved_literal`。文字输出对应 lost、approximate、preserved。后两种选择记在对应目标，每条轨道、字段只记首次，无须 `--allow-loss`，其他字段的损失仍按原策略处理。

- 不猜测任意区域到预设、内容类别到 dialog/music 标志或连续 screen/depth factor 的等价关系。
- Logic DBMD 仅保存已验证的静态字段；动态头追、耳机和 trimBypass 变化必须拒绝或诊断后保留初始值。
- 已验证的 DAMF 耳机值为 undefined / off / near / far；未选择原值写入时，Mid 默认拒绝，Allow 对对象采用 undefined、对 bed 采用该声道默认值。
- ADM importance 量化为 0..10，非整十的分值会报告损失。
- 音频 24-bit 量化和所选时钟精度属于已声明的格式行为，不算损失。DAMF 不写节目名，同样不算损失。

## mfadm 参考

`mfadm` 从不修改输入，也不覆盖已存在的输出。子命令一览见 [README](../README.md#命令行工具-mfadm)。

- `chunks`、`info`、`adm`、`convert` 加 `--json` 输出机器可读的 JSON。
- `convert` 的 `updates` 计对象更新，`bed_updates` 单独计来源 bed 的后续事件或 ADM 后续 block，目标派生的 active 边界不重复计入。
- `adm` 和 `axml` 读元数据时有预算，默认 256 MiB，可用 `--max-metadata <MiB>` 调整。
- `mfadm adm` 除了按文件自身的 `chna` 和采样率解析 ADM，还会核对 CHNA 的轨道数与 `fmt ` 一致、记录没有被截断。
- `convert` 按文件内容区分 DAMF 与 ADM BWF 输入；`--audio <wav>` 改用 WAV 作音频，`--fps` 覆盖 ADM 输入的输出帧率声明（不重采样），
  `--allow-loss` 允许降级并列出损失（等同 `LossPolicy::Allow`，默认拒绝）。
- `convert --preserve-literal <FIELD>` 逐项保留旧 Cartesian 角度范围或 DAMF mid。
- `convert --approximate <MAPPING>` 可重复，只启用指定字段的近似；支持的名称和规则见[语义损失与诊断](#语义损失与诊断)。
- `convert --dbmd-originator <TEXT>` 与 `--dbmd-product <TEXT>` 自定义所有 Logic 输出的 DBMD 字段，
  取值范围见[四种母版配置](#四种母版配置)；`mfadm convert --help` 也说明了 Conversion Tool 的来源提示。
- 出错时退出码为 1，参数错误为 2。

```sh
mfadm convert input.wav --to logic=output.wav \
  --dbmd-originator "Created using Dolby equipment"

mfadm convert input.wav --to home=output-home \
  --approximate diffuse-decorrelation --approximate screen-ref-factor
```

## 依赖与 feature

四个 crate 都禁止 unsafe（`unsafe_code = "forbid"`）。三个库的依赖只有 `quick-xml`、`libm` 和 `renamore`，版本全部锁定。

两个读取器放在默认关闭的 `read` feature 后面。DAMF 读取需要 YAML 解析，只有打开它才会引入 `saphyr-parser`
（同样锁定版本）；ADM 读取不需要额外依赖，但与 DAMF 共用同一套转换流程。只写母版的消费方依赖图不变。

`macin-format` 只提供命令行工具 `mfadm`，不上传 crates.io；它额外依赖 `clap` 和 `serde_json`，
这些依赖不会进入三个库。
