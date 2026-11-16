# 开发约定

本库提供 ADM/BWF 读写与 Atmos 母版写入。Rust 2024，MSRV 1.98，MIT，禁止 unsafe。
回复与文档使用中文，代码标识符使用英文。保持整数采样时间、结构化错误和明确的格式能力。
不依赖渲染、聚类或编解码实现；FFI 属于调用方。
优先复用已有 Cargo target 与全局缓存（`CARGO_TARGET_DIR` 或被忽略的 `.cargo/config.toml`），不复制消费方的 target 或媒体；不提交个人路径、媒体、规范 PDF、专有工具或生成产物。

## 命令

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo clippy --locked -p macin-atmos --all-targets -- -D warnings
cargo test --locked -p macin-atmos
python -B -m unittest discover -s scripts -p "test_*.py"
```

workspace 构建会经由 `macin-format` 打开 `macin-atmos` 的 `read`（Cargo feature 合并），所以只写母版的配置——
下游消费方实际依赖的那一种——必须用第四、五条单独检查。CI（`.github/workflows/ci.yml`）在 macos-15 与
windows-2025 上跑上面全部六条。

`crates/macin-bwf/tests/wave.rs` 里实际写入超过 4 GiB 的测试是 ignored 的，需要 `MRADM_WAV_LARGE_TEST_DIR`
指向有空间的目录；普通测试用虚拟输出覆盖同一边界。外部工具互操作检查器 `scripts/check_interop.py` 需要调用方提供
EBU EAR、Conversion Tool、atmos_info，用法见 `docs/VALIDATION.md`；它的产物放在被忽略的 `artifacts/`。

## 结构

- `macin-adm`：ADM XML（`xml.rs`、`document.rs`）、投影成 `Snapshot`（`project.rs`、`numeric.rs`、`records.rs`）、
  生成（`generate.rs`）与保留原文的回写（`patch.rs`）。`records.rs` 的 `#[repr(C)]` 记录由调用方的私有 C 适配器共享，
  不要改布局。数学函数走 `libm`，使导入的增益与位置不随平台变化。
- `macin-bwf`：RIFF/RF64/BW64 读写（`reader.rs`、`writer.rs`）、`chna`、成品容器编辑（`edit.rs`，样本字节原样拷贝）。
  不解释 ADM XML。
- `macin-atmos`：模型（`model.rs`）、`MasterWriter` / `MultiWriter`（`writer.rs`）、来源无关的序列化（`metadata.rs`）、
  批量容器入口与 CAF（`containers.rs`）、Logic DBMD（`dbmd.rs`）。`read` 之后：`damf.rs` + `yaml.rs`（严格 YAML 子集）、
  `adm.rs` + `adm/stream.rs`（流式 ADM 读取）。
- `macin-format`：`mfadm` 命令，`publish = false`。`convert.rs` 只读输入、要求输出不存在。

三个库的 `readme` 都指向根目录 `README.md`，它会随包进入 crates.io。

## 文档分工

照 MacinDecode Spatial Player 的分层，每类内容只有一个归宿。改了行为，就改它对应的那一份：

| 文件 | 写什么 | 不写什么 |
| --- | --- | --- |
| `README.md` / `README.en.md` | 能做什么、四个 crate、获取、快速上手、`mfadm` 一览、选哪种配置、支持范围与已知限制、兼容性一览、构建命令 | 逐条规则、损失字段清单、测量数据、日期记录、测试覆盖、脚本参数、API 变更说明 |
| `docs/REFERENCE.md` / `docs/REFERENCE.en.md` | 读写规则全集：量化约定、写入规则、四种配置、DAMF / ADM 读取的支持 / 损失 / 拒绝、诊断、`mfadm` 参数、依赖与 feature | 实测数字（只链接到验证记录） |
| `docs/VALIDATION.md` | 测试覆盖、互操作检查器用法，以及按日期记录的实测：平台、代码提交、工具版本、规模与内存、逐项结果 | 规则本身 |
| `docs/CRATES_IO_RELEASE.md` | 发版步骤 | |
| `AGENTS.md` | 本文：开发命令、结构、约定 | 用户文档 |

- 两对中英文件同步修改，中文为准。英文版的锚点链接到中文文档时保留中文锚点。
- README 开头那句“本文只讲……见格式参考”是分界线：想往 README 里加一条规则或一个数字时，改加到 REFERENCE 或 VALIDATION，
  README 只留一句结论和链接。
- 新增损失字段或拒绝条件时，同时更新 REFERENCE 的清单与对应测试；VALIDATION 里“逐项覆盖格式参考列出的损失字段”依赖这一点。
- 验证记录用作者本人的口吻：实测就写“实测”，不写“用户实测”。新的实测加一节，写明日期、平台、代码提交与工具版本；
  工具拒绝按已识别的限制记录，不写成通过。

## 约定

- 提交遵循 Conventional Commits，描述用中文（`feat(atmos):`、`fix(cli):`、`docs:`；破坏性变更加 `!`）。
- 依赖版本全部用 `=` 锁定在 workspace 的 `[workspace.dependencies]`；三个库之外的依赖不进入库的依赖图。
- 默认 `LossPolicy::Reject`。新映射先拒绝，验证过才放开；`Allow` 下的降级必须进入 `MappingDiagnostic`。
- 测试只用仓库内的合成夹具；真实工具与媒体只在本地验证，不进入 Git。
