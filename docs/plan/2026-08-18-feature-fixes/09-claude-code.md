# 09 · 接入 Claude Code（使用其订阅登录态）

## 1. 现状与本机事实

- 本机安装 Claude Code `2.1.220`（`claude --version` 已验证），并已有 OAuth 订阅登录（`~/.claude.json` 存在 `oauthAccount`）。
- Miyu 当前只有自己的 provider/LLM 客户端，没有任何调用本地 `claude` CLI 的工具。
- Claude Code 提供稳定的 headless 入口：`claude -p --output-format json`；该模式使用用户既有 Claude 登录/订阅，非交互目录跳过 workspace trust 提示。

## 2. 推荐形态（D10）

新增一个独立工具 **`claude_code`**，而不是塞进 `task` tier：

- `task` 是 Miyu 自己的子代理循环，tier 语义是“模型档位”；Claude Code 是外部 agent runtime，输入输出、权限、会话续跑、成本模型都不同，混在一起会让两者都难维护。
- 只在本机 owner 面注册：终端/WebUI 本地会话（`platform_context.is_none()`），QQ 等通讯平台**不注册**，避免把订阅和本机权限暴露给群聊。
- 工具不在子代理中递归开放（加入 `SUBAGENT_EXCLUDED` 同族排除表）。

## 3. 工具设计

### 3.1 Schema（模型可见）

```json
{
  "name": "claude_code",
  "description": "Run the locally installed Claude Code CLI in headless mode using the user's Claude subscription login. ...",
  "parameters": {
    "type": "object",
    "properties": {
      "prompt": {"type": "string", "description": "Complete task prompt for Claude Code. Include required context; the CLI does not see Miyu's conversation unless explicitly included."},
      "cwd": {"type": "string", "description": "Optional working directory. Defaults to the current session workspace."},
      "model": {"type": "string", "description": "Optional Claude model alias or full name. Omit to use the user's default subscription model."},
      "append_system_prompt": {"type": "string", "description": "Optional extra system prompt appended to Claude Code's default."}
    },
    "required": ["prompt"],
    "additionalProperties": false
  }
}
```

### 3.2 执行语义

1. 用 `tokio::process::Command` 调 `claude`（从 PATH 解析；允许配置 `plugins.claude_code.binary` 覆盖）。
2. 参数：`-p --output-format json --permission-mode <configured>`；`cwd` 为会话 workspace；环境继承当前 Miyu daemon（含 HOME，才能读到订阅登录态）。
3. 超时：配置 `timeout_seconds` 默认 600，tool 层用 `tokio::time::timeout` 包裹，超时 kill 进程组。
4. 输出限制：stdout 上限（默认 512 KiB）；超限截断并注明，不把无限 CLI 输出灌进上下文。
5. 进程内互斥/并发：同一 session 同时最多 1 个 `claude_code`（默认），防止多个订阅会话并发抢额度；配置可调。
6. 错误分类：binary missing / timeout / 非零退出 / JSON 解析失败，返回英文错误并保留原始 tail。

### 3.3 返回内容

解析 `--output-format json` 的 `result` 字段作为工具输出；附上可选的：

```json
{
  "ok": true,
  "result": "...",
  "session_id": "claude session id if present",
  "cost_usd": 0.0,
  "duration_ms": 123,
  "truncated": false
}
```

具体字段以本机 Claude Code 实际 JSON 输出为准，实现时先用 `claude -p` 的文档/本地 schema 核实。

### 3.4 记账与审计

- 不把 Claude 用量塞进 Miyu `Usage`（那是 OpenAI 兼容 usage 口径）。新增独立统计：`claude_code` 调用写入隐藏审计会话（类似 subagent），字段含 `cost_usd`、`duration_ms`、模型、session id、prompt 长度。
- REPL/WebUI 显示 Claude Code 成本时明确标注 `$`，不与 token Σ 混加。

## 4. 配置

`plugins.claude_code`：

```jsonc
{
  "enabled": true,              // 默认推荐 true；注册时自动探测 binary，缺失时工具返回明确错误
  "binary": "",                 // 空 = PATH 上的 claude
  "permission_mode": "acceptEdits", // D17 待确认；可选 default/plan/acceptEdits/bypassPermissions
  "timeout_seconds": 600,
  "max_output_bytes": 524288,
  "max_concurrent_per_session": 1
}
```

## 5. 修改文件清单

- 新 `src/tools/claude_code/{mod.rs,runner.rs,audit.rs}`（保持小文件）。
- `src/tools/mod.rs`：注册与排除表。
- `src/web/turns/task.rs`：`platform_context.is_none()` 时按配置注册。
- `src/config/tool_plugins.rs`、`src/config/defaults.rs`、`src/config_tui/plugin_settings.rs`：配置项与 TUI。
- `src/state/*`：Claude Code 审计会话/统计列。
- `src/tools/descriptions/claude_code.json`（英文）。

## 6. 缓存与安全

- 该工具 schema 只在本机 owner 注册，不影响平台 restricted registry。
- 工具目录在本机会话内保持字节稳定，不要把当前 model/cost 拼进 description。
- 权限由注册面承担，不在 prompt 里写“允许/禁止”。

## 7. 验收

1. PATH 放一个假 `claude` 脚本（fixture），断言 Miyu 调用参数、cwd、env、超时 kill、JSON 解析与截断。
2. QQ 群/私聊会话的工具目录不含 `claude_code`；终端/WebUI 本地会话包含。
3. 真实环境手动 1 次最小调用由用户验证订阅可用（**测试不自动跑真实订阅**）。
4. 审计记录成本与模型，不污染 token Σ。
5. `bash scripts/refactor-check.sh` 全绿。

## 8. 待确认

- D17：默认 permission mode（推荐 `acceptEdits`；`plan` 更安全但体验割裂）。
- D18：默认启用还是默认关闭（推荐启用但自动探测 binary）。
- D19：是否允许 `claude_code` 递归调用 Claude Code 自己的 subagent（默认允许，由 Claude 内部处理）。

---

## 9. 施工记录（2026-08-20，claude-code 分支）

用户在 08-20 把范围从"独立工具"扩大为**三件套全做**（D10 改判），并限定
"仅本人渠道"。全部落地并经真机订阅验证：

### 交付物

1. **`claude-code` 供应商协议**（中转层核心，`src/llm/openai_compatible/claude_code/`
   四小件：mod/session/payload/stream）：
   - 传输 = `claude -p` 子进程 stream-json 双向流；`--system-prompt` 整体替换
     （人格原样过去，无 CLI 身份与 CLAUDE.md 注入,实测首请求仅 ~246 input tok）；
     内置工具全关（`--tools ""`）。
   - **会话续传**：进程内逐消息哈希链匹配 append-only 前缀,命中则 `--resume`
     只发增量（真机实测第二轮 stdin 仅一条新输入）；redo/compact/重启自动整段
     转写重放（`<conversation-history>` 块）；claude 侧会话丢失（"No conversation
     found"）一次性自愈重放。
   - 用量按 Anthropic 口径归一（prompt = input+cache_read+cache_write）,进现有
     cache-usage 记账;订阅限流/登录失效翻译为 429/401 进端点冷却与故障转移；
     流空闲看门狗杀进程组。辅助请求（compact/记忆整理等 scope≠chat）走
     `--no-session-persistence` 一次性会话,不挂桥。
   - 平台门禁：`with_platform_delivery` 在平台回合拒绝该协议端点（订阅条款）。
   - 校验豁免：该协议 `base_url` 可为空（io.rs）。
2. **`miyu mcp-serve`**（隐藏子命令，`src/cli/mcp_serve.rs`）：MCP stdio server,
   与 `miyu tool-call` 同源——daemon 存活走 IPC ToolCatalog/ToolCall（会话→模式
   →registry,guard 管线齐备）,直连本地兜底。`ToolCatalog` 新增 `full` 位一次拿
   全量合同。供应商自动以 `--mcp-config` 挂桥（env 显式带 MIYU_SESSION/
   MIYU_TURN_ORIGIN/MIYU_HOME/XDG_RUNTIME_DIR）。
3. **`claude_code` 委托工具**：按本文 §3 落地,偏离两处经用户同意——审计走
   JSONL（`logs/claude-code-usage.jsonl`）不建 DB 表；新增 `resume` 参数。
   D17=acceptEdits、D18=默认启用、D19=允许。
4. 配置 `plugins.claude_code`（工具与供应商共用）,TUI 协议下拉加 `claude-code`。

### 顺手修的存量 bug

- **工具桥在阅后即焚(ask)会话里全 404**：ToolCall/ToolCatalog 的会话解析只认
  user kind,单次 CLI 形态下 run_command 脚本调桥同样中招；改用 TURN_TARGET_KINDS。

### 真机验证（订阅 haiku,08-20）

| 验证项 | 结果 |
|---|---|
| 纯文本中转（一次性会话） | "回复 OK"→"OK",思考流正常透传 |
| 同会话第二轮续传 | `--resume` 命中,stdin 只有增量一条（testkit/claude-code/run.py,PASS） |
| MCP 工具闭环 | `sqrt(7.317)*ln(93.4)` 经 claude→mcp-serve→daemon→计算器,回答 12.272270123540252 与本地计算逐位一致 |
| MCP 握手/目录/调用（不花额度） | initialize/tools/list 59 个工具全合同/tools/call 3978 |
| 平台门禁/限流分类/binary 缺失/改写重放 | 假 claude 测具 5 项 + 会话链/载荷 4 项,全绿 |

已知限制：①订阅无按 token 计费,成本列仅供参考;②转写重放时人格预设对话会作
为历史进入 claude 上下文（模型可正确区分,但"上一条"语义可能指向预设）;③
`claude_code` 工具在中转模式下经 MCP 可见,存在 claude 套 claude 的理论递归
（D19 本就允许,深度由订阅限流自然约束）;④平台侧辅助请求（qq-judge 等）未
挂平台门禁,如平台会话把主池配成 claude-code 需自行避免。
