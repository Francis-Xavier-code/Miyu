//! Background command jobs: spawn-and-forget shell processes with status
//! polling, bounded lifetimes, and orphan hygiene across restarts.
//!
//! Jobs live in the current process (daemon or direct REPL). A restart
//! terminates them — the ledger under the runtime dir lets the next
//! instance kill anything a crashed predecessor leaked. Completion invokes
//! an optional host hook (the daemon uses it to wake the model).

use super::{ToolRegistry, ToolSpec};
use crate::i18n::agent_text as t;
use crate::paths::MiyuPaths;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

const STOP_GRACE: Duration = Duration::from_secs(5);
const STATUS_POLL: Duration = Duration::from_millis(250);
/// job_status wait_seconds is clamped to this bound.
pub const MAX_WAIT_SECONDS: u64 = 30;
/// Output chunk cap per job_status call, mirroring script output limits.
const MAX_STATUS_OUTPUT_CHARS: usize = 20_000;
const LOG_RETENTION_DAYS: u64 = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    Running,
    Exited { code: Option<i32> },
    TimedOut,
    Stopped,
}

impl JobState {
    fn label(&self) -> String {
        match self {
            JobState::Running => "running".to_string(),
            JobState::Exited { code: Some(code) } => format!("exited({code})"),
            JobState::Exited { code: None } => "exited(signal)".to_string(),
            JobState::TimedOut => "timed_out".to_string(),
            JobState::Stopped => "stopped".to_string(),
        }
    }

    fn is_terminal(&self) -> bool {
        !matches!(self, JobState::Running)
    }
}

#[derive(Clone)]
struct JobEntry {
    job_id: String,
    command: String,
    workspace: PathBuf,
    session_id: Option<Arc<str>>,
    pid: u32,
    started_wall: SystemTime,
    started: Instant,
    finished: Option<Instant>,
    log_path: PathBuf,
    state: JobState,
}

/// Completion details handed to the host hook (daemon: model wake-up).
#[derive(Clone, Debug)]
pub struct JobCompletion {
    pub job_id: String,
    pub command: String,
    pub workspace: PathBuf,
    pub session_id: Option<Arc<str>>,
    pub state_label: String,
    pub exit_code: Option<i32>,
    pub runtime_seconds: u64,
    pub log_path: PathBuf,
}

pub type CompletionHook = Arc<dyn Fn(JobCompletion) + Send + Sync>;

#[derive(Serialize, Deserialize)]
struct LedgerEntry {
    owner_pid: u32,
    pid: u32,
    job_id: String,
    started_unix: u64,
}

struct JobHost {
    paths: MiyuPaths,
    limit: usize,
    max_runtime: Duration,
}

fn jobs() -> &'static Mutex<HashMap<String, JobEntry>> {
    static JOBS: OnceLock<Mutex<HashMap<String, JobEntry>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn host() -> &'static OnceLock<JobHost> {
    static HOST: OnceLock<JobHost> = OnceLock::new();
    &HOST
}

fn completion_hook() -> &'static Mutex<Option<CompletionHook>> {
    static HOOK: OnceLock<Mutex<Option<CompletionHook>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

/// Install the host completion hook (daemon: wake the model). Replaces any
/// previous hook; pass-through for the direct REPL which sets none.
pub fn set_completion_hook(hook: CompletionHook) {
    *completion_hook().lock().unwrap() = Some(hook);
}

/// One-time host init: remembers paths/limits and sweeps ledger entries
/// left behind by dead predecessor processes.
pub fn init(paths: &MiyuPaths, limit: usize, max_runtime_minutes: u64) {
    let _ = host().set(JobHost {
        paths: paths.clone(),
        limit: limit.max(1),
        max_runtime: Duration::from_secs(max_runtime_minutes.max(1) * 60),
    });
    sweep_stale_jobs(paths);
    cleanup_old_logs(paths);
}

fn require_host() -> Result<&'static JobHost> {
    host()
        .get()
        .context("background jobs are not initialized in this process")
}

fn logs_dir(paths: &MiyuPaths) -> PathBuf {
    paths.cache_dir.join("jobs")
}

fn ledger_path(paths: &MiyuPaths) -> PathBuf {
    paths.runtime_dir().join("background-jobs.json")
}

fn next_job_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!(
        "job_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn signal_process_group(pid: u32, signal: i32) {
    unsafe {
        libc::killpg(pid as i32, signal);
    }
}

fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Kill process groups recorded by predecessors that are no longer alive.
/// Entries owned by other live Miyu processes are left untouched.
pub fn sweep_stale_jobs(paths: &MiyuPaths) {
    let path = ledger_path(paths);
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(entries) = serde_json::from_slice::<Vec<LedgerEntry>>(&bytes) else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let mut kept = Vec::new();
    for entry in entries {
        if entry.owner_pid == std::process::id() {
            continue;
        }
        if process_alive(entry.owner_pid) {
            kept.push(entry);
            continue;
        }
        if process_alive(entry.pid) {
            tracing::info!(
                job_id = %entry.job_id,
                pid = entry.pid,
                "{}",
                crate::i18n::text(
                    "killing a background job leaked by a dead Miyu process",
                    "清理已死亡 Miyu 进程遗留的后台任务"
                )
            );
            signal_process_group(entry.pid, libc::SIGKILL);
        }
    }
    let _ = write_ledger(paths, &kept);
}

fn cleanup_old_logs(paths: &MiyuPaths) {
    let dir = logs_dir(paths);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let cutoff = SystemTime::now() - Duration::from_secs(LOG_RETENTION_DAYS * 24 * 3600);
    for entry in entries.flatten() {
        let keep = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|modified| modified >= cutoff)
            .unwrap_or(true);
        if !keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn write_ledger(paths: &MiyuPaths, entries: &[LedgerEntry]) -> Result<()> {
    let path = ledger_path(paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec(entries)?)?;
    Ok(())
}

fn sync_ledger(paths: &MiyuPaths) {
    let owner_pid = std::process::id();
    let entries = jobs()
        .lock()
        .unwrap()
        .values()
        .filter(|job| job.state == JobState::Running)
        .map(|job| LedgerEntry {
            owner_pid,
            pid: job.pid,
            job_id: job.job_id.clone(),
            started_unix: job
                .started_wall
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
        })
        .collect::<Vec<_>>();
    // Preserve entries owned by other live processes sharing this home.
    let mut merged = entries;
    if let Ok(bytes) = std::fs::read(ledger_path(paths)) {
        if let Ok(existing) = serde_json::from_slice::<Vec<LedgerEntry>>(&bytes) {
            merged.extend(
                existing
                    .into_iter()
                    .filter(|entry| entry.owner_pid != owner_pid),
            );
        }
    }
    if let Err(error) = write_ledger(paths, &merged) {
        tracing::debug!(error = %error, "failed to persist the background job ledger");
    }
}

/// Terminate every job owned by this process; called on daemon shutdown
/// and direct-REPL exit so setsid'd children never outlive their host.
pub fn shutdown_all() {
    let running = jobs()
        .lock()
        .unwrap()
        .values()
        .filter(|job| job.state == JobState::Running)
        .map(|job| job.pid)
        .collect::<Vec<_>>();
    for pid in &running {
        signal_process_group(*pid, libc::SIGTERM);
    }
    if !running.is_empty() {
        std::thread::sleep(Duration::from_millis(300));
        for pid in running {
            if process_alive(pid) {
                signal_process_group(pid, libc::SIGKILL);
            }
        }
    }
    if let Some(host) = host().get() {
        sync_ledger(&host.paths);
    }
}

/// Spawn `command` detached in its own process group; stdout+stderr stream
/// into a log file. Returns the tool JSON for run_command.
pub async fn spawn_background(command: &str) -> Result<String> {
    let host = require_host()?;
    let running = jobs()
        .lock()
        .unwrap()
        .values()
        .filter(|job| job.state == JobState::Running)
        .count();
    if running >= host.limit {
        bail!(
            "已有 {running} 个后台任务在运行（上限 {}）；先用 job_status 检查并用 job_stop 结束不需要的任务",
            host.limit
        );
    }
    let job_id = next_job_id();
    let dir = logs_dir(&host.paths);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create job log dir {}", dir.display()))?;
    let log_path = dir.join(format!("{job_id}.log"));
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("failed to create job log {}", log_path.display()))?;
    let workspace = super::workspace::effective_workdir();
    let mut process = Command::new("sh");
    process
        .arg("-lc")
        .arg(command)
        .current_dir(&workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log.try_clone()?))
        .stderr(std::process::Stdio::from(log));
    process.process_group(0);
    let mut child = process.spawn().context("failed to spawn the background job")?;
    let pid = child.id().context("background job has no pid")?;
    let entry = JobEntry {
        job_id: job_id.clone(),
        command: command.to_string(),
        workspace: workspace.clone(),
        session_id: super::workspace::try_session(),
        pid,
        started_wall: SystemTime::now(),
        started: Instant::now(),
        finished: None,
        log_path: log_path.clone(),
        state: JobState::Running,
    };
    jobs().lock().unwrap().insert(job_id.clone(), entry);
    sync_ledger(&host.paths);

    let reaper_job_id = job_id.clone();
    let max_runtime = host.max_runtime;
    tokio::spawn(async move {
        let state = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => match status.code() {
                    Some(code) => JobState::Exited { code: Some(code) },
                    None => JobState::Exited { code: None },
                },
                Err(_) => JobState::Exited { code: None },
            },
            _ = tokio::time::sleep(max_runtime) => {
                signal_process_group(pid, libc::SIGTERM);
                let _ = tokio::time::timeout(STOP_GRACE, child.wait()).await;
                if process_alive(pid) {
                    signal_process_group(pid, libc::SIGKILL);
                    let _ = child.wait().await;
                }
                JobState::TimedOut
            }
        };
        finalize_job(&reaper_job_id, state, true);
    });

    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "job_id": job_id,
        "pid": pid,
        "log": log_path.display().to_string(),
        "note": "后台运行中。用 job_status 轮询（可用 wait_seconds 阻塞等待）；任务完成前不要臆测其结果。"
    }))?)
}

fn finalize_job(job_id: &str, state: JobState, fire_hook: bool) {
    let completion = {
        let mut jobs = jobs().lock().unwrap();
        let Some(job) = jobs.get_mut(job_id) else {
            return;
        };
        if job.state.is_terminal() {
            return;
        }
        job.state = state.clone();
        job.finished = Some(Instant::now());
        JobCompletion {
            job_id: job.job_id.clone(),
            command: job.command.clone(),
            workspace: job.workspace.clone(),
            session_id: job.session_id.clone(),
            state_label: state.label(),
            exit_code: match state {
                JobState::Exited { code } => code,
                _ => None,
            },
            runtime_seconds: job.started.elapsed().as_secs(),
            log_path: job.log_path.clone(),
        }
    };
    if let Some(host) = host().get() {
        sync_ledger(&host.paths);
    }
    if fire_hook {
        let hook = completion_hook().lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(completion);
        }
    } else {
        let _ = completion;
    }
}

fn job_snapshot(job_id: &str) -> Option<JobEntry> {
    jobs().lock().unwrap().get(job_id).cloned()
}

fn read_log_slice(path: &PathBuf, offset: u64) -> (String, u64, u64, bool) {
    let Ok(bytes) = std::fs::read(path) else {
        return (String::new(), offset, 0, false);
    };
    let size = bytes.len() as u64;
    let start = offset.min(size) as usize;
    let mut end = bytes.len();
    let mut truncated = false;
    if end - start > MAX_STATUS_OUTPUT_CHARS {
        end = start + MAX_STATUS_OUTPUT_CHARS;
        truncated = true;
    }
    let slice = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    (slice, end as u64, size, truncated)
}

async fn job_status(args: Value) -> Result<String> {
    let job_id = args
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(job_id) = job_id else {
        let jobs = jobs().lock().unwrap();
        let mut rows = jobs.values().collect::<Vec<_>>();
        rows.sort_by_key(|job| job.started_wall);
        let rows = rows
            .into_iter()
            .map(|job| {
                json!({
                    "job_id": job.job_id,
                    "status": job.state.label(),
                    "command": truncate_command(&job.command),
                    "runtime_seconds": job.finished.unwrap_or_else(Instant::now)
                        .duration_since(job.started).as_secs(),
                    "workspace": job.workspace.display().to_string(),
                })
            })
            .collect::<Vec<_>>();
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "jobs": rows,
        }))?);
    };

    let wait = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(MAX_WAIT_SECONDS);
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);

    let deadline = Instant::now() + Duration::from_secs(wait);
    let mut job = job_snapshot(job_id).with_context(|| {
        format!("job {job_id} not found; jobs are process-local and cleared on restart")
    })?;
    while !job.state.is_terminal() && Instant::now() < deadline {
        tokio::time::sleep(STATUS_POLL).await;
        job = job_snapshot(job_id).context("job disappeared while waiting")?;
    }

    let (content, next, size, truncated) = read_log_slice(&job.log_path, offset);
    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "job_id": job.job_id,
        "status": job.state.label(),
        "running": !job.state.is_terminal(),
        "command": truncate_command(&job.command),
        "runtime_seconds": job.finished.unwrap_or_else(Instant::now)
            .duration_since(job.started).as_secs(),
        "output": {
            "offset": offset,
            "content": content,
            "next_offset": next,
            "log_size": size,
            "truncated": truncated,
        },
    }))?)
}

async fn job_stop(args: Value) -> Result<String> {
    let job_id = args
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("job_id is required; usage: job_stop({\"job_id\":\"job_...\"})")?;
    let job = job_snapshot(job_id)
        .with_context(|| format!("job {job_id} not found"))?;
    if job.state.is_terminal() {
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "job_id": job_id,
            "status": job.state.label(),
            "note": "任务此前已结束",
        }))?);
    }
    // Mark terminal first so the reaper's own finalize becomes a no-op and
    // the completion hook never fires for a stop the model itself requested.
    finalize_job(job_id, JobState::Stopped, false);
    signal_process_group(job.pid, libc::SIGTERM);
    let deadline = Instant::now() + STOP_GRACE;
    while process_alive(job.pid) && Instant::now() < deadline {
        tokio::time::sleep(STATUS_POLL).await;
    }
    if process_alive(job.pid) {
        signal_process_group(job.pid, libc::SIGKILL);
    }
    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "job_id": job_id,
        "status": "stopped",
    }))?)
}

fn truncate_command(command: &str) -> String {
    let mut truncated = command.chars().take(200).collect::<String>();
    if truncated.len() < command.len() {
        truncated.push('…');
    }
    truncated
}

/// job_status + job_stop, for registries that can run commands.
pub fn register_management(registry: &mut ToolRegistry) {
    register_status(registry);
    registry.register(
        ToolSpec::new(
            "job_stop",
            t(
                "Stop a background job started by run_command with background=true. Sends SIGTERM to its process group, escalating to SIGKILL after a grace period.",
                "停止 run_command background=true 启动的后台任务。向其进程组发送 SIGTERM，宽限期后升级为 SIGKILL。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "要停止的后台任务 id。" }
                },
                "required": ["job_id"],
                "additionalProperties": false
            }),
            |args| async move { job_stop(args).await },
        )
        .writes()
        .with_display_name(t("Stop background job", "停止后台任务")),
    );
}

/// job_status only, for read-only registries (Plan mode).
pub fn register_status(registry: &mut ToolRegistry) {
    registry.register(
        ToolSpec::new(
            "job_status",
            t(
                "Check background jobs started by run_command with background=true. Without job_id lists all jobs; with job_id returns status plus incremental log output from offset. wait_seconds (max 30) blocks until the job finishes or the wait elapses.",
                "查询 run_command background=true 启动的后台任务。不带 job_id 列出全部任务；带 job_id 返回状态和从 offset 起的增量日志输出。wait_seconds（上限 30）会阻塞等待任务结束或超时。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "任务 id；省略则列出全部后台任务。" },
                    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 30, "description": "任务未结束时最多阻塞等待多少秒再返回。" },
                    "offset": { "type": "integer", "minimum": 0, "description": "日志读取起始字节偏移；用上次返回的 next_offset 增量读取。" }
                },
                "additionalProperties": false
            }),
            |args| async move { job_status(args).await },
        )
        .with_display_name(t("Check background jobs", "查询后台任务")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `init` is process-global (OnceLock), so every test shares one leaked
    /// home; individual tests must tolerate jobs from their siblings.
    fn shared_init() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            let temp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
            let root = temp.path().to_path_buf();
            let paths = MiyuPaths {
                config_dir: root.join("config"),
                config_file: root.join("config/config.jsonc"),
                skills_dir: root.join("config/skills"),
                data_dir: root.join("data"),
                cache_dir: root.join("cache"),
                state_dir: root.join("state"),
                ..crate::paths::MiyuPaths::new().unwrap()
            };
            init(&paths, 8, 120);
        });
    }

    #[tokio::test]
    async fn background_job_lifecycle() {
        shared_init();
        let spawned: Value =
            serde_json::from_str(&spawn_background("echo hello; exit 3").await.unwrap()).unwrap();
        let job_id = spawned["job_id"].as_str().unwrap().to_string();
        assert!(spawned["ok"].as_bool().unwrap());

        let status: Value = serde_json::from_str(
            &job_status(json!({"job_id": job_id, "wait_seconds": 10})).await.unwrap(),
        )
        .unwrap();
        assert_eq!(status["status"], "exited(3)");
        assert!(status["output"]["content"]
            .as_str()
            .unwrap()
            .contains("hello"));
    }

    #[tokio::test]
    async fn job_stop_terminates_a_running_job() {
        shared_init();
        let spawned: Value =
            serde_json::from_str(&spawn_background("sleep 300").await.unwrap()).unwrap();
        let job_id = spawned["job_id"].as_str().unwrap().to_string();
        let stopped: Value =
            serde_json::from_str(&job_stop(json!({"job_id": job_id})).await.unwrap()).unwrap();
        assert_eq!(stopped["status"], "stopped");
        let status: Value =
            serde_json::from_str(&job_status(json!({"job_id": job_id})).await.unwrap()).unwrap();
        assert_eq!(status["status"], "stopped");
    }

    #[tokio::test]
    async fn incremental_output_reads_from_offset() {
        shared_init();
        let spawned: Value =
            serde_json::from_str(&spawn_background("printf 'AAABBB'").await.unwrap()).unwrap();
        let job_id = spawned["job_id"].as_str().unwrap().to_string();
        let first: Value = serde_json::from_str(
            &job_status(json!({"job_id": job_id, "wait_seconds": 10})).await.unwrap(),
        )
        .unwrap();
        assert_eq!(first["output"]["content"], "AAABBB");
        let second: Value = serde_json::from_str(
            &job_status(json!({"job_id": job_id, "offset": 3})).await.unwrap(),
        )
        .unwrap();
        assert_eq!(second["output"]["content"], "BBB");
    }
}
