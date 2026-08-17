# 08 · Phase 5：拆分 `config.rs` 与 `config_tui.rs`

## 一、`src/config.rs` → `src/config/`

### 拆分原则

- **Schema 按领域拆**，方法保留在类型定义旁。
- **默认值统一进 `defaults.rs`**，避免散落。
- **迁移逻辑独立**，与 `AppConfig::load/save` 分开。
- 所有 `serde` 字段名、默认值、旧键迁移映射**一字不改**。

### 1. `config/mod.rs`

保留：

- `AppConfig` 顶层结构；
- `load / load_or_default / init_files / save / migrate`；
- `memory_config()` 复制逻辑；
- 各子模块 re-export，保证 `crate::config::Xxx` 外部路径完全兼容。

### 2. `config/schema_app.rs`

搬入：`AppConfig` 除顶层外，还包括 `CacheConfig`、`ContextConfig`、`ToolsConfig`、`McpConfig`、`SkillsConfig`、`DisplayConfig`、`NotificationsConfig`、`PromptConfig`、`SubagentTiersConfig`、`ModelTier`、`ActiveProviderModelConfig`、`ModelCostConfig` 等非平台/非插件配置。

### 3. `config/schema_provider.rs`

搬入：`ProviderConfig`、`ResolvedProviderKey`、`ProviderModelChoice`、模型温度/成本/模态相关方法、`resolved_api_keys`、内置供应商模板与 normalize 逻辑。

### 4. `config/schema_platform.rs`

搬入：`PlatformsConfig`、`OneBotConfig`、`QqPrivateChatsConfig`、`QqGroupChatsConfig`、`PlatformRateLimit`、`PlatformConversation*`、`PlatformModelRoute`、平台会话限流与模型池路由方法。

### 5. `config/schema_plugins.rs`

搬入：`PluginsConfig` 与全部插件子配置（web、web_images、vision、image_generation、memes、knowledge_base、deep_research、api_quota、diagnostics 等），以及平台插件设置（QQ 群管理、撤回、表情收集、入群审批、消息历史、real_context）。

`real_context` 配置约 900 行，可独立成 `config/schema_real_context.rs`。

### 6. `config/model_pool.rs`

搬入：`text_provider_model_choices`、`active_provider_model_choices`、多模态池、`vision_provider_choice`、模型增删改查、`toggle_active_*`、`active_context_window`、上下文窗口查询、模型引用 prune/rename。

### 7. `config/prompt_paths.rs`

搬入：`system_prompt_for`、`base/custom_system_prompt`、prompts/identities/persona 路径、persona scope、dev_scoped、`validate_persona_files`、persona 数据目录。

### 8. `config/defaults.rs`

搬入全部 `default_*()` 与 `impl Default` 中的常量默认值。命名保持原样。

### 9. `config/validate.rs`

搬入 `validate()` 和各 `validate_*` 函数；保留报错文案与顺序。

### 10. `config/migrate.rs`

搬入 `migrate`、normalize 系列、旧输出目录 `remap/relocate`、`DEPRECATED` 表与 real_context 旧键映射。

### 11. 测试拆分

原 `config.rs` 尾部 6217–8688 的测试按 schema 领域搬到各子文件 `#[cfg(test)] mod tests`；契约测试（默认值、旧键迁移）集中在 `config/tests/migrations.rs`。

## 二、`src/config_tui.rs` → `src/config_tui/`

`config_tui` 只有一个公开入口 `run()`，非常适合按页面拆。

### 1. `config_tui/mod.rs`

保留：`run`、TerminalSession、主菜单、保存确认、错误兜底。

### 2. `config_tui/widgets.rs`

搬入：

| 原位置 | 内容 |
|---|---|
| 8270–8960 | `draw_menu`、`draw_form`、`Field`、`run_form*`、选择器、文本编辑、布尔/枚举控件 |

这是最值得优先抽出的部分：通用控件无业务逻辑，测试最多。

### 3. `config_tui/provider_page.rs`

搬入 `ProviderBrowser` 三列浏览、模型抓取、provider/model 表单、文本/多模态/embedding 选择、子代理档位。

### 4. `config_tui/plugin_page.rs`

搬入 13 插件菜单、字段表、`plugin_fields`、`apply_plugin_fields`、api quota 账号管理。

### 5. `config_tui/prompt_page.rs`

搬入自定义提示词、普通/Dev 人格、用户身份、persona 管理、身份管理、dialogs/hints 编辑。

### 6. `config_tui/platform_page.rs`

搬入命令前缀/权限、QQ 26 项菜单、会话限流、限流、ID 列表、模型路由、QQ 插件子页、入群审批、消息历史、表情口袋、真实上下文、回复处理。

该文件可能仍超 1500 行；可继续拆 `platform_qq.rs` 与 `platform_real_context.rs`。

### 7. `config_tui/settings_page.rs`

搬入全局设置 16 字段、校验、模型选择控件。

### 8. 验收

- [ ] `cargo test config:: config_tui::` 全绿。
- [ ] `miyu config validate` 与基线一致。
- [ ] TUI 冒烟：主菜单十项可进入、保存/退出、脏检测、provider 三列导航与基线一致。
- [ ] 配置 JSON 默认值与基线逐字节一致。
