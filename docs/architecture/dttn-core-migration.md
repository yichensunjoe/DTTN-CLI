# DTTN Core 迁移设计

## 目标

将当前代码库改造成模型中立、供应商中立的 DTTN Agent 平台，同时保持每一阶段都可构建、可测试、可回滚。

最终状态：

- 用户命令统一为 `dttn`。
- 用户配置目录统一为 `DTTN_HOME`、`~/.dttn` 和 `/etc/dttn`。
- Agent Core 不硬编码具体模型，企业发行配置决定默认模型。
- 业务代码仅依赖模型角色和 Provider 接口。
- 认证、模型目录、推理、遥测、更新和远程服务均通过可替换 Provider 注入。
- 源码、日志、协议字段、配置项和用户界面不再出现遗留供应商品牌。

## 不变量

迁移期间必须保持：

1. Agent Loop、Session Actor 和工具运行时行为不因品牌改造发生隐式变化。
2. 每个阶段都可以从上一阶段单独回滚。
3. 不在同一个提交中同时进行大规模符号重命名和行为修改。
4. 配置迁移必须显式，不能静默覆盖用户数据。
5. macOS 与 Windows 的路径、进程名和终端行为分别测试。

## 阶段划分

### Phase 1：发行边界

- 默认模型切换为企业发行模型。
- 可执行文件改为 `dttn`。
- 用户配置主目录切换为 `DTTN_HOME` 和 `~/.dttn`。
- 重写公开文档、安全策略和配置示例。
- 增加品牌与默认配置自动检查。

本阶段允许保留内部兼容包装，以保证现有调用点继续编译。兼容包装不得用于新代码。

### Phase 2：运行时命名

- 将默认 Agent 类型改为 `dttn-code-agent`。
- 将启动信息、错误提示、日志服务名和进程识别统一为 DTTN。
- 将旧环境变量迁移为 `DTTN_*`。
- 将配置字段改为中立命名，例如 `inference_base_url`、`agent_gateway_url`。
- 为旧配置提供一次性离线迁移工具，不在核心运行时长期保留双读逻辑。

### Phase 3：Provider 抽象

引入以下接口：

```rust
pub trait ModelProvider {
    fn catalog(&self) -> ModelCatalog;
    fn sampler(&self, model: &ModelId) -> Result<Box<dyn ModelSampler>>;
}

pub trait AuthProvider {
    fn credentials(&self) -> Result<CredentialSet>;
}

pub trait ServiceEndpoints {
    fn inference_base_url(&self) -> &str;
    fn model_catalog_url(&self) -> Option<&str>;
    fn telemetry_endpoint(&self) -> Option<&str>;
}
```

业务模块只使用中立接口，不读取供应商专用字段。

### Phase 4：协议与持久化

- 将内部 ACP 扩展键集中到单一协议模块。
- 使用 DTTN 自有命名空间。
- 为 Session、Trace 和持久化数据增加 schema version。
- 编写旧数据离线迁移器和回滚工具。

### Phase 5：crate 与目录重命名

最后处理内部 crate、模块和目录名称。此阶段只做机械重命名，不同时修改业务行为。

推荐顺序：

1. leaf crates；
2. tools / workspace；
3. sampler / models / config；
4. shell / pager；
5. composition root；
6. 根工作区依赖别名与构建脚本。

## 验证矩阵

每个阶段至少执行：

- `cargo test`：受影响的小型 crate。
- `cargo check --bin dttn`：组合入口。
- macOS TUI 启动、Resize、iTerm2 图片、退出恢复。
- Windows ConPTY 启动、中文显示、路径和子进程。
- Headless 与 ACP Stdio 冒烟测试。
- 配置优先级和模型角色解析测试。
- 无凭据、Endpoint 超时、模型不存在和工具取消测试。

## 完成标准

迁移完成需要同时满足：

- 全仓文本扫描不再出现遗留供应商品牌，许可证和第三方 NOTICE 中依法必须保留的内容除外。
- 默认启动不访问外部供应商服务。
- 未配置公司 Endpoint 时 fail closed，并提供明确错误。
- `dttn` 在 macOS 和 Windows CI 中构建通过。
- 现有 Agent 轨迹测试和工具调用测试无回归。
