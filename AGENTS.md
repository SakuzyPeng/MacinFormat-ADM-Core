# 开发约定

本库提供 ADM/BWF 读写与 Atmos 母版写入。Rust 2024，MSRV 1.98，MIT，禁止 unsafe。
回复与文档使用中文，代码标识符使用英文。保持整数采样时间、结构化错误和明确的格式能力。
不依赖渲染、聚类或编解码实现；FFI 属于调用方。
优先复用已有 Cargo target 与全局缓存，不提交个人路径、媒体、规范 PDF、专有工具或生成产物。
执行 cargo fmt --all -- --check、cargo clippy --locked --workspace --all-targets -- -D warnings 和 cargo test --locked --workspace。
