use anyhow::Result;
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ConversationKind {
    Private,
    Group,
}

impl ConversationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
        }
    }
}

/// Stable identity of a transport conversation. The bot account is part of
/// the key so two OneBot accounts can never share history or routing rules.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PlatformConversation {
    pub(crate) platform: String,
    pub(crate) account_id: String,
    pub(crate) kind: ConversationKind,
    pub(crate) conversation_id: String,
}

impl PlatformConversation {
    pub(crate) fn scope_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.platform,
            self.account_id,
            self.kind.as_str(),
            self.conversation_id
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutboundOrigin {
    FinalReply,
    Tool,
    Command,
    Plugin,
}

#[derive(Clone, Debug)]
pub(crate) enum OutboundSegment {
    /// Agent output that still contains Markdown. Adapters flatten this only
    /// after plugins have had a chance to render it.
    Markdown(String),
    Text(String),
    Mention(String),
    ImageBytes {
        mime: String,
        data: Arc<[u8]>,
        alt: String,
    },
    ImagePath {
        path: PathBuf,
        alt: String,
    },
    FilePath {
        path: PathBuf,
        name: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ForwardNode {
    pub(crate) user_id: String,
    pub(crate) display_name: String,
    pub(crate) segments: Vec<OutboundSegment>,
}

#[derive(Clone, Debug)]
pub(crate) enum OutboundBody {
    Segments(Vec<OutboundSegment>),
    Forward(Vec<ForwardNode>),
}

#[derive(Clone, Debug)]
pub(crate) struct OutboundMessage {
    pub(crate) body: OutboundBody,
    pub(crate) reply_to: Option<String>,
    pub(crate) origin: OutboundOrigin,
    /// Plugin-private, in-memory metadata. Adapters never serialize it.
    pub(crate) metadata: BTreeMap<String, Value>,
}

impl OutboundMessage {
    pub(crate) fn segments(origin: OutboundOrigin, segments: Vec<OutboundSegment>) -> Self {
        Self {
            body: OutboundBody::Segments(segments),
            reply_to: None,
            origin,
            metadata: BTreeMap::new(),
        }
    }

    pub(crate) fn text(origin: OutboundOrigin, text: impl Into<String>) -> Self {
        Self::segments(origin, vec![OutboundSegment::Text(text.into())])
    }

    pub(crate) fn markdown(origin: OutboundOrigin, text: impl Into<String>) -> Self {
        Self::segments(origin, vec![OutboundSegment::Markdown(text.into())])
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SendReceipt {
    pub(crate) message_ids: Vec<String>,
}

/// Protocol adapter capability used by the platform-neutral output pipeline.
pub(crate) trait PlatformAdapter: Send + Sync {
    fn send<'a>(&'a self, message: OutboundMessage) -> BoxFuture<'a, Result<SendReceipt>>;

    fn bot_display_name<'a>(&'a self) -> BoxFuture<'a, Result<String>>;
}
