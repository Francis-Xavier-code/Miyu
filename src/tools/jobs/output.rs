//! 任务日志的读取与结论提取。
//!
//! 日志可能很大，所以按偏移增量读（`read_log_slice`）、按行数取尾
//! （`read_log_tail`）。尾部预算随任务数缩小（`tail_lines_for`）——十个任务各
//! 占五十行就把上下文吃光了。
//!
//! `completion_result` 从子代理输出里认出结论标记；失败时给更多行，因为失败的
//! 现场比成功的更需要看。

use crate::tools::jobs::*;

/// Output chunk cap per job_status call, mirroring script output limits.
pub(crate) const MAX_STATUS_OUTPUT_CHARS: usize = 20_000;

/// 列表模式里单行日志的上限,防止一行超长把整条任务的额度吃光。
pub(crate) const MAX_TAIL_LINE_CHARS: usize = 200;

/// 分离子代理把最终结论追加到日志末尾时用的分隔符。写在 `task.rs`,读在这里,
/// 所以常量放在两边都够得着的位置——分头写死过一次就会悄悄对不上。
pub(crate) const SUBAGENT_RESULT_MARKER: &str = "===== 子代理结果 =====";

pub(crate) const SUBAGENT_ERROR_MARKER: &str = "===== 子代理失败 =====";

pub(crate) fn read_log_slice(path: &PathBuf, offset: u64, budget: usize) -> (String, u64, u64, bool) {
    let Ok(bytes) = std::fs::read(path) else {
        return (String::new(), offset, 0, false);
    };
    let size = bytes.len() as u64;
    let start = offset.min(size) as usize;
    let mut end = bytes.len();
    let mut truncated = false;
    if end - start > budget {
        end = start + budget;
        truncated = true;
    }
    let slice = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    (slice, end as u64, size, truncated)
}

/// 日志尾部的最后 `lines` 行,给列表模式用。
///
/// 和 `read_log_slice` 是两种语义:那个从 `offset` 向后增量读,用来追新输出;
/// 这个取的是最新的一段,回答「现在跑成什么样了」。所以列表模式**不返回**
/// next_offset——把尾部片段的末尾当成续读起点会漏掉中间那一大段。
///
/// 单行超过 `MAX_TAIL_LINE_CHARS` 会被截断,免得某行是一整坨 JSON 就把
/// 整条任务的额度吃光。前面还有内容时开头补一个 `…`。
pub(crate) fn read_log_tail(path: &PathBuf, lines: usize) -> (String, u64) {
    let Ok(bytes) = std::fs::read(path) else {
        return (String::new(), 0);
    };
    let size = bytes.len() as u64;
    let text = String::from_utf8_lossy(&bytes);
    let all = text.lines().collect::<Vec<_>>();
    let start = all.len().saturating_sub(lines);
    let mut out = String::new();
    if start > 0 {
        out.push_str("…\n");
    }
    for line in &all[start..] {
        if line.chars().count() > MAX_TAIL_LINE_CHARS {
            let clipped = line
                .chars()
                .take(MAX_TAIL_LINE_CHARS)
                .collect::<String>();
            out.push_str(&clipped);
            out.push('…');
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    (out, size)
}

/// 后台任务完成时,交给模型的「结果」。
///
/// 按任务类型分开取,因为两者的产物根本不同:子代理有明确的最终结论(它就是
/// 这次任务的交付物,**不截断**——截了模型还得回头读日志,等于没给);后台命令
/// 没有结论,日志尾部就是结果,失败时给得比成功时多,因为报错的根因常常要往上
/// 翻十几行。
pub fn completion_result(log_path: &PathBuf, is_subagent: bool, ok: bool) -> Option<(String, String)> {
    if is_subagent {
        let text = std::fs::read_to_string(log_path).ok()?;
        for marker in [SUBAGENT_RESULT_MARKER, SUBAGENT_ERROR_MARKER] {
            if let Some(index) = text.rfind(marker) {
                let body = text[index + marker.len()..].trim();
                if body.is_empty() {
                    continue;
                }
                let label = if marker == SUBAGENT_RESULT_MARKER {
                    "子代理结论"
                } else {
                    "子代理失败"
                };
                return Some((label.to_string(), body.to_string()));
            }
        }
        return None;
    }
    let (tail, _) = read_log_tail(log_path, if ok { 10 } else { 30 });
    let tail = tail.trim_end();
    (!tail.is_empty()).then(|| ("输出结尾".to_string(), tail.to_string()))
}

/// 任务越多每条给得越少,总量始终有界:最坏 20 条 × 3 行 × 200 字符 ≈ 12 K。
pub(crate) fn tail_lines_for(job_count: usize) -> usize {
    match job_count {
        0..=6 => 10,
        7..=15 => 5,
        _ => 3,
    }
}

pub(in crate::tools::jobs) fn job_detail_json(job: &JobEntry, offset: u64, budget: usize) -> Value {
    let (content, next, size, truncated) = read_log_slice(&job.log_path, offset, budget);
    json!({
        "job_id": job.job_id,
        "kind": job.kind_label(),
        "title": job.title,
        "status": job.state.label(),
        "running": !job.state.is_terminal(),
        "command": truncate_command(&job.command),
        // 完整翻阅走 read_file 读这个路径,不在这里重造一套分页。
        "log_path": job.log_path.display().to_string(),
        "runtime_seconds": job.finished.unwrap_or_else(Instant::now)
            .duration_since(job.started).as_secs(),
        "output": {
            "offset": offset,
            "content": content,
            "next_offset": next,
            "log_size": size,
            "truncated": truncated,
        },
    })
}

pub(crate) async fn job_status(args: Value) -> Result<String> {
    let ids = requested_job_ids(&args);
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let current = crate::tools::workspace::try_session();

    if ids.len() > 1 {
        // Batch: same per-job shape as the single-id form, wrapped in `jobs`.
        // The log budget is split across the requested ids so asking about
        // five jobs cannot drag back five full logs.
        ensure_jobs_visible(&ids, current.as_deref(), all, "查看")?;
        let budget = (MAX_STATUS_OUTPUT_CHARS / ids.len()).max(1);
        let mut rows = Vec::with_capacity(ids.len());
        for id in &ids {
            match job_snapshot(id) {
                Some(job) => rows.push(job_detail_json(&job, offset, budget)),
                None => rows.push(json!({
                    "job_id": id,
                    "ok": false,
                    "error": format!("后台命令 {id} 不存在；后台命令随宿主进程重启而清空"),
                })),
            }
        }
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "jobs": rows,
        }))?);
    }

    let Some(job_id) = ids.first().map(String::as_str) else {
        // No job_id: list this session's jobs (all=true lists every
        // session's). 完成会自动唤醒调用方,这里不提供阻塞等待。
        let mut jobs = jobs().lock().unwrap();
        prune_expired_terminal(&mut jobs);
        let mut rows = jobs
            .values()
            .filter(|job| job_visible(job, current.as_deref(), all))
            .collect::<Vec<_>>();
        rows.sort_by_key(|job| job.started_wall);
        // 每条带一段日志尾部:否则模型看完列表还得逐个再查一轮才知道进展,
        // 而子代理的提示语本来就写着「日志即其进度」。
        let tail_lines = tail_lines_for(rows.len());
        let rows = rows
            .into_iter()
            .map(|job| {
                let (recent_output, log_size) = read_log_tail(&job.log_path, tail_lines);
                json!({
                    "job_id": job.job_id,
                    "kind": job.kind_label(),
                    "title": job.title,
                    "status": job.state.label(),
                    "running": !job.state.is_terminal(),
                    "command": truncate_command(&job.command),
                    "runtime_seconds": job.finished.unwrap_or_else(Instant::now)
                        .duration_since(job.started).as_secs(),
                    "recent_output": recent_output,
                    "log_size": log_size,
                    "log_path": job.log_path.display().to_string(),
                    "workspace": job.workspace.display().to_string(),
                })
            })
            .collect::<Vec<_>>();
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "jobs": rows,
        }))?);
    };

    ensure_jobs_visible(&ids, current.as_deref(), all, "查看")?;
    let job = job_snapshot(job_id).with_context(|| {
        format!("后台命令 {job_id} 不存在；后台命令随宿主进程重启而清空")
    })?;

    // Single id keeps the flat shape it always had.
    let mut detail = job_detail_json(&job, offset, MAX_STATUS_OUTPUT_CHARS);
    if let Some(map) = detail.as_object_mut() {
        map.insert("ok".to_string(), json!(true));
    }
    Ok(serde_json::to_string_pretty(&detail)?)
}

pub(crate) fn truncate_command(command: &str) -> String {
    let mut truncated = command.chars().take(200).collect::<String>();
    if truncated.len() < command.len() {
        truncated.push('…');
    }
    truncated
}
