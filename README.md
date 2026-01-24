# axum-bootstrap

[![Crates.io](https://img.shields.io/crates/v/axum-bootstrap.svg)](https://crates.io/crates/axum-bootstrap)
[![License](https://img.shields.io/crates/l/axum-bootstrap.svg)](https://github.com/arloor/axum-bootstrap)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-blue.svg?maxAge=3600)](https://github.com/arloor/axum-bootstrap)

基于 [Axum](https://github.com/tokio-rs/axum) 框架的 Rust Web 服务器脚手架，提供了开箱即用的 TLS、日志、监控等功能，帮助开发者快速搭建生产级别的 Web 服务。

## ✨ 特性

- 🚀 **基于 Axum + Hyper**：使用 Rust 最流行的异步 Web 框架
- 🔒 **TLS/HTTPS 支持**：内置 TLS 支持，基于 tokio-rustls
- 📝 **多种日志方案**：支持 tracing-subscriber、env_logger、flexi_logger
- 🎯 **优雅关闭**：支持 graceful shutdown，确保请求正常处理
- 🔑 **JWT 认证**：可选的 JWT 认证中间件
- 🌐 **双栈监听**：同时支持 IPv4 和 IPv6
- ⚡ **高性能**：基于 Tokio 异步运行时
- 🛡️ **错误处理**：统一的错误处理机制
- 🔧 **请求拦截器**：可自定义请求拦截逻辑
- ⏱️ **超时控制**：可配置的连接空闲超时

## 📦 安装

在 `Cargo.toml` 中添加依赖：

```toml
[dependencies]
axum-bootstrap = "0.1"
```

### 功能特性（Features）

```toml
# 默认启用 tracing-subscriber 日志
axum-bootstrap = { version = "0.1", features = ["use_tracing_subscriber"] }

# 启用 JWT 认证功能
axum-bootstrap = { version = "0.1", features = ["jwt"] }

# 使用 env_logger
axum-bootstrap = { version = "0.1", features = ["use_env_logger"] }

# 使用 flexi_logger
axum-bootstrap = { version = "0.1", features = ["use_flexi_logger"] }
```

可用的 features：

- `use_tracing_subscriber`（默认）：使用 tracing-subscriber 进行日志记录
- `use_env_logger`：使用 env_logger 进行日志记录
- `use_flexi_logger`：使用 flexi_logger 进行日志记录
- `jwt`：启用 JWT 认证功能

### 工具函数

- `util::format`：地址格式化工具
- `util::io`：IO 相关工具，包括双栈监听器创建
- `util::json`：JSON 处理工具
- `util::tls`：TLS 配置工具

## 📖 API 文档

完整的 API 文档请访问 [docs.rs](https://docs.rs/axum-bootstrap)

## 🛣️ 路线图

### 当前限制

- ⏳ **HTTP/3 支持**：等待 hyper 和 axum 上游支持
  - [hyper HTTP/3 PR](https://github.com/hyperium/hyper/pull/3925)
  - [axum HTTP/3 Issue](https://github.com/tokio-rs/axum/issues/1096)

### 未来计划

- [ ] 更多中间件示例
- [ ] 性能监控和追踪
- [ ] 更完善的错误处理
- [ ] 数据库连接池管理
- [ ] WebSocket 支持示例

## 🤝 贡献

欢迎贡献代码！请遵循以下步骤：

1. Fork 本仓库
2. 创建特性分支 (`git checkout -b feature/AmazingFeature`)
3. 提交更改 (`git commit -m 'Add some AmazingFeature'`)
4. 推送到分支 (`git push origin feature/AmazingFeature`)
5. 开启 Pull Request

## 📝 许可证

本项目采用 MIT OR Apache-2.0 双重许可。详见 [LICENSE](LICENSE) 文件。

## 🙏 致谢

本项目参考和学习了以下项目：

1. [axum serve-with-hyper](https://github.com/tokio-rs/axum/blob/main/examples/serve-with-hyper/src/main.rs#L81)
2. [axum hyper graceful shutdown](https://github.com/hyperium/hyper-util/blob/master/examples/server_graceful.rs)
3. [axum anyhow-error-response](https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs)
4. [axum error-handling](https://github.com/tokio-rs/axum/blob/main/examples/error-handling/src/main.rs)

## 👤 作者

**arloor** - [admin@arloor.com](mailto:admin@arloor.com)

项目主页：[https://github.com/arloor/axum-bootstrap](https://github.com/arloor/axum-bootstrap)

## ⭐ Star History

如果这个项目对你有帮助，请给它一个 Star！
