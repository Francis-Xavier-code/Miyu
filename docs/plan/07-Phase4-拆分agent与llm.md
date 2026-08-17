# 07 · Phase 4：拆分 `agent/` 与 `llm/openai_compatible.rs`

## 一、`src/agent/mod.rs`

当前 `agent/mod.rs` 约 9.6k 行，生产逻辑集中在消息组装、工具循环、redo、持久化提取。拆成以下文件，`Agent` 结构体与公共 API 留在 `mod.rs`。

### 1. `agent/events.rs`

搬入：

- `AgentEvent` 枚举（355–456）
- `TurnJournalSink`（468–848）
- `emit_tool_progress`（849–888）

### 2. `agent/context.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 3910–3977 | `chat_messages` |
| 3978–4085 | `push_history_turn`、`compact_fork_prefix` |
| 4479–4590 | `turn_context_block_visible`、`visible_association_lines`、`live_user_index`、`last_fossil_with_prefix`、`fossil_context_messages`、`replay_fossil` |
| 4775–4809 | `summary_checkpoint_message`、`private_tool_memory` |

**铁律**：这些函数是字节前缀契约的核心，移动时逐字节保留，禁止重排、合并、格式化字符串模板。

### 3. `agent/turn_input.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 2046–2062 | `resolve_persona_reminder` |
| 2272–2424 | `prepare_user_input` |
| 5751–5909 | 图片占位符解析与路径重写 |
| 5873–6249 | host environment、runtime context 注入 |

### 4. `agent/tool_loop.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 1338–1515 | `execute_parallel_task_calls` |
| 2922–3864 | `chat_with_tools` 主循环 |
| 3866–3885 | `initial_loaded_tools` |

依赖 `agent/context.rs` 的 request message 构建，循环状态保持当前私有字段访问方式（`pub(super)` 调整）。

### 5. `agent/tool_reports.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 1152–1202 | `spill_tool_output`、`spill_replacement` |
| 4187–4381 | `tool_output_succeeded`、artifact 自动发布判断 |
| 4645–4809 | `tool_call_footprint`、`extract_persistable_tool_report`、`private_tool_memory` |
| 4882–5065 | `derive_tool_flow`、assistant 消息重建 |
| 5351–5637 | 驱逐/归档、报告压缩、`truncate_chars` |

### 6. `agent/redo.rs`

搬入：

- `PendingRedoGuard`（101–158）
- `RedoPromptInput`（161–170）
- `redo_stream_turn`（1855–2024）
- `redo_checkpoint_payload` 等 redo 相关 helper

### 7. `agent/queue.rs`

搬入：

- `QueueIngressBarrier/State/Reservation`（220–280）
- `consume_queued_prompts` 系列（1578–1656）

### 8. `agent/overflow_glue.rs`

当前 `handle_overflow`、`trim_visible_context`、`compact_now` 与 `compact.rs` 强相关。做法：

- 保留 `agent/compact.rs`、`agent/overflow.rs` 不动；
- 把 `mod.rs` 中 1657–1734、2432–2630、2631–2858 的 trim/handle/compact glue 移入 `agent/overflow_glue.rs`，接口全部 `pub(super)`。

### 9. `agent/mod.rs` 最终保留

- `Agent` 结构体与公开方法：`new/new_for_audience/chat_stream*/redo_stream*/reload_config/reset_memory` 等；
- 各子模块声明；
- 原 `mod.rs` 顶部文档注释。

## 二、`src/llm/openai_compatible.rs`

目标目录 `src/llm/openai_compatible/`，每个文件只处理一种协议或一个横切主题。

### 1. `types.rs`

搬入：

| 原位置 | 类型 |
|---|---|
| 3024–3148 | `ChatRequest`、`ResponsesRequest`、`AnthropicRequest/Message/ContentBlock/Tool` |
| 3525–3825 | 各响应/流事件/usage 反序列化结构 |
| 4018–4248 | `ToolCallAccumulator`、`AnthropicToolAccumulator`、`ResponsesToolAccumulator`、`Utf8LineBuffer`、`SseDataBuffer` |

### 2. `error.rs`

搬入：

| 原位置 | 类型/函数 |
|---|---|
| 134–367 | `TransportFailureKind`、`TransportFailure`、`HttpFailureKind`、`HttpStatusFailure`、`classify_provider_error_body`、`normalize_error_signal`、`format_error_chain` |

### 3. `endpoint.rs`

搬入：

| 原位置 | 类型/函数 |
|---|---|
| 780–998 | `LlmEndpoint`、`LlmScheduler`、端点构造/冷却/排序/标记 |
| 1621–1850 | `chat_stream_inner` 的端点重试主循环 |

### 4. `thinking.rs`

搬入：

| 原位置 | 类型/函数 |
|---|---|
| 420–737 | thinking 能力判断、variant 选项、持久化偏好 |
| 1260–1477 | 客户端 thinking 方法 |
| 7863–8394 | variant 合并/协议映射（后段） |

### 5. `chat.rs`

搬入：

| 原位置 | 类型/函数 |
|---|---|
| 1900–2052 | Chat Completions 请求/发送 |
| 2053–2265 | 传输重试、chunk 读取、zen 兼容重试 |
| 2266–2520 | Chat SSE/非流消费、失败转换 |
| 3525–3608 | Chat 响应/choice/delta 反序列化（或放 types.rs） |
| 4249–4409 | Chat SSE 行处理 |

### 6. `responses.rs`

搬入：

| 原位置 | 类型/函数 |
|---|---|
| 2704–2883 | Responses 请求与 SSE 主循环 |
| 2884–3023 | 协议判断/兼容探测 |
| 4413–4754 | Responses SSE 事件处理 |
| 5127–5246 | `finalize_responses_stream_result` |

### 7. `anthropic.rs`

搬入：

| 原位置 | 类型/函数 |
|---|---|
| 2503–2632 | Anthropic 请求/发送 |
| 2633–2703 | Anthropic SSE 消费 |
| 4756–5033 | Anthropic 事件处理、usage 合并 |

### 8. `stream_common.rs`

搬入 `ChatStreamChunk`、`ChatStreamKind`、stream 缓冲/UTF-8 边界等被三种协议共用的流式 helper。

### 9. `mod.rs`

`OpenAiCompatibleClient` 结构体、构造方法、公开 API 保留在 `mod.rs`；内部实现调用子模块 `pub(super)` 函数。

## 三、本阶段特殊验收

1. **工具循环缓存回归**：用 Phase 0 的固定会话重放，比较拆分前后第二轮 `cache_read` 绝对值；要求不下降（provider 波动需记录）。
2. **三协议 fixture 测试**：现有 DeepSeek/Anthropic/Responses 相关测试必须原样通过；测试模块随实现一起搬。
3. **`cargo test llm:: agent::` 全绿**。
4. 文件规模：`agent/mod.rs` < 1200 行；`llm/openai_compatible/` 每个文件 < 1500 行。
