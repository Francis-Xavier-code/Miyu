use super::types::{OutboundMessage, PlatformConversation, SendReceipt};
use super::PlatformTurnContext;
use anyhow::Result;
use futures_util::future::BoxFuture;
use std::sync::Arc;

mod renderer;
mod reply_processor;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PluginDescriptor {
    pub(crate) id: &'static str,
    pub(crate) priority: i32,
    pub(crate) default_enabled: bool,
}

pub(crate) struct PlatformTurnInput {
    pub(crate) content: String,
    pub(crate) system_context: Vec<String>,
}

pub(crate) struct PreparedSend {
    pub(crate) primary: OutboundMessage,
    pub(crate) after_success: Vec<OutboundMessage>,
    pub(crate) fallback: Option<OutboundMessage>,
    pub(crate) suppress_final_reply: bool,
}

impl PreparedSend {
    pub(super) fn unchanged(message: OutboundMessage) -> Self {
        Self {
            primary: message,
            after_success: Vec::new(),
            fallback: None,
            suppress_final_reply: false,
        }
    }
}

pub(crate) trait PlatformPlugin: Send + Sync {
    fn descriptor(&self) -> PluginDescriptor;

    fn handle_command<'a>(
        &'a self,
        _context: &'a PlatformTurnContext,
        _text: &'a str,
    ) -> BoxFuture<'a, Result<Option<OutboundMessage>>> {
        Box::pin(async { Ok(None) })
    }

    fn before_turn<'a>(
        &'a self,
        _context: &'a PlatformTurnContext,
        _input: &'a mut PlatformTurnInput,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn before_send<'a>(
        &'a self,
        _context: &'a PlatformTurnContext,
        message: OutboundMessage,
    ) -> BoxFuture<'a, Result<PreparedSend>> {
        Box::pin(async move { Ok(PreparedSend::unchanged(message)) })
    }

    fn after_send<'a>(
        &'a self,
        _context: &'a PlatformTurnContext,
        _message: &'a OutboundMessage,
        _receipt: &'a SendReceipt,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
pub(crate) struct PlatformPluginRegistry {
    plugins: Vec<Arc<dyn PlatformPlugin>>,
}

impl PlatformPluginRegistry {
    pub(crate) fn built_in() -> Result<Self> {
        Ok(Self::new(vec![Arc::new(
            reply_processor::ReplyProcessorPlugin::new()?,
        )]))
    }

    pub(crate) fn new(mut plugins: Vec<Arc<dyn PlatformPlugin>>) -> Self {
        plugins.sort_by(|left, right| {
            right
                .descriptor()
                .priority
                .cmp(&left.descriptor().priority)
                .then_with(|| left.descriptor().id.cmp(right.descriptor().id))
        });
        Self { plugins }
    }

    pub(crate) async fn handle_command(
        &self,
        context: &PlatformTurnContext,
        text: &str,
    ) -> Option<OutboundMessage> {
        for plugin in self.enabled_plugins(context) {
            match plugin.handle_command(context, text).await {
                Ok(Some(response)) => return Some(response),
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    plugin = plugin.descriptor().id,
                    error = %error,
                    "platform plugin command failed"
                ),
            }
        }
        None
    }

    pub(crate) async fn before_turn(
        &self,
        context: &PlatformTurnContext,
        input: &mut PlatformTurnInput,
    ) {
        for plugin in self.enabled_plugins(context) {
            if let Err(error) = plugin.before_turn(context, input).await {
                tracing::warn!(
                    plugin = plugin.descriptor().id,
                    error = %error,
                    "platform plugin before-turn hook failed"
                );
            }
        }
    }

    pub(crate) async fn before_send(
        &self,
        context: &PlatformTurnContext,
        message: OutboundMessage,
    ) -> PreparedSend {
        let mut prepared = PreparedSend::unchanged(message);
        for plugin in self.enabled_plugins(context) {
            let previous = prepared.primary.clone();
            match plugin.before_send(context, prepared.primary).await {
                Ok(mut next) => {
                    if next.fallback.is_none() && next.primary.metadata != previous.metadata {
                        next.fallback = Some(previous);
                    }
                    prepared.after_success.append(&mut next.after_success);
                    prepared.suppress_final_reply |= next.suppress_final_reply;
                    if prepared.fallback.is_none() {
                        prepared.fallback = next.fallback;
                    }
                    prepared.primary = next.primary;
                }
                Err(error) => {
                    tracing::warn!(
                        plugin = plugin.descriptor().id,
                        error = %error,
                        "platform plugin before-send hook failed"
                    );
                    prepared.primary = previous;
                }
            }
        }
        prepared
    }

    pub(crate) async fn after_send(
        &self,
        context: &PlatformTurnContext,
        message: &OutboundMessage,
        receipt: &SendReceipt,
    ) {
        for plugin in self.enabled_plugins(context) {
            if let Err(error) = plugin.after_send(context, message, receipt).await {
                tracing::warn!(
                    plugin = plugin.descriptor().id,
                    error = %error,
                    "platform plugin after-send hook failed"
                );
            }
        }
    }

    fn enabled_plugins<'a>(
        &'a self,
        context: &'a PlatformTurnContext,
    ) -> impl Iterator<Item = &'a Arc<dyn PlatformPlugin>> + 'a {
        self.plugins.iter().filter(move |plugin| {
            let descriptor = plugin.descriptor();
            context.plugin_enabled(descriptor.id, descriptor.default_enabled)
        })
    }
}

#[allow(dead_code)]
fn _conversation_is_stable_key(_conversation: &PlatformConversation) {}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlugin {
        id: &'static str,
        priority: i32,
    }

    impl PlatformPlugin for TestPlugin {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                id: self.id,
                priority: self.priority,
                default_enabled: true,
            }
        }
    }

    #[test]
    fn registry_orders_by_priority_then_stable_id() {
        let registry = PlatformPluginRegistry::new(vec![
            Arc::new(TestPlugin {
                id: "z-last",
                priority: 1,
            }),
            Arc::new(TestPlugin {
                id: "b-second",
                priority: 10,
            }),
            Arc::new(TestPlugin {
                id: "a-first",
                priority: 10,
            }),
        ]);
        assert_eq!(
            registry
                .plugins
                .iter()
                .map(|plugin| plugin.descriptor().id)
                .collect::<Vec<_>>(),
            vec!["a-first", "b-second", "z-last"]
        );
    }
}
