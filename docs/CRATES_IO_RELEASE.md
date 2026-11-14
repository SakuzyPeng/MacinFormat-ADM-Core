# crates.io 发布准备

当前准备 `0.1.0` 的包元数据与归档，尚未上传 crates.io。公开 GitHub 源码可作为固定提交的 Git 依赖使用。

`macin-format`（`mfadm` 命令）设为 `publish = false`，`cargo publish --workspace` 会跳过它。

三个 crate 继承 workspace 版本、MSRV、MIT 与仓库地址，归档包含 README 和许可证。
`macin-adm` 同时携带 common definitions 的 Apache-2.0 LICENSE、NOTICE 与来源清单。

发布前在干净工作树执行以下检查。

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo package --locked --workspace
cargo publish --locked --workspace --dry-run
```

用 `cargo package -p <crate> --list` 核对归档只含清单、锁文件、README、许可证、源码、测试、示例和 ADM 资源。

后续正式上传时，先发布 `macin-adm` 与 `macin-bwf`，等待注册表可解析，再发布 `macin-atmos`。
首次发布前确认 crates.io 名称和 owner，发布后检查 docs.rs 及仅使用注册表依赖的下游构建。
