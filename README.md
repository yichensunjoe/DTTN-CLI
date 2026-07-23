<div align="center">

# DTTN-CLI

**面向企业研发场景的高性能终端 AI Agent**

DTTN-CLI 是一套使用 Rust 构建的终端 Agent 运行时与全屏 TUI。它能够理解代码仓库、编辑文件、执行命令、调用工具、连接 MCP 服务，并通过 ACP 以交互式、无头模式或编辑器集成方式运行。

</div>

---

## 项目目标

DTTN-CLI 的长期目标是提供一套模型中立、可观测、可扩展、可恢复的生产级 Agent 平台：

- 支持公司内部模型和兼容接口，不绑定单一模型供应商。
- 提供清晰且有边界的 Agent Loop、工具调用、状态管理与任务编排。
- 支持超时、重试、取消、中断恢复和 Human-in-the-loop。
- 支持 MCP、插件、Skills、工作区隔离与权限控制。
- 优先保证执行性能、低延迟、并发能力和可测试性。
- 面向 macOS 与 Windows 发布，并通过平台级 CI 验证。

## 当前发行默认值

当前 DTTN 发行配置使用以下默认模型角色：

```text
primary           = agnes-2.0-flash
web_search        = agnes-2.0-flash
image_description = agnes-2.0-flash
session_summary   = agnes-2.0-flash
```

这些值是发行配置，不是 Agent Core 的永久模型绑定。运行时仍可通过 CLI、环境变量、配置文件或远程模型目录覆盖。

## 从源码运行

### 环境要求

- Rust：使用 `rust-toolchain.toml` 固定工具链。
- protoc：优先使用仓库中的启动器，也可通过 `PATH` 或 `PROTOC` 提供。
- macOS：主要开发与验证平台。
- Windows：目标发布平台，必须通过独立 CI 和 ConPTY 冒烟测试后再标记为稳定。

### 常用命令

```sh
cargo run --bin dttn
cargo build --release --bin dttn
cargo check --bin dttn
python scripts/check_dttn_phase1.py
cargo fmt --all
```

Release 二进制输出：

```text
target/release/dttn       # macOS / Linux
target/release/dttn.exe   # Windows
```

## Windows x64

Windows 版本通过独立流水线生成 `dttn-windows-x86_64.zip` 和 SHA-256 校验文件。仓库提供无管理员权限的 PowerShell 安装器，并在 CI 中验证编译、二进制启动、压缩包校验和离线安装全过程。

完整安装、源码构建、卸载步骤和当前安全边界见 [`docs/windows.md`](docs/windows.md)。Windows 上现有 `sandbox-enforce` 功能尚无内核级后端，运行外部命令时应按未沙箱隔离处理。

## 运行模式

DTTN-CLI 支持以下入口：

- TUI：全屏交互式 Agent。
- Headless：用于脚本、自动化和 CI。
- ACP Stdio：通过标准输入输出与编辑器或上层客户端通信。
- Leader：提供长生命周期 Agent 进程、连接复用与会话恢复。

## 架构边界

```text
DTTN CLI Composition Root
          │
          ├── TUI / Headless / ACP / Leader
          │
          ▼
Agent Runtime and Session Actors
          │
          ├── Model Sampling
          ├── Tool Runtime
          ├── Workspace and VCS
          ├── MCP / Plugins / Skills
          ├── Persistence and Recovery
          └── Telemetry and Tracing
```

核心设计原则：

1. 模型调用、工具执行和状态更新解耦。
2. 所有执行循环具备步数、时间、Token 和成本边界。
3. 工具调用支持超时、取消、重试和幂等控制。
4. 会话状态可持久化、恢复、追踪和回放。
5. 不可信输入、外部工具和项目级插件默认受权限边界约束。

## 配置原则

模型解析优先级：

```text
CLI 参数 > 环境变量 > config.toml > 远程设置 > 发行默认值
```

建议在企业环境中通过受管配置明确指定：

- 默认模型及各模型角色。
- 模型目录和推理 Endpoint。
- 允许使用的模型集合。
- 工具权限与沙箱策略。
- MCP 服务和插件来源。
- 遥测、日志和数据上传策略。

## 开发规范

- 根工作区配置由生成流程维护，优先修改各 crate 的配置文件。
- 构建和测试应尽量限定到目标包，避免无必要的全工作区构建。
- 所有 Agent 行为修改必须同时提供单元测试、轨迹测试或集成测试。
- 不允许在缺少基准数据时宣称延迟、成本或成功率得到提升。
- 用户可见品牌、命令和配置命名统一使用 DTTN。
- 内部遗留 crate 名称将在分阶段迁移中逐步消除，不应继续新增依赖。

## 许可证

第一方代码使用 Apache License 2.0。第三方和内嵌依赖继续遵循各自许可证与 NOTICE 文件。
