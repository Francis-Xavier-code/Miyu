//! Claude Code CLI 中转协议(`protocol = "claude-code"`)。
//!
//! 传输层不是 HTTP,是本机 `claude` 子进程的 stream-json 双向流:CLI 用用户
//! 既有的订阅登录态,Miyu 不经手任何凭据。工具循环的所有权在 claude 侧——
//! Miyu 的工具经 `miyu mcp-serve` 桥挂进去(内层调用照走 daemon 的 guard 管
//! 线),所以这条线对 Miyu 的回合循环呈现为「一次请求、纯文本(+思考)回来、
//! 永远没有 tool_calls」。
//!
//! 会话续传:Miyu 的消息历史是严格 append-only 的(缓存体制的成果),所以
//! 「这次请求是否延续上次」用逐消息哈希链判定——匹配上就 `--resume` 只发增
//! 量;对不上(redo/compact/daemon 重启)就重开 claude 会话全量重放。见
//! [`session`]。

mod payload;
mod session;
mod stream;

use crate::llm::openai_compatible::*;

/// 客户端构造期解析好的运行时参数(binary/工具作用域/权限模式),端点间共享。
pub(in crate::llm::openai_compatible) struct ClaudeCodeRuntime {
    pub(in crate::llm::openai_compatible) binary: PathBuf,
    /// Miyu home 根(MIYU_HOME):MCP 桥子进程必须显式带上,不赌 claude 会
    /// 把父环境透传给 MCP server——透传缺失时桥会静默连到默认 home 的
    /// daemon,拿错目录还不报错。
    pub(in crate::llm::openai_compatible) home: PathBuf,
    /// claude 原生工具(Bash/Edit/Read…)的模式作用域:off/dev/normal/all。
    pub(in crate::llm::openai_compatible) native_tools: String,
    /// Miyu 工具经 MCP 桥挂给 claude 的模式作用域:off/dev/normal/all。
    pub(in crate::llm::openai_compatible) miyu_tools: String,
    /// 原生工具开启时的 --permission-mode(无头模式没有交互审批)。
    pub(in crate::llm::openai_compatible) permission_mode: String,
    pub(in crate::llm::openai_compatible) idle_timeout: Duration,
    pub(in crate::llm::openai_compatible) prefer_subscription: bool,
}

/// 模式作用域判定:会话是 dev 还是 normal 由 Agent 构造时经
/// `with_claude_code_dev_mode` 打到客户端上。未知值按 off 兜底。
pub(in crate::llm::openai_compatible) fn scope_allows(scope: &str, dev_mode: bool) -> bool {
    match scope.trim().to_ascii_lowercase().as_str() {
        "all" => true,
        "dev" => dev_mode,
        "normal" => !dev_mode,
        _ => false,
    }
}

impl ClaudeCodeRuntime {
    pub(in crate::llm::openai_compatible) fn from_config(
        config: &AppConfig,
        paths: &MiyuPaths,
    ) -> Self {
        let plugin = &config.plugins.claude_code;
        let binary = if plugin.binary.trim().is_empty() {
            PathBuf::from("claude")
        } else {
            PathBuf::from(plugin.binary.trim())
        };
        Self {
            binary,
            home: paths.root_dir.clone(),
            native_tools: plugin.native_tools.clone(),
            miyu_tools: plugin.miyu_tools.clone(),
            permission_mode: plugin.permission_mode.clone(),
            idle_timeout: Duration::from_secs(plugin.idle_timeout_seconds.max(30)),
            prefer_subscription: plugin.prefer_subscription,
        }
    }
}

impl OpenAiCompatibleClient {
    pub(crate) async fn chat_claude_code_stream<F>(
        &self,
        messages: Vec<ChatMessage>,
        _tools: Vec<ToolDefinition>,
        request_id: &str,
        on_chunk: &mut F,
    ) -> Result<ChatResult>
    where
        F: FnMut(ChatStreamChunk) -> Result<()>,
    {
        let runtime = self
            .claude_code
            .clone()
            .context("claude-code runtime was not initialized for this client")?;
        if self.claude_code_platform_blocked {
            bail!(t(
                "the claude-code provider relays through the user's Claude subscription and is limited to owner sessions; platform messages must use a different provider",
                "claude-code 供应商走用户订阅额度,仅限本机会话使用;平台消息请配置其他供应商"
            ));
        }
        let model = self.provider.default_model.clone();
        let (system_prompt, conversation) = payload::split_system(messages);
        // 辅助请求(压缩摘要、标题、judge)一次一个 claude 会话即可,不建持久
        // 会话也不参与续传匹配,免得污染主对话的会话映射。
        let ephemeral = self.request_scope != "chat";
        // 一律用会话工作区(与 run_command 同源):原生工具在这里操作文件,
        // 无工具时 cwd 无关紧要。回合作用域外(测试/辅助)回退进程 cwd。
        let workdir = crate::tools::workspace::effective_workdir();
        let miyu_session = crate::tools::workspace::try_session();
        let miyu_session = miyu_session.as_deref();
        let chain = session::prefix_chain(&self.provider.id, &model, &system_prompt, &conversation);
        let resumable = if ephemeral {
            None
        } else {
            session::find_resumable(
                &self.provider.id,
                &model,
                miyu_session,
                &chain,
                conversation.len(),
            )
        };
        let outcome = {
            let (resume_id, covered) = match resumable.clone() {
                Some((id, len)) => (Some(id), len),
                None => (None, 0),
            };
            let delta = &conversation[covered..];
            let payload = payload::render_user_payload(delta);
            let args = self.claude_code_args(
                &runtime,
                &model,
                &system_prompt,
                resume_id.as_deref(),
                ephemeral,
            );
            crate::llm::request_log::record(
                &self.provider.id,
                &model,
                "claude-code",
                self.request_scope,
                &runtime.binary.display().to_string(),
                // conversation 是续传哈希链的原料,录下来才能诊断"为什么没命中"。
                &json!({ "args": args, "stdin": payload, "conversation": conversation }),
            );
            stream::run_claude_turn(&runtime, &workdir, &args, &payload, request_id, on_chunk).await
        };
        let outcome = match outcome {
            Err(error)
                if resumable.is_some() && stream::resume_session_lost(&error) =>
            {
                // claude 侧会话被清理(过期/手动删除):忘掉映射,整段全量重放
                // 一次。只对「会话找不到」类错误自愈,限流/登录错误照常上抛。
                tracing::warn!(
                    request_id,
                    error = %format!("{error:#}"),
                    "claude-code resume target is gone; replaying the full conversation in a fresh session"
                );
                if let Some((session_id, _)) = &resumable {
                    session::forget_session(session_id);
                }
                let payload = payload::render_user_payload(&conversation);
                let args =
                    self.claude_code_args(&runtime, &model, &system_prompt, None, ephemeral);
                stream::run_claude_turn(&runtime, &workdir, &args, &payload, request_id, on_chunk).await?
            }
            other => other?,
        };
        if !ephemeral {
            if let Some(claude_session) = &outcome.claude_session {
                // 预测下一轮的前缀:本轮回放化石 = 已发送的会话消息 + 一条
                // assistant 正文(思考回放已退役,无工具轮就是纯文本)。预测
                // 若与实际化石有分歧,下一轮匹配不上,自动退化为全量重放——
                // 只损失效率,不损失正确性。
                let content = outcome.result.content.clone();
                if !content.trim().is_empty() {
                    let predicted = ChatMessage::assistant(content, None);
                    let next_hash =
                        session::extend_chain(chain[conversation.len()], &predicted);
                    session::record_session(
                        &self.provider.id,
                        &model,
                        miyu_session,
                        conversation.len() + 1,
                        next_hash,
                        claude_session.clone(),
                    );
                }
            }
        }
        Ok(outcome.result)
    }

    fn claude_code_args(
        &self,
        runtime: &ClaudeCodeRuntime,
        model: &str,
        system_prompt: &str,
        resume: Option<&str>,
        ephemeral: bool,
    ) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--verbose",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--include-partial-messages",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        args.push("--model".into());
        args.push(model.to_string());
        // 用户在模型菜单显式配了上下文窗口,就同步给 claude 的自动压缩阈值
        // (compact 后会话 id 不变,续传不断);没配则随 claude 自身默认。
        // CLI 只收 100k–1M,低于下限的配置不透传。
        if let Some(window) = self.provider.model_context_window.get(model) {
            if (100_000..=1_000_000).contains(window) {
                args.push("--autocompact".into());
                args.push(window.to_string());
            }
        }
        // 思考档:Miyu 的 thinking-variant 选择映射到 CLI 的 --effort。
        if let Some((_, variant)) = self.selected_reasoning_variant() {
            if let crate::models_cache::ReasoningSetting::Effort(effort) = variant.setting {
                args.push("--effort".into());
                args.push(effort);
            }
        }
        if !system_prompt.trim().is_empty() {
            // 整体替换默认系统提示词:人格/开发提示词原样过去,同时甩掉
            // Claude Code 自带的 CLI 身份与 CLAUDE.md 注入。
            args.push("--system-prompt".into());
            args.push(system_prompt.to_string());
        }
        // 工具面按双四档作用域装配;辅助请求(摘要/标题)永远无工具。
        let native_on =
            !ephemeral && scope_allows(&runtime.native_tools, self.claude_code_dev_mode);
        let miyu_on = !ephemeral && scope_allows(&runtime.miyu_tools, self.claude_code_dev_mode);
        if native_on {
            // claude 原生工具(训练分布内)开放;无头模式没有交互审批,
            // 权限模式决定 Bash 等是否可用(默认 bypassPermissions)。
            args.push("--permission-mode".into());
            args.push(runtime.permission_mode.clone());
        } else {
            args.push("--tools".into());
            args.push(String::new());
        }
        args.push("--strict-mcp-config".into());
        if miyu_on {
            // 两套同开时去重:与 claude 原生重复的 Miyu 工具剔除,原生优先
            // (用户拍板清单;load_skill/manage_skill 与 claude 的 Skill 内容
            // 不同,不算重复)。
            if let Some(mcp_config) = mcp_bridge_config(runtime, native_on) {
                args.push("--mcp-config".into());
                args.push(mcp_config);
                args.push("--allowedTools".into());
                args.push("mcp__miyu".into());
            }
        }
        if let Some(resume) = resume {
            args.push("--resume".into());
            args.push(resume.to_string());
        }
        if ephemeral {
            args.push("--no-session-persistence".into());
        }
        args
    }
}

/// Miyu 工具经 MCP stdio 桥挂给 claude:`miyu mcp-serve` 打回 daemon,与
/// `miyu tool-call` 同一条会话→模式→registry 解析链。没有会话作用域(测试
/// /直连辅助请求)就不挂桥。
/// 两套工具同开时从桥里剔除的 Miyu 工具(与 claude 原生功能重复,原生
/// 在训练分布内、优先):Bash/Edit/Read/Task/WebSearch/WebFetch 各自的
/// Miyu 对应物,以及 claude 自带后台任务与定时能力覆盖的 job/alarm。
const BRIDGE_DUPLICATE_TOOLS: &[&str] = &[
    "read_file",
    "apply_patch",
    "run_command",
    "task",
    "web_search",
    "web_fetch",
    "glob",
    "grep",
    "todowrite",
    "job",
    "alarm",
];

fn mcp_bridge_config(runtime: &ClaudeCodeRuntime, exclude_duplicates: bool) -> Option<String> {
    let session = crate::tools::workspace::try_session()?;
    let exe = crate::paths::miyu_executable().ok()?;
    let origin =
        serde_json::to_string(&crate::tools::workspace::current_turn_origin()).ok()?;
    let mut env = serde_json::Map::new();
    env.insert("MIYU_SESSION".into(), json!(&*session));
    env.insert("MIYU_TURN_ORIGIN".into(), json!(origin));
    env.insert("MIYU_HOME".into(), json!(runtime.home));
    if exclude_duplicates {
        env.insert(
            "MIYU_MCP_EXCLUDE".into(),
            json!(BRIDGE_DUPLICATE_TOOLS.join(",")),
        );
    }
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        env.insert("XDG_RUNTIME_DIR".into(), json!(xdg.to_string_lossy()));
    }
    Some(
        json!({
            "mcpServers": {
                "miyu": {
                    "command": exe,
                    "args": ["mcp-serve"],
                    "env": env,
                }
            }
        })
        .to_string(),
    )
}

/// 清空 Miyu 会话时的联动:丢弃它名下的续传映射,并尽力删除 claude 侧的
/// 会话转录(`~/.claude/projects/<项目槽>/<会话id>.jsonl`)。存储布局是
/// claude 的内部实现,删不到只记日志不报错——映射已丢弃,该 claude 会话
/// 无论如何不会再被续传。
pub(crate) fn forget_claude_code_session(miyu_session: &str) {
    let removed = session::forget_miyu_session(miyu_session);
    if removed.is_empty() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let projects = std::path::Path::new(&home).join(".claude").join("projects");
    let Ok(project_dirs) = std::fs::read_dir(&projects) else {
        return;
    };
    for project in project_dirs.flatten() {
        for claude_session in &removed {
            let transcript = project.path().join(format!("{claude_session}.jsonl"));
            if !transcript.exists() {
                continue;
            }
            match std::fs::remove_file(&transcript) {
                Ok(()) => tracing::info!(
                    miyu_session,
                    claude_session = %claude_session,
                    "removed the claude-side transcript for a cleared Miyu session"
                ),
                Err(error) => tracing::warn!(
                    %error,
                    path = %transcript.display(),
                    "failed to remove a claude-side transcript (best effort)"
                ),
            }
        }
    }
}
