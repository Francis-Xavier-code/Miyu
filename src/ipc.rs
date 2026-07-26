use crate::paths::MiyuPaths;
use crate::question::QuestionAnswers;
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::path::Path;
use std::{
    fs::File, fs::OpenOptions, os::fd::AsRawFd, os::unix::fs::PermissionsExt,
    os::unix::process::CommandExt, path::PathBuf, process::Stdio, time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub const PROTOCOL_VERSION: u16 = 2;
const MAX_FRAME_BYTES: usize = 24 * 1024 * 1024;

/// Unique id of this build, stamped by build.rs. A daemon whose build id
/// differs from the client's is restarted transparently so a rebuild never
/// keeps serving stale code.
pub const BUILD_ID: &str = env!("MIYU_BUILD_ID");

/// Access URLs for the WebUI: loopback plus every local IPv4 address.
/// Shared between the daemon (startup banner) and the CLI (`miyu web` /
/// `--status` output).
pub fn web_access_urls(port: u16) -> Vec<String> {
    let mut addresses = std::collections::BTreeSet::new();
    addresses.insert(std::net::Ipv4Addr::LOCALHOST);
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if let if_addrs::IfAddr::V4(address) = interface.addr {
                if !address.ip.is_unspecified() {
                    addresses.insert(address.ip);
                }
            }
        }
    }
    addresses
        .into_iter()
        .map(|address| format!("http://{address}:{port}"))
        .collect()
}

#[derive(Clone, Debug)]
pub struct DaemonInfo {
    pub pid: u32,
    pub web_port: u16,
    pub web_public: bool,
    pub build_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    pub context_tokens: u64,
    pub context_window: Option<usize>,
    pub cumulative_tokens: u64,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub session_name: String,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Reference to a chat session in IPC commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionRef {
    /// The daemon's current session.
    Current,
    /// A session by exact id.
    Id { id: String },
    /// A user session of the active persona by (case-insensitive) name.
    Name { name: String },
}

pub struct DirectCoreLease {
    lock_file: File,
}

pub struct WebCoreLease {
    lock_file: File,
    socket_path: PathBuf,
}

struct StarterLease {
    lock_file: File,
}

impl Drop for DirectCoreLease {
    fn drop(&mut self) {
        unlock(&self.lock_file);
    }
}

impl Drop for WebCoreLease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        unlock(&self.lock_file);
    }
}

impl Drop for StarterLease {
    fn drop(&mut self) {
        unlock(&self.lock_file);
    }
}

pub fn acquire_direct_core(paths: &MiyuPaths) -> Result<DirectCoreLease> {
    prepare_runtime_dir(paths)?;
    let lock_path = paths
        .runtime_dir()
        .join(format!("direct-core-{}.lock", std::process::id()));
    acquire_direct_core_at(lock_path)
}

pub fn acquire_web_core(paths: &MiyuPaths) -> Result<WebCoreLease> {
    prepare_runtime_dir(paths)?;
    let lock_file = acquire_lock(paths.ipc_lock())?;
    let socket_path = paths.ipc_socket();
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }
    Ok(WebCoreLease {
        lock_file,
        socket_path,
    })
}

fn prepare_runtime_dir(paths: &MiyuPaths) -> Result<()> {
    let runtime_dir = paths.runtime_dir();
    std::fs::create_dir_all(&runtime_dir)?;
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn acquire_direct_core_at(lock_path: PathBuf) -> Result<DirectCoreLease> {
    Ok(DirectCoreLease {
        lock_file: acquire_lock(lock_path)?,
    })
}

fn acquire_lock(lock_path: PathBuf) -> Result<File> {
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    let result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        bail!("Miyu Web core is starting or temporarily unavailable");
    }
    Ok(lock_file)
}

fn unlock(lock_file: &File) {
    unsafe {
        libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub version: u16,
    #[serde(flatten)]
    pub command: Command,
}

impl Request {
    pub fn new(command: Command) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            command,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Ping,
    Shutdown,
    ReloadConfig,
    GetStatus,
    ResetConversation {
        all: bool,
    },
    Undo,
    Pop {
        turn_ids: Vec<String>,
    },
    Compact,
    StartTurn {
        content: String,
        mode: String,
        #[serde(default)]
        images: Vec<Option<ImageAttachment>>,
        /// Client working directory; the core adopts it for the duration of
        /// the turn so file tools resolve relative to the caller. Interim
        /// bridge until sessions carry a proper workspace context.
        #[serde(default)]
        cwd: Option<std::path::PathBuf>,
    },
    Cancel {
        run_id: String,
    },
    AnswerQuestion {
        question_id: String,
        answers: QuestionAnswers,
    },
    ListSessions {
        #[serde(default)]
        include_archived: bool,
    },
    CreateSession {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        switch: bool,
    },
    SwitchSession {
        target: SessionRef,
    },
    RenameSession {
        target: SessionRef,
        name: String,
    },
    ArchiveSession {
        target: SessionRef,
        archived: bool,
    },
    DeleteSession {
        target: SessionRef,
    },
    SetWorkspace {
        target: SessionRef,
        #[serde(default)]
        path: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageAttachment {
    Binary { mime: String, data: Vec<u8> },
    Path { path: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Ready {
        pid: u32,
        #[serde(default)]
        web_port: u16,
        #[serde(default)]
        web_public: bool,
        #[serde(default)]
        build_id: String,
    },
    Accepted {
        run_id: String,
    },
    Event {
        id: u64,
        kind: String,
        data: Value,
    },
    Ack,
    AdminResult {
        state: SessionState,
        data: Value,
    },
    Error {
        message: String,
    },
}

pub async fn connect(path: &Path) -> Result<UnixStream> {
    UnixStream::connect(path)
        .await
        .with_context(|| format!("connecting to Miyu core at {}", path.display()))
}

pub async fn daemon_info(paths: &MiyuPaths) -> Option<DaemonInfo> {
    let mut stream = tokio::time::timeout(Duration::from_millis(250), connect(&paths.ipc_socket()))
        .await
        .ok()?
        .ok()?;
    send(&mut stream, &Request::new(Command::Ping)).await.ok()?;
    match tokio::time::timeout(Duration::from_millis(250), receive::<Frame>(&mut stream))
        .await
        .ok()?
        .ok()??
    {
        Frame::Ready {
            pid,
            web_port,
            web_public,
            build_id,
        } => Some(DaemonInfo {
            pid,
            web_port,
            web_public,
            build_id,
        }),
        _ => None,
    }
}

pub async fn ensure_daemon(paths: &MiyuPaths, args: &[OsString]) -> Result<DaemonInfo> {
    if let Some(info) = daemon_info(paths).await {
        if info.build_id == BUILD_ID {
            return Ok(info);
        }
        restart_stale_daemon(paths).await?;
    }
    let _starter = acquire_starter(paths)?;
    if let Some(info) = daemon_info(paths).await {
        if info.build_id == BUILD_ID {
            return Ok(info);
        }
        restart_stale_daemon(paths).await?;
    }
    start_daemon_process(paths, args)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(info) = daemon_info(paths).await {
            return Ok(info);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Miyu daemon did not become ready within 8 seconds");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Shuts down a daemon left over from an older build so the caller can spawn
/// one matching the current binary.
async fn restart_stale_daemon(paths: &MiyuPaths) -> Result<()> {
    let mut stream = connect(&paths.ipc_socket()).await?;
    send(&mut stream, &Request::new(Command::Shutdown)).await?;
    let _ = receive::<Frame>(&mut stream).await;
    wait_for_daemon_exit(paths, Duration::from_secs(5))
        .await
        .context("waiting for the outdated Miyu daemon to stop")
}

pub async fn wait_for_daemon_exit(paths: &MiyuPaths, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if daemon_info(paths).await.is_none() {
            if let Ok(lease) = acquire_direct_core(paths) {
                drop(lease);
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "Miyu daemon did not stop within {} seconds",
                timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn acquire_starter(paths: &MiyuPaths) -> Result<StarterLease> {
    prepare_runtime_dir(paths)?;
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.daemon_start_lock())?;
    let result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(StarterLease { lock_file })
}

fn start_daemon_process(paths: &MiyuPaths, args: &[OsString]) -> Result<()> {
    std::fs::create_dir_all(paths.logs_dir())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.logs_dir().join("daemon.log"))?;
    // The daemon is this very binary re-executed with a hidden subcommand,
    // so a single installed file is always sufficient.
    let executable =
        std::env::current_exe().context("resolving the Miyu executable to spawn the daemon")?;
    let mut command = std::process::Command::new(executable);
    command.arg("__daemon");
    if args.is_empty() {
        // Auto-spawned daemon: let the OS pick a free WebUI port.
        command.args(["--port", "0"]);
    }
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn().context("starting Miyu daemon")?;
    Ok(())
}

pub async fn send<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME_BYTES {
        bail!("IPC frame exceeds the 24 MiB limit");
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

pub async fn receive<T: DeserializeOwned>(stream: &mut UnixStream) -> Result<Option<T>> {
    let length = match stream.read_u32().await {
        Ok(length) => length as usize,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("invalid IPC frame length: {length}");
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_protocol_is_explicitly_versioned() {
        let value = serde_json::to_value(Request::new(Command::Ping)).unwrap();
        assert_eq!(value["version"], PROTOCOL_VERSION);
        assert_eq!(value["command"], "ping");
    }

    #[test]
    fn ready_frame_exposes_daemon_web_state() {
        let value = serde_json::to_value(Frame::Ready {
            pid: 42,
            web_port: 4096,
            web_public: false,
            build_id: "test-build".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "ready");
        assert_eq!(value["pid"], 42);
        assert_eq!(value["web_port"], 4096);
        assert_eq!(value["web_public"].as_bool(), Some(false));
    }

    #[test]
    fn admin_commands_round_trip_with_explicit_state() {
        let request = Request::new(Command::ResetConversation { all: true });
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["command"], "reset_conversation");
        assert_eq!(value["all"], true);

        let frame = Frame::AdminResult {
            state: SessionState {
                context_tokens: 12,
                context_window: Some(1000),
                cumulative_tokens: 34,
                session_id: "default".to_string(),
                session_name: "默认会话".to_string(),
                workspace: None,
            },
            data: serde_json::json!({"ok": true}),
        };
        let value = serde_json::to_value(frame).unwrap();
        assert_eq!(value["type"], "admin_result");
        assert_eq!(value["state"]["cumulative_tokens"], 34);
    }

    #[tokio::test]
    async fn framed_protocol_round_trips_over_unix_socket() {
        let (mut left, mut right) = UnixStream::pair().unwrap();
        let request = Request::new(Command::StartTurn {
            content: "hello".to_string(),
            mode: "normal".to_string(),
            images: vec![Some(ImageAttachment::Binary {
                mime: "image/png".to_string(),
                data: vec![1, 2, 3],
            })],
            cwd: Some(std::path::PathBuf::from("/tmp/workdir")),
        });
        let writer = tokio::spawn(async move { send(&mut left, &request).await });
        let received = receive::<Request>(&mut right).await.unwrap().unwrap();
        writer.await.unwrap().unwrap();

        assert_eq!(received.version, PROTOCOL_VERSION);
        match received.command {
            Command::StartTurn {
                content,
                mode,
                images,
                cwd,
            } => {
                assert_eq!(content, "hello");
                assert_eq!(mode, "normal");
                assert_eq!(images.len(), 1);
                assert_eq!(cwd, Some(std::path::PathBuf::from("/tmp/workdir")));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_writing() {
        let (mut left, _right) = UnixStream::pair().unwrap();
        let request = Request::new(Command::StartTurn {
            content: "x".repeat(MAX_FRAME_BYTES),
            mode: "normal".to_string(),
            images: Vec::new(),
            cwd: None,
        });
        assert!(send(&mut left, &request).await.is_err());
    }

    #[test]
    fn direct_core_lease_is_exclusive() {
        let temp = tempfile::tempdir().unwrap();
        let lock = temp.path().join("core.lock");
        let first = acquire_direct_core_at(lock.clone()).unwrap();
        assert!(acquire_direct_core_at(lock.clone()).is_err());
        drop(first);
        assert!(acquire_direct_core_at(lock).is_ok());
    }
}
