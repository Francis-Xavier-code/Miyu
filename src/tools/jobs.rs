//! Background command jobs: spawn-and-forget shell processes with status
//! polling, bounded lifetimes, and orphan hygiene across restarts.
//!
//! Jobs live in the current process (daemon or direct REPL). A restart
//! terminates them — the ledger under the runtime dir lets the next
//! instance kill anything a crashed predecessor leaked. Completion invokes
//! an optional host hook (the daemon uses it to wake the model).

use super::{CommandOutputStream, ToolProgress, ToolRegistry, ToolSpec};
use crate::i18n::agent_text as t;
use crate::paths::MiyuPaths;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
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

/// Human-facing Chinese label for a status string; tool outputs keep the
/// raw English label for the model.
pub fn status_display(status: &str) -> String {
    if !crate::i18n::is_zh() {
        return match status {
            "stopped" => "stopped".to_string(),
            "timed_out" => "timed out".to_string(),
            "exited(signal)" => "killed".to_string(),
            "exited(0)" => "done".to_string(),
            other => other
                .strip_prefix("exited(")
                .and_then(|rest| rest.strip_suffix(')'))
                .map(|code| format!("exit {code}"))
                .unwrap_or_else(|| other.to_string()),
        };
    }
    match status {
        "stopped" => "已中断".to_string(),
        "timed_out" => "已超时".to_string(),
        "exited(signal)" => "异常退出".to_string(),
        "exited(0)" => "完成".to_string(),
        other => other
            .strip_prefix("exited(")
            .and_then(|rest| rest.strip_suffix(')'))
            .map(|code| format!("退出码 {code}"))
            .unwrap_or_else(|| other.to_string()),
    }
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

/// What a background job actually is: an OS process group, or an in-process
/// detached subagent future.
#[derive(Clone)]
pub enum JobKind {
    Command { pid: u32 },
    Subagent { abort: tokio::task::AbortHandle },
}

#[derive(Clone)]
struct JobEntry {
    job_id: String,
    title: String,
    command: String,
    workspace: PathBuf,
    session_id: Option<Arc<str>>,
    kind: JobKind,
    started_wall: SystemTime,
    started: Instant,
    finished: Option<Instant>,
    log_path: PathBuf,
    state: JobState,
    /// Set once the host reported the finished job (model wake delivered or
    /// direct-REPL user moved on); acknowledged jobs leave the overview.
    acknowledged: bool,
}

/// Completion details handed to the host hook (daemon: model wake-up).
#[derive(Clone, Debug)]
pub struct JobCompletion {
    pub job_id: String,
    pub title: String,
    /// False when the model itself stopped the command — the host should
    /// clean up UI strips but not wake the model about it.
    pub wake_requested: bool,
    /// True for detached subagents (wording of the wake prompt differs).
    pub is_subagent: bool,
    pub command: String,
    pub workspace: PathBuf,
    pub session_id: Option<Arc<str>>,
    pub state_label: String,
    pub exit_code: Option<i32>,
    pub runtime_seconds: u64,
    pub log_path: PathBuf,
}

pub type CompletionHook = Arc<dyn Fn(JobCompletion) + Send + Sync>;
pub type StartedHook = Arc<dyn Fn(JobOverview) + Send + Sync>;

/// UI-facing snapshot of one job, for status strips and IPC polling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobOverview {
    pub job_id: String,
    pub title: String,
    /// "command" or "subagent" — UIs word their labels by this.
    #[serde(default)]
    pub kind: String,
    /// Owning turn session; UIs only strip-display jobs of their own session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub status: String,
    pub running: bool,
    pub runtime_seconds: u64,
}

#[derive(Serialize, Deserialize)]
struct LedgerEntry {
    owner_pid: u32,
    pid: u32,
    job_id: String,
    started_unix: u64,
}

struct JobHost {
    paths: MiyuPaths,
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

fn started_hook() -> &'static Mutex<Option<StartedHook>> {
    static HOOK: OnceLock<Mutex<Option<StartedHook>>> = OnceLock::new();
    HOOK.get_or_init(|| Mutex::new(None))
}

/// Install the host started hook (daemon: publish job.started to UIs).
pub fn set_started_hook(hook: StartedHook) {
    *started_hook().lock().unwrap() = Some(hook);
}

fn overview_of(job: &JobEntry) -> JobOverview {
    JobOverview {
        job_id: job.job_id.clone(),
        title: job.title.clone(),
        kind: match job.kind {
            JobKind::Command { .. } => "command".to_string(),
            JobKind::Subagent { .. } => "subagent".to_string(),
        },
        session_id: job.session_id.as_deref().map(str::to_string),
        status: job.state.label(),
        running: !job.state.is_terminal(),
        runtime_seconds: job
            .finished
            .unwrap_or_else(Instant::now)
            .duration_since(job.started)
            .as_secs(),
    }
}

/// Jobs the UI status strip should show: running only — finished commands
/// are reported by the wake follow-up, so a terminal chip carries no
/// information.
pub fn overview() -> Vec<JobOverview> {
    let jobs = jobs().lock().unwrap();
    let mut rows = jobs
        .values()
        .filter(|job| !job.state.is_terminal())
        .collect::<Vec<_>>();
    rows.sort_by_key(|job| job.started_wall);
    rows.into_iter().map(overview_of).collect()
}

/// Mark a finished job as reported; it disappears from the overview.
pub fn acknowledge(job_id: &str) {
    if let Some(job) = jobs().lock().unwrap().get_mut(job_id) {
        if job.state.is_terminal() {
            job.acknowledged = true;
        }
    }
}

/// Install the host completion hook (daemon: wake the model). Replaces any
/// previous hook; pass-through for the direct REPL which sets none.
pub fn set_completion_hook(hook: CompletionHook) {
    *completion_hook().lock().unwrap() = Some(hook);
}

/// One-time host init: remembers paths/limits and sweeps ledger entries
/// left behind by dead predecessor processes.
pub fn init(paths: &MiyuPaths, max_runtime_minutes: u64) {
    let _ = host().set(JobHost {
        paths: paths.clone(),
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
    // Short hex id for display friendliness; collision-checked against the
    // live registry, so six chars are plenty for a per-process job list.
    loop {
        let id = format!("{:06x}", rand::random::<u32>() & 0xff_ffff);
        if !jobs().lock().unwrap().contains_key(&id) {
            return id;
        }
    }
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
        .filter_map(|job| job.pid().map(|pid| (job, pid)))
        .map(|(job, pid)| LedgerEntry {
            owner_pid,
            pid,
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
        .map(|job| job.kind.clone())
        .collect::<Vec<_>>();
    let mut pids = Vec::new();
    for kind in &running {
        match kind {
            JobKind::Command { pid } => {
                signal_process_group(*pid, libc::SIGTERM);
                pids.push(*pid);
            }
            JobKind::Subagent { abort } => abort.abort(),
        }
    }
    if !pids.is_empty() {
        std::thread::sleep(Duration::from_millis(300));
        for pid in pids {
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
pub async fn spawn_background(
    command: &str,
    title: Option<&str>,
    progress: &ToolProgress,
) -> Result<String> {
    let host = require_host()?;
    let title = title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(16).collect::<String>())
        .unwrap_or_else(|| {
            let mut fallback = command.chars().take(20).collect::<String>();
            if fallback.len() < command.len() {
                fallback.push('…');
            }
            fallback
        });
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
        title,
        command: command.to_string(),
        workspace: workspace.clone(),
        session_id: super::workspace::try_session(),
        kind: JobKind::Command { pid },
        started_wall: SystemTime::now(),
        started: Instant::now(),
        finished: None,
        log_path: log_path.clone(),
        state: JobState::Running,
        acknowledged: false,
    };
    let started = overview_of(&entry);
    jobs().lock().unwrap().insert(job_id.clone(), entry);
    sync_ledger(&host.paths);
    if let Some(hook) = started_hook().lock().unwrap().clone() {
        hook(started);
    }

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

    // Surface the job id in the tool's visible output stream so the user
    // can see at a glance which job this call started.
    progress.report_command_output(
        CommandOutputStream::Stdout,
        format!(
            "{} {job_id}\n",
            crate::i18n::text("Running in background:", "已后台运行")
        )
        .into_bytes(),
    );

    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "job_id": job_id,
        "pid": pid,
        "log": log_path.display().to_string(),
        "note": t("Background command running. Query with job_status (wait_seconds blocks); never assume its result before it finishes.", "后台命令运行中。用 job_status 查询（可用 wait_seconds 阻塞等待）；命令完成前不要臆测其结果。")
    }))?)
}

/// Detach a subagent as a background job: allocate an id and log file,
/// register the entry, spawn the provided future, and finalize through the
/// same completion hook as background commands (same wake, strip, stop).
/// The builder receives (job_id, log_path) so the future can stream its
/// progress into the log that `job_status` reads.
pub async fn spawn_background_subagent<F>(
    title: Option<&str>,
    description: &str,
    progress: &ToolProgress,
    build: impl FnOnce(String, PathBuf) -> F,
) -> Result<String>
where
    F: std::future::Future<Output = JobState> + Send + 'static,
{
    let host = require_host()?;
    let job_id = next_job_id();
    let dir = logs_dir(&host.paths);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create job log dir {}", dir.display()))?;
    let log_path = dir.join(format!("{job_id}.log"));
    std::fs::write(&log_path, b"")
        .with_context(|| format!("failed to create job log {}", log_path.display()))?;
    let title = title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(16).collect::<String>())
        .unwrap_or_else(|| {
            let mut fallback = description.chars().take(16).collect::<String>();
            if fallback.chars().count() < description.chars().count() {
                fallback.push('…');
            }
            fallback
        });
    let fut = build(job_id.clone(), log_path.clone());
    let reaper_job_id = job_id.clone();
    let handle = tokio::spawn(async move {
        let state = fut.await;
        finalize_job(&reaper_job_id, state, true);
    });
    let entry = JobEntry {
        job_id: job_id.clone(),
        title,
        command: description.to_string(),
        workspace: super::workspace::effective_workdir(),
        session_id: super::workspace::try_session(),
        kind: JobKind::Subagent {
            abort: handle.abort_handle(),
        },
        started_wall: SystemTime::now(),
        started: Instant::now(),
        finished: None,
        log_path: log_path.clone(),
        state: JobState::Running,
        acknowledged: false,
    };
    let started = overview_of(&entry);
    jobs().lock().unwrap().insert(job_id.clone(), entry);
    if let Some(hook) = started_hook().lock().unwrap().clone() {
        hook(started);
    }
    // Subagent detach note rides its own progress channel so it lands as the
    // block's ↳ subject line; CommandOutput is dropped for non-run_command.
    progress.report(format!(
        "__subagent_detach__{} {job_id}",
        crate::i18n::text("Running in background:", "已后台运行")
    ));
    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "kind": "background_subagent",
        "job_id": job_id,
        "log": log_path.display().to_string(),
        "note": t(
            "Subagent detached to the background. Query with job_status (the log holds its progress); never assume its result before it finishes — you will be woken automatically when it completes.",
            "子代理已后台分离运行。用 job_status 查询（日志即其进度）；完成前不要臆测结果，完成后会自动唤起你跟进。"
        )
    }))?)
}

fn finalize_job(job_id: &str, state: JobState, wake_requested: bool) {
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
            title: job.title.clone(),
            wake_requested,
            is_subagent: matches!(job.kind, JobKind::Subagent { .. }),
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
    let hook = completion_hook().lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(completion);
    }
}

impl JobEntry {
    fn pid(&self) -> Option<u32> {
        match &self.kind {
            JobKind::Command { pid } => Some(*pid),
            JobKind::Subagent { .. } => None,
        }
    }
}

fn job_snapshot(job_id: &str) -> Option<JobEntry> {
    jobs().lock().unwrap().get(job_id).cloned()
}

fn read_log_slice(path: &PathBuf, offset: u64, budget: usize) -> (String, u64, u64, bool) {
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

/// Job ids a call is asking about: the `job_ids` array first, then a scalar
/// `job_id`, de-duplicated while keeping the caller's order. Shared by
/// `job_status` and `job_stop` — a plain `dedup()` only drops *adjacent*
/// repeats, so `["a","b","a"]` used to slip two `a`s through.
fn requested_job_ids(args: &Value) -> Vec<String> {
    let mut ids: Vec<String> = args
        .get("job_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(single) = args
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        ids.push(single.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    ids
}

/// Rejects ids owned by another session, mirroring the `all=true` escape
/// hatch both job tools offer.
fn ensure_jobs_visible(ids: &[String], current: Option<&str>, all: bool, verb: &str) -> Result<()> {
    let jobs = jobs().lock().unwrap();
    for id in ids {
        if let Some(job) = jobs.get(id) {
            if !job_visible(job, current, all) {
                bail!("后台任务 {id} 属于其他会话；如确需{verb}请传 all=true");
            }
        }
    }
    Ok(())
}

/// Blocks until any of `ids` reaches a terminal state or the wait elapses.
async fn wait_for_any_finished(ids: &[String], wait: u64) {
    if wait == 0 || ids.is_empty() {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(wait);
    while Instant::now() < deadline {
        let finished = {
            let jobs = jobs().lock().unwrap();
            ids.iter()
                .any(|id| jobs.get(id).map(|job| job.state.is_terminal()).unwrap_or(true))
        };
        if finished {
            return;
        }
        tokio::time::sleep(STATUS_POLL).await;
    }
}

fn job_detail_json(job: &JobEntry, offset: u64, budget: usize) -> Value {
    let (content, next, size, truncated) = read_log_slice(&job.log_path, offset, budget);
    json!({
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
    })
}

/// Session-scoped visibility: a tool call only sees jobs of its own turn
/// session unless it passes `all=true`. Jobs or callers without a session
/// (tests, direct invocations outside a turn) stay globally visible.
fn job_visible(job: &JobEntry, current: Option<&str>, all: bool) -> bool {
    if all {
        return true;
    }
    match (current, job.session_id.as_deref()) {
        (Some(current), Some(session)) => current == session,
        _ => true,
    }
}

async fn job_status(args: Value) -> Result<String> {
    let ids = requested_job_ids(&args);
    let wait = args
        .get("wait_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(MAX_WAIT_SECONDS);
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let current = super::workspace::try_session();

    if ids.len() > 1 {
        // Batch: same per-job shape as the single-id form, wrapped in `jobs`.
        // The log budget is split across the requested ids so asking about
        // five jobs cannot drag back five full logs.
        ensure_jobs_visible(&ids, current.as_deref(), all, "查看")?;
        wait_for_any_finished(&ids, wait).await;
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
        // session's). With wait_seconds, block until ANY watched job that
        // was running at call time finishes (or the wait elapses) — one
        // call can await a whole batch instead of polling each job.
        let running_at_start = || {
            jobs()
                .lock()
                .unwrap()
                .values()
                .filter(|job| !job.state.is_terminal())
                .filter(|job| job_visible(job, current.as_deref(), all))
                .map(|job| job.job_id.clone())
                .collect::<Vec<_>>()
        };
        let watched = running_at_start();
        if wait > 0 && !watched.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(wait);
            while Instant::now() < deadline {
                let finished = {
                    let jobs = jobs().lock().unwrap();
                    watched.iter().any(|id| {
                        jobs.get(id).map(|job| job.state.is_terminal()).unwrap_or(true)
                    })
                };
                if finished {
                    break;
                }
                tokio::time::sleep(STATUS_POLL).await;
            }
        }
        let jobs = jobs().lock().unwrap();
        let mut rows = jobs
            .values()
            .filter(|job| job_visible(job, current.as_deref(), all))
            .collect::<Vec<_>>();
        rows.sort_by_key(|job| job.started_wall);
        let rows = rows
            .into_iter()
            .map(|job| {
                json!({
                    "job_id": job.job_id,
                    "title": job.title,
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

    ensure_jobs_visible(&ids, current.as_deref(), all, "查看")?;
    let deadline = Instant::now() + Duration::from_secs(wait);
    let mut job = job_snapshot(job_id).with_context(|| {
        format!("后台命令 {job_id} 不存在；后台命令随宿主进程重启而清空")
    })?;
    while !job.state.is_terminal() && Instant::now() < deadline {
        tokio::time::sleep(STATUS_POLL).await;
        job = job_snapshot(job_id).context("job disappeared while waiting")?;
    }

    // Single id keeps the flat shape it always had.
    let mut detail = job_detail_json(&job, offset, MAX_STATUS_OUTPUT_CHARS);
    if let Some(map) = detail.as_object_mut() {
        map.insert("ok".to_string(), json!(true));
    }
    Ok(serde_json::to_string_pretty(&detail)?)
}

/// Stop every running job bound to `session_id`; returns how many were
/// stopped. Used when the owning REPL exits — background commands follow
/// their conversation's lifecycle.
pub async fn stop_session_jobs(session_id: &str) -> usize {
    let targets = jobs()
        .lock()
        .unwrap()
        .values()
        .filter(|job| {
            job.state == JobState::Running && job.session_id.as_deref() == Some(session_id)
        })
        .map(|job| job.job_id.clone())
        .collect::<Vec<_>>();
    // Concurrent: serial stops used to add up, and with a stubborn child each
    // one held the caller for the whole grace period.
    let outcomes =
        futures_util::future::join_all(targets.iter().map(|job_id| stop_job(job_id))).await;
    outcomes.into_iter().filter(Result::is_ok).count()
}

/// Host-initiated stop (WebUI strip ✕ button); same semantics as job_stop.
pub async fn stop_job(job_id: &str) -> Result<()> {
    job_stop(json!({ "job_id": job_id })).await.map(|_| ())
}

async fn job_stop(args: Value) -> Result<String> {
    let ids = requested_job_ids(&args);
    if ids.is_empty() {
        bail!("job_id 或 job_ids 至少提供一个；usage: job_stop({{\"job_ids\":[\"abc123\"]}})");
    }
    let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
    let current = super::workspace::try_session();
    ensure_jobs_visible(&ids, current.as_deref(), all, "停止")?;
    if ids.len() > 1 {
        // Concurrent for the same reason as `stop_session_jobs`; per-id errors
        // still stay per-id rather than aborting the batch.
        let outcomes = futures_util::future::join_all(ids.iter().map(|id| stop_one(id))).await;
        let results = ids
            .iter()
            .zip(outcomes)
            .map(|(id, outcome)| match outcome {
                Ok(status) => json!({ "job_id": id, "ok": true, "status": status }),
                Err(error) => json!({ "job_id": id, "ok": false, "error": error.to_string() }),
            })
            .collect::<Vec<_>>();
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "results": results,
        }))?);
    }
    let job_id = &ids[0];
    let job = job_snapshot(job_id)
        .with_context(|| format!("后台任务 {job_id} 不存在"))?;
    if job.state.is_terminal() {
        return Ok(serde_json::to_string_pretty(&json!({
            "ok": true,
            "job_id": job_id,
            "status": job.state.label(),
            "note": t("the background task had already finished", "该后台任务此前已结束"),
        }))?);
    }
    let status = stop_one(job_id).await?;
    Ok(serde_json::to_string_pretty(&json!({
        "ok": true,
        "job_id": job_id,
        "status": status,
    }))?)
}

/// Stop a single job; returns its resulting status label.
async fn stop_one(job_id: &str) -> Result<String> {
    let job = job_snapshot(job_id)
        .with_context(|| format!("后台任务 {job_id} 不存在"))?;
    if job.state.is_terminal() {
        return Ok(job.state.label());
    }
    // Mark terminal first so the reaper's own finalize becomes a no-op;
    // wake_requested=false tells the host to clean up without waking.
    finalize_job(job_id, JobState::Stopped, false);
    acknowledge(job_id);
    match &job.kind {
        JobKind::Command { pid } => {
            let pid = *pid;
            // SIGTERM goes out synchronously so a well-behaved child is
            // already dying when this returns. Only the grace period and the
            // SIGKILL escalation are detached — waiting them out inline is
            // what made Ctrl+C feel frozen, and it bought nothing: the job was
            // marked terminal above and has already left `overview()`.
            signal_process_group(pid, libc::SIGTERM);
            tokio::spawn(async move {
                let deadline = Instant::now() + STOP_GRACE;
                while process_alive(pid) && Instant::now() < deadline {
                    tokio::time::sleep(STATUS_POLL).await;
                }
                if process_alive(pid) {
                    signal_process_group(pid, libc::SIGKILL);
                }
            });
        }
        JobKind::Subagent { abort } => abort.abort(),
    }
    Ok("stopped".to_string())
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
                "Stop this session's background tasks (commands or subagents). Commands get SIGTERM then SIGKILL; subagents are aborted. Accepts job_ids for batch stops. Pass all=true to stop other sessions' jobs.",
                "停止本会话的后台任务（后台命令或后台子代理）。命令向进程组发送 SIGTERM，宽限期后升级 SIGKILL；子代理直接中止。支持 job_ids 批量。停止其他会话的任务需传 all=true。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "要停止的任务 id（单个）。" },
                    "job_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "要停止的任务 id 列表（批量）。"
                    },
                    "all": { "type": "boolean", "description": "true 时允许停止其他会话的后台任务。" }
                },
                "additionalProperties": false
            }),
            |args| async move { job_stop(args).await },
        )
        .writes()
        .with_display_name(t("Stop background task", "停止后台任务")),
    );
}

/// job_status only, for read-only registries (Plan mode).
pub fn register_status(registry: &mut ToolRegistry) {
    registry.register(
        ToolSpec::new(
            "job_status",
            t(
                "Check background jobs of the current session. Without an id lists this session's jobs (wait_seconds then waits for ANY running job to finish — one call can await a batch); with job_id returns status plus incremental log output from offset; with job_ids returns the same per-job detail for several at once, sharing the log budget. wait_seconds is capped at 30. Pass all=true to see other sessions' jobs.",
                "查询本会话的后台任务。不带 id 列出本会话全部任务（此时 wait_seconds 表示等待任意一条运行中任务结束，适合一次等待多条）；带 job_id 返回状态和从 offset 起的增量日志输出；带 job_ids 一次返回多条任务的同样明细（日志额度在这些任务间均分）。wait_seconds 上限 30 秒。跨会话查询需传 all=true。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "单个后台任务 id；与 job_ids 都省略则列出本会话全部任务。" },
                    "job_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "一次查询多个后台任务 id。"
                    },
                    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 30, "description": "任务未结束时最多阻塞等待多少秒再返回；多 id 时等待其中任意一条结束。" },
                    "offset": { "type": "integer", "minimum": 0, "description": "日志读取起始字节偏移；用上次返回的 next_offset 增量读取。" },
                    "all": { "type": "boolean", "description": "true 时不限本会话，查询全部会话的后台任务。" }
                },
                "additionalProperties": false
            }),
            |args| async move { job_status(args).await },
        )
        .with_display_name(t("Check background tasks", "查询后台任务")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_progress() -> ToolProgress {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Box::leak(Box::new(receiver));
        ToolProgress::new(sender)
    }

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
            init(&paths, 120);
        });
    }

    #[tokio::test]
    async fn background_job_lifecycle() {
        shared_init();
        let spawned: Value =
            serde_json::from_str(&spawn_background("echo hello; exit 3", Some("退出码测试"), &test_progress()).await.unwrap()).unwrap();
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

    #[test]
    fn requested_ids_merge_both_forms_and_drop_repeats() {
        assert_eq!(
            requested_job_ids(&json!({"job_ids": [" a ", "b", "a", ""], "job_id": "b"})),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(requested_job_ids(&json!({})).is_empty());
        // Caller order survives — a plain sort would reshuffle the report.
        assert_eq!(
            requested_job_ids(&json!({"job_ids": ["z", "a"]})),
            vec!["z".to_string(), "a".to_string()]
        );
    }

    #[tokio::test]
    async fn stopping_returns_without_waiting_out_the_grace_period() {
        shared_init();
        // `sh -c 'trap "" TERM; sleep 30'` ignores SIGTERM, so the old inline
        // grace wait would hold the caller for the full STOP_GRACE per job.
        let mut ids = Vec::new();
        for _ in 0..2 {
            let spawned: Value = serde_json::from_str(
                &spawn_background("trap '' TERM; sleep 30", Some("顽固任务"), &test_progress())
                    .await
                    .unwrap(),
            )
            .unwrap();
            ids.push(spawned["job_id"].as_str().unwrap().to_string());
        }

        let started = std::time::Instant::now();
        let stopped = stop_session_jobs_for_test(&ids).await;
        let elapsed = started.elapsed();

        assert_eq!(stopped, 2);
        // Two stubborn jobs used to cost 2 × STOP_GRACE; the escalation now
        // runs detached, so the caller is back essentially immediately.
        assert!(
            elapsed < STOP_GRACE,
            "stopping blocked for {elapsed:?}, expected well under {STOP_GRACE:?}"
        );
        for id in &ids {
            assert!(job_snapshot(id).unwrap().state.is_terminal());
        }
    }

    /// `stop_session_jobs` filters by session id, which these tests do not
    /// have; drive the same concurrent path over explicit ids instead.
    async fn stop_session_jobs_for_test(ids: &[String]) -> usize {
        futures_util::future::join_all(ids.iter().map(|id| stop_job(id)))
            .await
            .into_iter()
            .filter(Result::is_ok)
            .count()
    }

    #[tokio::test]
    async fn job_status_reports_several_ids_at_once() {
        shared_init();
        let mut ids = Vec::new();
        for marker in ["alpha", "beta"] {
            let spawned: Value = serde_json::from_str(
                &spawn_background(&format!("echo {marker}"), Some(marker), &test_progress())
                    .await
                    .unwrap(),
            )
            .unwrap();
            ids.push(spawned["job_id"].as_str().unwrap().to_string());
        }

        let status: Value = serde_json::from_str(
            &job_status(json!({"job_ids": ids, "wait_seconds": 10}))
                .await
                .unwrap(),
        )
        .unwrap();
        let rows = status["jobs"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        for (row, marker) in rows.iter().zip(["alpha", "beta"]) {
            assert!(row["output"]["content"].as_str().unwrap().contains(marker));
        }

        // A single id keeps the flat shape callers already parse.
        let single: Value = serde_json::from_str(
            &job_status(json!({"job_id": ids[0], "wait_seconds": 10}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(single["job_id"], ids[0].as_str());
        assert!(single["jobs"].is_null());
        assert!(single["output"]["content"]
            .as_str()
            .unwrap()
            .contains("alpha"));
    }

    #[tokio::test]
    async fn background_subagent_lifecycle() {
        shared_init();
        let spawned: Value = serde_json::from_str(
            &spawn_background_subagent(Some("子代理测试"), "描述文本", &test_progress(), |_job_id, log_path| {
                async move {
                    let _ = std::fs::write(&log_path, "工作中\n");
                    JobState::Exited { code: Some(0) }
                }
            })
            .await
            .unwrap(),
        )
        .unwrap();
        let job_id = spawned["job_id"].as_str().unwrap().to_string();
        assert_eq!(spawned["kind"], "background_subagent");
        let status: Value = serde_json::from_str(
            &job_status(json!({"job_id": job_id, "wait_seconds": 10}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status["status"], "exited(0)");
        assert!(status["output"]["content"]
            .as_str()
            .unwrap()
            .contains("工作中"));
    }

    #[tokio::test]
    async fn job_stop_terminates_a_running_job() {
        shared_init();
        let spawned: Value =
            serde_json::from_str(&spawn_background("sleep 300", None, &test_progress()).await.unwrap()).unwrap();
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
            serde_json::from_str(&spawn_background("printf 'AAABBB'", None, &test_progress()).await.unwrap()).unwrap();
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
