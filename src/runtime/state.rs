//! 进程级会话与平台运行时状态。
//!
//! `DaemonState` 是整个 daemon 的根：配置管理器、状态库、事件总线、提问
//! 代理、actor 通道、平台运行时都挂在它上面。Web、IPC、平台适配三条路都
//! 从这里取。
// 兄弟模块的类型互相引用（DaemonState 持有 EventHub、run 记录引用
// ManagerState 等），统一从 mod.rs 的再导出取，免得每个文件维护一份
// 交叉导入清单。
use super::*;
use crate::agent::AgentMode;
use crate::config::AppConfig;
use crate::llm::OpenAiCompatibleClient;
use crate::paths::MiyuPaths;
use crate::platforms::PlatformRuntime;
use crate::tools::build_tool_registry;
use crate::state::StateStore;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
// 只有 `for_test` 用得到，不加 cfg 的话 lib 构建会报未使用
#[cfg(test)]
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};

// ── DaemonState / TurnEngineState / TurnResources 与其缓存 ──
#[derive(Clone)]
pub(crate) struct DaemonState {
    pub(crate) auth: WebAuth,
    pub(crate) boot_id: Arc<str>,
    pub(crate) web_port: u16,
    pub(crate) web_public: bool,
    pub(crate) web_bind: IpAddr,
    pub(crate) paths: MiyuPaths,
    pub(crate) manager: Arc<Mutex<ManagerState>>,
    pub(crate) state_store: StateStore,
    pub(crate) events: EventHub,
    pub(crate) questions: QuestionBroker,
    pub(crate) actor_tx: mpsc::UnboundedSender<ActorCommand>,
    pub(crate) shutdown_tx: broadcast::Sender<()>,
    pub(crate) turn_engine: TurnEngineState,
    pub(crate) platforms: PlatformRuntime,
}

#[cfg(test)]
impl DaemonState {
    pub(crate) fn for_test(paths: MiyuPaths, web_port: u16) -> Result<Self> {
        let state_store = StateStore::new(&paths)?;
        let config = AppConfig::default();
        let context = cold_context(&config, &state_store)?;
        let manager = Arc::new(Mutex::new(ManagerState {
            config,
            active_runs: HashMap::new(),
            admin_busy: false,
            context,
            persona_session_ids: HashMap::new(),
        }));
        let (actor_tx, _actor_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, _shutdown_rx) = broadcast::channel(1);
        Ok(Self {
            auth: WebAuth::new(None),
            boot_id: Arc::from("boot-test"),
            web_port,
            web_public: false,
            web_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            paths,
            manager,
            state_store,
            events: EventHub::new(),
            questions: QuestionBroker::new(),
            actor_tx,
            shutdown_tx,
            turn_engine: TurnEngineState::default(),
            platforms: PlatformRuntime::new()?,
        })
    }

}

#[derive(Clone, Default)]
pub(crate) struct TurnEngineState(Arc<AtomicU8>);

impl TurnEngineState {
    pub(crate) const COLD: u8 = 0;
    pub(crate) const INITIALIZING: u8 = 1;
    pub(crate) const READY: u8 = 2;
    pub(crate) const FAILED: u8 = 3;

    pub(crate) fn set(&self, state: u8) {
        self.0.store(state, Ordering::Release);
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::READY
    }

    pub(crate) fn label(&self) -> &'static str {
        match self.0.load(Ordering::Acquire) {
            Self::INITIALIZING => "initializing",
            Self::READY => "ready",
            Self::FAILED => "failed",
            _ => "cold",
        }
    }
}

/// Expensive per-turn dependencies are initialized on first use and shared
/// by subsequent turns. The cache is keyed by the effective configuration so
/// a QQ conversation-specific model pool gets its own client/tool snapshot.
/// Configuration reloads clear the cache before the next request.
pub(crate) struct TurnResources {
    pub(crate) client: OpenAiCompatibleClient,
    pub(crate) normal_tools: crate::tools::ToolRegistry,
    pub(crate) dev_tools: crate::tools::ToolRegistry,
    pub(crate) restricted_tools: crate::tools::ToolRegistry,
}

pub(crate) const MAX_CACHED_TURN_RESOURCE_CONFIGS: usize = 16;

pub(crate) struct TurnResourceCache {
    pub(crate) entries: HashMap<[u8; 32], Arc<TurnResources>>,
    pub(crate) order: VecDeque<[u8; 32]>,
}

impl Default for TurnResourceCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl TurnResourceCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub(crate) fn key(config: &AppConfig) -> Result<[u8; 32]> {
        let encoded =
            serde_json::to_vec(config).context("serializing effective turn configuration")?;
        Ok(*blake3::hash(&encoded).as_bytes())
    }

    pub(crate) fn get_or_build(
        &mut self,
        config: &AppConfig,
        paths: &MiyuPaths,
    ) -> Result<Arc<TurnResources>> {
        let key = Self::key(config)?;
        if let Some(resources) = self.entries.get(&key).cloned() {
            self.order.retain(|entry| *entry != key);
            self.order.push_back(key);
            return Ok(resources);
        }

        crate::models_cache::ensure_active_metadata(paths, config);
        let restricted_tools = if config.tools.enabled {
            crate::tools::restricted_platform_registry(config, paths)
        } else {
            crate::tools::ToolRegistry::new()
        };
        crate::tools::register_script_display_names(&restricted_tools);
        let resources = Arc::new(TurnResources {
            client: OpenAiCompatibleClient::from_config(config, paths)?,
            normal_tools: build_tool_registry(config, paths, AgentMode::Normal, false)?,
            dev_tools: build_tool_registry(config, paths, AgentMode::Dev, false)?,
            restricted_tools,
        });

        if self.entries.len() >= MAX_CACHED_TURN_RESOURCE_CONFIGS {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key);
        self.entries.insert(key, resources.clone());
        Ok(resources)
    }
}

// ── WebAuth 登录节流（DaemonState 持有它，只能一起走） ──
#[derive(Clone)]
pub(crate) struct WebAuth {
    pub(crate) password_digest: Option<[u8; 32]>,
    /// 按登录先后有序:超限淘汰最旧令牌,而不是把全部在用会话一起登出。
    pub(crate) sessions: Arc<Mutex<Vec<String>>>,
    pub(crate) attempts: Arc<Mutex<HashMap<IpAddr, LoginAttempt>>>,
}

#[derive(Clone, Copy)]
pub(crate) struct LoginAttempt {
    pub(crate) window_started: Instant,
    pub(crate) failures: u8,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LoginFailure {
    Invalid,
    RateLimited,
}

impl WebAuth {
    pub(crate) fn new(password: Option<&str>) -> Self {
        let password_digest = password.map(|password| {
            let mut digest = Sha256::new();
            digest.update(password.as_bytes());
            digest.finalize().into()
        });
        Self {
            password_digest,
            sessions: Arc::new(Mutex::new(Vec::new())),
            attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn required(&self) -> bool {
        self.password_digest.is_some()
    }

    pub(crate) fn is_authenticated(&self, supplied: Option<&str>) -> bool {
        if !self.required() {
            return true;
        }
        supplied.is_some_and(|token| {
            self.sessions
                .lock()
                .unwrap()
                .iter()
                .any(|existing| existing == token)
        })
    }

    pub(crate) fn login(&self, peer: IpAddr, password: &str) -> std::result::Result<String, LoginFailure> {
        let Some(expected) = self.password_digest else {
            return Ok(String::new());
        };
        let now = Instant::now();
        {
            let mut attempts = self.attempts.lock().unwrap();
            let entry = attempts.entry(peer).or_insert(LoginAttempt {
                window_started: now,
                failures: 0,
            });
            if now.duration_since(entry.window_started) >= LOGIN_WINDOW {
                entry.window_started = now;
                entry.failures = 0;
            }
            if entry.failures >= LOGIN_ATTEMPT_LIMIT {
                return Err(LoginFailure::RateLimited);
            }
        }

        let mut digest = Sha256::new();
        digest.update(password.as_bytes());
        let supplied: [u8; 32] = digest.finalize().into();
        if !constant_time_eq(&supplied, &expected) {
            let mut attempts = self.attempts.lock().unwrap();
            if let Some(entry) = attempts.get_mut(&peer) {
                entry.failures = entry.failures.saturating_add(1);
            }
            return Err(LoginFailure::Invalid);
        }

        let token = random_token(32);
        let mut sessions = self.sessions.lock().unwrap();
        sessions.push(token.clone());
        // 第 65 个登录淘汰最旧的一个令牌;此前是 sessions.clear() 全员登出。
        if sessions.len() > 64 {
            sessions.remove(0);
        }
        Ok(token)
    }
}

// ── cold_context ──
pub(crate) fn cold_context(config: &AppConfig, state_store: &StateStore) -> Result<ContextSnapshot> {
    let cumulative = state_store.session_cumulative_token_totals()?;
    Ok(ContextSnapshot {
        tokens: 0,
        window: config.active_context_window()?,
        cumulative_tokens: cumulative.total,
        cumulative_prompt_tokens: cumulative.prompt,
        cumulative_cache_read_tokens: cumulative.cache_read,
    })
}
