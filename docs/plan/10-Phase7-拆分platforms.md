# 10 · Phase 7：拆分 `platforms/` 与插件

## 一、`src/platforms/onebot.rs` → `src/platforms/onebot/`

### 1. `onebot/mod.rs`

保留：

- `OneBotAdapter` 结构体；
- `PlatformAdapter for OneBotAdapter` 的总入口（`send`、`bot_display_name` 等 trait 方法转发）；
- 平台模块内 re-export；
- 模块文档。

### 2. `onebot/ws.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 397–535 | `ConnectionRegistry`、`RegisteredConnection`、`ConnectionHandle` |
| 561–728 | `QqListenerManager`、`PreparedQqListener` |
| 821–1210 | `onebot_ws`、`onebot_ws_on_web_port`、token 鉴权、`connection_loop`、API echo 路由 |
| 1195–1210 | 连接替换时清理 pending |

### 3. `onebot/parse.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 1507–1550 | `InboundMessage`、`MediaRef`、`FileRef`、占位文本 |
| 4778–5210 | `decode_cq_text`、text/mention/image/reply/file/face/record/video 解析 |
| 5212–5372 | `parse_message` 数组解析 |
| 5388–5495 | `onebot_id_value`、`parse_message_info`、`parse_group_member` |
| 3925–4057 | 引用消息图片合并 |
| 1619–1860 | 显示名、群名、提及解析（或放 `onebot/context.rs`） |

### 4. `onebot/send.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 5496–5552 | 回复目标前缀 |
| 6009–6351 | `send_message`、`send_segments`、`send_forward`、发送超时、图片段、文本切块、部分失败 |
| 6373–6420 | `send_timeout_for`、`read_file_capped` |
| 5552–5928 | `message_images`、上传文件、平台文件下载、群成员查询等 adapter 方法 |

### 5. `onebot/commands.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 2978–3210 | 命令解析优先级、followup/supersede |
| 3503–3740 | `execute_builtin_command`、`execute_models_command`、确认/提示文案 |

### 6. `onebot/admission.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 1354–1490 | `RateDecision`、`sends_rate_limit_notice`、`admission_for_access`、模型池应用 |
| 8208–8392 | 动态授权矩阵、好友请求 |
| 4750–4790 | `group_trigger_text` |

### 7. `onebot/approval.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 2067–2670 | 好友/加群/群邀请识别与审批前置 |
| 8394–8910 | 加群审批 AI 流程、SnowLuma flag 修正、审批结果执行 |

### 8. `onebot/files.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 4548–4740 | 文件缓存迁移、配额、下载、落盘、文件名净化 |

### 9. 验收

- [ ] `cargo test platforms::onebot` 全绿。
- [ ] QQ 冒烟：NapCat 连接、私聊/群聊消息、图片、文件、合并转发、撤回、群管理、审批与基线一致。
- [ ] `src/platforms/onebot/` 每个文件 < 1500 行。

## 二、`src/platforms/plugins/real_context/mod.rs`

### 拆分

| 新文件 | 搬入内容 |
|---|---|
| `real_context/trigger.rs` | `decide_group_trigger`、快速路径、pending/generation、接管/补救、`select_trigger`、heat/续聊状态 |
| `real_context/context.rs` | `inject_context`、水位、上下文图/文件引用、身份警告、审核通知 |
| `real_context/history.rs` | `format_history*`、`active_target_prompt`、`normalize_active_targets`、80KB 预算 |
| `real_context/runtime.rs` | `RuntimeState`、`SessionRuntime`、`RealContextPlugin` 生命周期 glue |
| `real_context/mod.rs` | descriptor、hook 实现分发、模块 re-export |

`affection.rs`、`judge.rs`、`active_judgement_skip.rs` 保持独立，但把各自入口 hook 只暴露给 `mod.rs`。

### 验收

- [ ] `cargo test real_context::` 全绿。
- [ ] 群聊触发/插话/审核/好感度冒烟与基线一致。

## 三、`src/platforms/plugins/renderer.rs`

拆成：

| 新文件 | 内容 |
|---|---|
| `renderer/mod.rs` | `MarkdownImageRenderer`、`RenderConfig`、worker 进程协议入口 |
| `renderer/worker.rs` | `WorkerProcess/WorkerSlot/spawn/exchange/run_renderer_worker` |
| `renderer/fonts.rs` | 字体目录/加载/缓存 |
| `renderer/layout.rs` | Block 模型、布局、分列、页面尺寸 |
| `renderer/draw.rs` | 绘制、表格、glyph、PNG 编码 |

### 验收

- [ ] `cargo test renderer` 全绿。
- [ ] 长回复转图字体、CJK、表格、Emoji 输出与基线一致。

## 四、`src/platforms/plugins/message_history/store.rs`

拆成：

| 新文件 | 内容 |
|---|---|
| `message_history/store/mod.rs` | `HistoryStore` 句柄、actor、命令 |
| `message_history/store/schema.rs` | SQLite schema、PRAGMA、迁移 v1–v4 |
| `message_history/store/insert.rs` | record/recall/boundary |
| `message_history/store/query.rs` | recent/search/activity |
| `message_history/store/delete.rs` | 删除、清理、vacuum |

### 验收

- [ ] `cargo test message_history` 全绿。
- [ ] FTS 搜索、撤回先于消息落库等边界行为与基线一致。

## 五、其他平台文件

- `platforms/mod.rs` 保持 `PlatformRuntime` 与事件循环，若超过 1500 行再把会话解析/限流/发送管线拆成 `runtime_session.rs`、`runtime_send.rs`。
- `platforms/types.rs` 中纯类型已在 Phase 1 下沉，本阶段只保留 `PlatformAdapter` trait 和依赖运行时的类型。
