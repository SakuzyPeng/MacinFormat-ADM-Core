# crates.io 发布准备

当前准备 `0.1.0` 的包元数据与归档，尚未上传 crates.io。公开 GitHub 源码可作为固定提交的 Git 依赖使用。

`macin-format`（`mfadm` 命令）设为 `publish = false`，`cargo publish --workspace` 会跳过它。
`macin-atmos` 的 `read` feature 默认关闭，`saphyr-parser` 只作为可选依赖进入其清单；发布前还要单独执行
`cargo clippy --locked -p macin-atmos --all-targets -- -D warnings` 与 `cargo test --locked -p macin-atmos`，
确认不开 `read` 的配置。

CLI 下载包、三平台构建与 GitHub 发布草稿见 [mfadm 下载包与 GitHub 发布](BINARY_RELEASE.md)。

三个 crate 继承 workspace 版本、MSRV、MIT 与仓库地址，归档包含 README 和许可证。
`macin-adm` 同时携带 common definitions 的 Apache-2.0 LICENSE、NOTICE 与来源清单。

发布前在干净工作树执行以下检查。

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo clippy --locked -p macin-atmos --all-targets -- -D warnings
cargo test --locked -p macin-atmos
python3 -B -m unittest discover -s scripts -p "test_*.py"
cargo package --locked --workspace --exclude macin-format --all-features
cargo publish --locked --workspace --dry-run
```

用 `cargo package -p <crate> --list` 核对归档只含清单、锁文件、README、许可证、源码、测试、示例和 ADM 资源。

归档与演练复用 Cargo 配置中的 target 和全局缓存。开发期间同一版本重复归档时，如果出现编译结果与归档源码不符，
可对受影响的 crate 执行 `cargo clean -p <crate>` 后重试，以刷新该包的构建缓存。

正式上传前确认 crates.io 名称与当前账户，使用 Cargo 1.98 的 workspace 发布支持。

```sh
cargo publish --locked --workspace
```

发布顺序为 `macin-adm`、`macin-bwf`，随后是依赖它们的 `macin-atmos`。发布后检查三个库的 docs.rs，
并在独立消费项目中仅通过注册表依赖执行默认与 `macin-atmos/read` 构建。
发布包的创建与演练选项见 Cargo 官方文档。
[cargo package](https://doc.rust-lang.org/cargo/commands/cargo-package.html) ·
[cargo publish](https://doc.rust-lang.org/cargo/commands/cargo-publish.html)
