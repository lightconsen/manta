//! Equivalence tests between `DefaultInboundPipeline` and a hand-assembled
//! stage runner.
//!
//! Both harnesses use the same [`InboundStage`] wrappers; the difference is
//! whether the stages are executed by `DefaultInboundPipeline` (which caches
//! the default pre/post stage lists) or by the test harness directly. These
//! tests construct both implementations from the same components, feed them
//! identical inputs, and assert that the observable outputs are the same.
//!
//! Observable outputs:
//! - `process(message)` returns the same `Option<RoutedMessage>`
//! - `flush(key)` returns the same list of `RoutedMessage`s
//! - The routed_tx channel receives the same messages
//! - Suppression behavior matches
//! - Queue mode selection matches
//! - Agent/workspace routing matches

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use crate::channels::envelope::SessionEnvelopeManager;
    use crate::channels::identity::IdentityValidator;
    use crate::channels::{Attachment, IncomingMessage, InputProvenance, MentionState};
    use crate::inbound::debounce::{DebouncedItem, InboundDebouncer, InboundDebouncerConfig};
    use crate::inbound::dispatch::{AutoReplyDispatch, AutoReplyDispatchConfig};
    use crate::inbound::media::MediaUnderstandingPipeline;
    use crate::inbound::queue::QueueModeResolver;
    use crate::inbound::router::{AgentRouter, AgentRouterConfig};
    use crate::inbound::stage::{
        build_routed_message, default_post_debounce_stages, default_pre_debounce_stages,
        run_inbound_stages, InboundContext, InboundStageAction,
    };
    use crate::inbound::{DefaultInboundPipeline, InboundPipeline, RoutedMessage};

    /// Shared test harness interface.
    #[async_trait]
    trait PipelineHarness: Send + Sync {
        async fn process(
            &self,
            msg: IncomingMessage,
        ) -> (Option<RoutedMessage>, Vec<RoutedMessage>);

        async fn flush(&self, key: &str) -> (Vec<RoutedMessage>, Vec<RoutedMessage>);
    }

    // ── DefaultInboundPipeline harness ───────────────────────────────────────

    struct DefaultPipelineHarness {
        pipeline: Arc<DefaultInboundPipeline>,
        routed_rx: tokio::sync::Mutex<mpsc::Receiver<RoutedMessage>>,
    }

    impl DefaultPipelineHarness {
        async fn new(
            debouncer: Arc<InboundDebouncer>,
            flush_rx: mpsc::Receiver<Vec<DebouncedItem>>,
            dispatch: AutoReplyDispatch,
            router: AgentRouter,
            envelope_manager: Option<SessionEnvelopeManager>,
        ) -> Self {
            let (routed_tx, routed_rx) = mpsc::channel::<RoutedMessage>(64);
            let envelope_manager = envelope_manager.unwrap_or_else(|| {
                let dir = tempfile::tempdir().expect("failed to create temp envelope dir");
                SessionEnvelopeManager::new(dir.into_path())
            });
            let pipeline = Arc::new(
                DefaultInboundPipeline::new(
                    debouncer,
                    MediaUnderstandingPipeline::new(),
                    dispatch,
                    QueueModeResolver::new(),
                    Arc::new(router),
                    routed_tx,
                    flush_rx,
                )
                .with_identity_validator(IdentityValidator::new())
                .with_envelope_manager(envelope_manager),
            );
            // Start the background loop on a clone so the harness keeps an Arc.
            pipeline.clone().start();
            Self {
                pipeline,
                routed_rx: tokio::sync::Mutex::new(routed_rx),
            }
        }
    }

    #[async_trait]
    impl PipelineHarness for DefaultPipelineHarness {
        async fn process(
            &self,
            msg: IncomingMessage,
        ) -> (Option<RoutedMessage>, Vec<RoutedMessage>) {
            let mut rx = self.routed_rx.lock().await;
            let result = self.pipeline.process(msg).await;
            // Drain any messages sent to the channel during this call, with a
            // bounded wait to avoid flakiness on slow CI runners.
            let channel_messages = drain_with_timeout(&mut rx, Duration::from_millis(200)).await;
            (result, channel_messages)
        }

        async fn flush(&self, key: &str) -> (Vec<RoutedMessage>, Vec<RoutedMessage>) {
            let mut rx = self.routed_rx.lock().await;
            let result = self.pipeline.flush(key).await;
            let channel_messages = drain_with_timeout(&mut rx, Duration::from_millis(200)).await;
            (result, channel_messages)
        }
    }

    // ── Stage-based harness ──────────────────────────────────────────────────

    struct StageHarness {
        pre_stages: Vec<Box<dyn crate::inbound::stage::InboundStage>>,
        post_stages: Vec<Box<dyn crate::inbound::stage::InboundStage>>,
        debouncer: Arc<InboundDebouncer>,
        routed_tx: mpsc::Sender<RoutedMessage>,
        routed_rx: tokio::sync::Mutex<mpsc::Receiver<RoutedMessage>>,
    }

    impl StageHarness {
        async fn new(
            debouncer: Arc<InboundDebouncer>,
            flush_rx: mpsc::Receiver<Vec<DebouncedItem>>,
            dispatch: AutoReplyDispatch,
            router: AgentRouter,
            envelope_manager: Option<SessionEnvelopeManager>,
        ) -> Arc<Self> {
            let (routed_tx, routed_rx) = mpsc::channel::<RoutedMessage>(64);
            let envelope_manager = envelope_manager.unwrap_or_else(|| {
                let dir = tempfile::tempdir().expect("failed to create temp envelope dir");
                SessionEnvelopeManager::new(dir.into_path())
            });
            let pre_stages = default_pre_debounce_stages(Some(IdentityValidator::new()), debouncer.clone());
            let post_stages = default_post_debounce_stages(
                MediaUnderstandingPipeline::new(),
                dispatch,
                Some(envelope_manager),
                QueueModeResolver::new(),
                std::sync::Arc::new(router),
            );

            let harness = Arc::new(Self {
                pre_stages,
                post_stages,
                debouncer: debouncer.clone(),
                routed_tx,
                routed_rx: tokio::sync::Mutex::new(routed_rx),
            });

            // Start the same background flush loop that DefaultInboundPipeline
            // runs so debounced messages are re-injected through post-stages.
            let harness_clone = harness.clone();
            tokio::spawn(async move {
                harness_clone.run_flush_loop(flush_rx).await;
            });

            harness
        }

        async fn run_flush_loop(&self, mut flush_rx: mpsc::Receiver<Vec<DebouncedItem>>) {
            while let Some(batch) = flush_rx.recv().await {
                for item in batch {
                    let mut ctx = InboundContext::new(item.message);
                    match run_inbound_stages(&self.post_stages, &mut ctx).await {
                        Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                            Ok(routed) => {
                                let _ = self.routed_tx.send(routed).await;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Stage harness flush routing failed");
                            }
                        },
                        Ok(action) => {
                            tracing::debug!(?action, "Stage harness flush terminal action");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Stage harness flush stage failed");
                        }
                    }
                }
            }
        }
    }

    #[async_trait]
    impl PipelineHarness for Arc<StageHarness> {
        async fn process(
            &self,
            msg: IncomingMessage,
        ) -> (Option<RoutedMessage>, Vec<RoutedMessage>) {
            let mut rx = self.routed_rx.lock().await;
            let mut ctx = InboundContext::new(msg);

            let result = match run_inbound_stages(&self.pre_stages, &mut ctx).await {
                Ok(InboundStageAction::Continue) => {
                    match run_inbound_stages(&self.post_stages, &mut ctx).await {
                        Ok(InboundStageAction::Continue) => match build_routed_message(&mut ctx) {
                            Ok(routed) => {
                                let _ = self.routed_tx.send(routed.clone()).await;
                                Some(routed)
                            }
                            Err(_) => None,
                        },
                        _ => None,
                    }
                }
                _ => None,
            };

            let channel_messages = drain_with_timeout(&mut rx, Duration::from_millis(200)).await;
            (result, channel_messages)
        }

        async fn flush(&self, key: &str) -> (Vec<RoutedMessage>, Vec<RoutedMessage>) {
            let mut rx = self.routed_rx.lock().await;
            let messages = self.debouncer.flush_key(key).await;
            let mut routed = Vec::new();
            for msg in messages {
                let mut ctx = InboundContext::new(msg);
                if let Ok(InboundStageAction::Continue) =
                    run_inbound_stages(&self.post_stages, &mut ctx).await
                {
                    match build_routed_message(&mut ctx) {
                        Ok(r) => {
                            let _ = self.routed_tx.send(r.clone()).await;
                            routed.push(r);
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Stage harness flush routing failed");
                        }
                    }
                }
            }
            let channel_messages = drain_with_timeout(&mut rx, Duration::from_millis(200)).await;
            (routed, channel_messages)
        }
    }

    /// Drain all currently available messages from `rx`, waiting up to
    /// `timeout` for the first message to arrive.
    async fn drain_with_timeout(
        rx: &mut mpsc::Receiver<RoutedMessage>,
        timeout: Duration,
    ) -> Vec<RoutedMessage> {
        let mut messages = Vec::new();
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(first)) => messages.push(first),
            _ => return messages,
        }
        while let Ok(m) = rx.try_recv() {
            messages.push(m);
        }
        messages
    }

    // ── Shared assertions ────────────────────────────────────────────────────

    async fn assert_process_equivalent(
        default_pipeline: &dyn PipelineHarness,
        stage: &dyn PipelineHarness,
        msg: IncomingMessage,
    ) {
        let (default_result, default_channel) = default_pipeline.process(msg.clone()).await;
        let (stage_result, stage_channel) = stage.process(msg).await;

        assert_routed_message_eq(&default_result, &stage_result, "process() return value mismatch");
        assert_channel_eq(&default_channel, &stage_channel, "process() routed_tx mismatch");
    }

    async fn assert_flush_equivalent(
        default_pipeline: &dyn PipelineHarness,
        stage: &dyn PipelineHarness,
        key: &str,
    ) {
        let (default_result, default_channel) = default_pipeline.flush(key).await;
        let (stage_result, stage_channel) = stage.flush(key).await;

        assert_channel_eq(&default_result, &stage_result, "flush() return value mismatch");
        assert_channel_eq(&default_channel, &stage_channel, "flush() routed_tx mismatch");
    }

    fn assert_routed_message_eq(
        left: &Option<RoutedMessage>,
        right: &Option<RoutedMessage>,
        context: &str,
    ) {
        match (left, right) {
            (None, None) => {}
            (Some(l), Some(r)) => {
                assert_eq!(l.agent_id, r.agent_id, "{context}: agent_id mismatch");
                assert_eq!(l.workspace_id, r.workspace_id, "{context}: workspace_id mismatch");
                assert_eq!(l.queue_mode, r.queue_mode, "{context}: queue_mode mismatch");
                assert_eq!(
                    l.suppress_delivery, r.suppress_delivery,
                    "{context}: suppress_delivery mismatch"
                );
                assert_eq!(l.incoming.content, r.incoming.content, "{context}: content mismatch");
                assert_eq!(
                    l.media_results.is_some(),
                    r.media_results.is_some(),
                    "{context}: media_results presence mismatch"
                );
            }
            (None, Some(_)) => {
                panic!("{context}: default pipeline returned None, stage returned Some")
            }
            (Some(_), None) => {
                panic!("{context}: default pipeline returned Some, stage returned None")
            }
        }
    }

    fn assert_channel_eq(left: &[RoutedMessage], right: &[RoutedMessage], context: &str) {
        assert_eq!(left.len(), right.len(), "{context}: channel message count mismatch");
        for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
            assert_eq!(l.agent_id, r.agent_id, "{context}: msg {i} agent_id mismatch");
            assert_eq!(l.workspace_id, r.workspace_id, "{context}: msg {i} workspace_id mismatch");
            assert_eq!(l.queue_mode, r.queue_mode, "{context}: msg {i} queue_mode mismatch");
            assert_eq!(
                l.suppress_delivery, r.suppress_delivery,
                "{context}: msg {i} suppress_delivery mismatch"
            );
            assert_eq!(
                l.media_results.is_some(),
                r.media_results.is_some(),
                "{context}: msg {i} media_results presence mismatch"
            );
            assert_eq!(
                l.incoming.content, r.incoming.content,
                "{context}: msg {i} content mismatch"
            );
        }
    }

    // ── Fixtures ─────────────────────────────────────────────────────────────

    fn plain_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "hello")
    }

    fn command_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "/help")
    }

    fn group_unmentioned_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "hello")
            .with_provenance(InputProvenance::ExternalUser {
                channel: "telegram".into(),
                is_direct: false,
            })
            .with_mention(MentionState::NotMentioned)
    }

    fn mentioned_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "hello")
            .with_provenance(InputProvenance::ExternalUser {
                channel: "telegram".into(),
                is_direct: false,
            })
            .with_mention(MentionState::Mentioned)
    }

    fn agent_mention_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "@sales I need help")
    }

    fn workspace_mention_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "#team-alpha hello")
    }

    fn interrupt_message() -> IncomingMessage {
        IncomingMessage::new("user1", "conv1", "!stop")
    }

    fn image_message() -> IncomingMessage {
        let attachment = Attachment::new("cat.png", "image/png").with_data(vec![1, 2, 3, 4]);
        IncomingMessage::new("user1", "conv1", "look at this").with_attachment(attachment)
    }

    fn dispatch_config_suppress_groups() -> AutoReplyDispatchConfig {
        AutoReplyDispatchConfig {
            suppress_unless_mentioned_in_groups: true,
            ..Default::default()
        }
    }

    fn router_config_default_agent(agent_id: &str) -> AgentRouterConfig {
        AgentRouterConfig {
            default_agent_id: agent_id.into(),
            ..Default::default()
        }
    }

    async fn build_default_pipeline(
        config: AutoReplyDispatchConfig,
        router_config: AgentRouterConfig,
        debounce_ms: u64,
    ) -> DefaultPipelineHarness {
        let (flush_tx, flush_rx) = mpsc::channel::<Vec<DebouncedItem>>(64);
        let debouncer = InboundDebouncer::new(
            InboundDebouncerConfig {
                debounce_ms,
                ..Default::default()
            },
            flush_tx,
        );
        let dispatch = AutoReplyDispatch::new(config);
        let router = AgentRouter::new(router_config);
        DefaultPipelineHarness::new(debouncer, flush_rx, dispatch, router, None).await
    }

    async fn build_stage(
        config: AutoReplyDispatchConfig,
        router_config: AgentRouterConfig,
        debounce_ms: u64,
    ) -> Arc<StageHarness> {
        let (flush_tx, flush_rx) = mpsc::channel::<Vec<DebouncedItem>>(64);
        let debouncer = InboundDebouncer::new(
            InboundDebouncerConfig {
                debounce_ms,
                ..Default::default()
            },
            flush_tx,
        );
        let dispatch = AutoReplyDispatch::new(config);
        let router = AgentRouter::new(router_config);
        StageHarness::new(debouncer, flush_rx, dispatch, router, None).await
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn equivalence_plain_message_routes_to_default() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, plain_message()).await;
    }

    #[tokio::test]
    async fn equivalence_command_bypasses_debounce() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, command_message()).await;
    }

    #[tokio::test]
    async fn equivalence_group_unmentioned_suppressed() {
        let default_pipeline = build_default_pipeline(
            dispatch_config_suppress_groups(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(dispatch_config_suppress_groups(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, group_unmentioned_message()).await;
    }

    #[tokio::test]
    async fn equivalence_group_mentioned_allowed() {
        let default_pipeline = build_default_pipeline(
            dispatch_config_suppress_groups(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(dispatch_config_suppress_groups(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, mentioned_message()).await;
    }

    #[tokio::test]
    async fn equivalence_agent_mention_overrides_default() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, agent_mention_message()).await;
    }

    #[tokio::test]
    async fn equivalence_workspace_mention_sets_workspace() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, workspace_mention_message()).await;
    }

    #[tokio::test]
    async fn equivalence_interrupt_sets_queue_mode() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, interrupt_message()).await;
    }

    #[tokio::test]
    async fn equivalence_image_attachment_produces_media_results() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            50,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 50)
                .await;
        assert_process_equivalent(&default_pipeline, &stage, image_message()).await;
    }

    #[tokio::test]
    async fn equivalence_flush_debounced_messages() {
        let default_pipeline = build_default_pipeline(
            AutoReplyDispatchConfig::default(),
            router_config_default_agent("default"),
            10_000,
        )
        .await;
        let stage =
            build_stage(AutoReplyDispatchConfig::default(), router_config_default_agent("default"), 10_000)
                .await;

        // Absorb a debounced message; it should not produce an immediate result.
        let (default_result, default_channel) =
            default_pipeline.process(plain_message()).await;
        let (stage_result, stage_channel) = stage.process(plain_message()).await;
        assert!(default_result.is_none());
        assert!(stage_result.is_none());
        assert!(default_channel.is_empty());
        assert!(stage_channel.is_empty());

        // Flushing the conversation should emit the same routed messages from
        // both harnesses.
        assert_flush_equivalent(&default_pipeline, &stage, "conv1").await;
    }
}
