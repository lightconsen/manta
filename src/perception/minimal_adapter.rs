//! [`MinimalAdapter`] — the default implementation of
//! [`AgentPerceptionAdapter`].
//!
//! `MinimalAdapter` wires together the per-agent stages
//! ([`AttentionGate`] + [`SalienceFilter`]) and the shared upstream
//! infrastructure ([`PerceptionStreamHub`] + [`DerivedStreamHub`] +
//! [`DefaultTemporalProcessor`]) into a single facade an agent can
//! talk to.
//!
//! ```text
//!  raw_hub ────────► [SalienceFilter] ─► Change ─┐
//!                                                ├─► [AttentionGate] ─► pending queue
//!  derived_hub ─► (Entity/Discrete/Anomaly) ─────┘                            │
//!                                                                             ▼
//!                                                           agent.next_event() / now().recent_events
//! ```
//!
//! Two background tasks are spawned at construction:
//!
//! 1. A *derived-event* forwarder that pulls already-cooked events
//!    (Entity/Discrete/Anomaly) and runs them through the gate.
//! 2. A *raw-observation* forwarder that runs each observation through
//!    the salience filter, producing `Event::Change` when the value
//!    drifts above threshold, then through the gate.
//!
//! Both tasks halt when [`shutdown`] is called.
//!
//! [`shutdown`]: AgentPerceptionAdapter::shutdown

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::{broadcast, Notify};
use tokio::task::JoinHandle;

use crate::perception::{
    AdapterError, AgentPerceptionAdapter, AttentionGate, DerivedStreamHub,
    DefaultTemporalProcessor, Event, Focus, PerceptionStreamHub, PerceptionSummarizer,
    SalienceFilter, Snapshot, TemporalProcessor,
};

/// Construction-time tuning for [`MinimalAdapter`].
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    /// Max events held in the `next_event` queue. Oldest events are
    /// dropped when the queue overflows (FIFO with bounded depth).
    pub max_pending: usize,
    /// Max events mirrored into [`Snapshot::recent_events`]. Older
    /// events fall off the back of the ring.
    pub max_recent: usize,
}

impl Default for AdapterConfig {
    fn default() -> Self {
        Self {
            max_pending: 256,
            max_recent: 64,
        }
    }
}

/// Internal shared state — held in `Arc` so the spawned tasks can
/// reach in.
struct Inner {
    focus: Mutex<Focus>,
    gate: Mutex<AttentionGate>,
    filter: Mutex<SalienceFilter>,

    pending: Mutex<VecDeque<Event>>,
    notify: Notify,

    recent: Mutex<VecDeque<Event>>,

    temporal: Arc<DefaultTemporalProcessor>,
    summarizer: Option<Arc<dyn PerceptionSummarizer>>,

    shutdown: AtomicBool,
    config: AdapterConfig,
}

impl Inner {
    /// Push a (gate-admitted) event onto both the pending queue and
    /// the recent ring; signal any waiters.
    fn push_event(&self, ev: Event) {
        {
            let mut p = self.pending.lock().expect("pending poisoned");
            if p.len() >= self.config.max_pending {
                p.pop_front();
            }
            p.push_back(ev.clone());
        }
        {
            let mut r = self.recent.lock().expect("recent poisoned");
            if r.len() >= self.config.max_recent {
                r.pop_front();
            }
            r.push_back(ev);
        }
        self.notify.notify_one();
    }
}

/// Default per-agent adapter.
pub struct MinimalAdapter {
    inner: Arc<Inner>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl MinimalAdapter {
    /// Spawn the per-agent forwarder tasks and return a ready-to-use
    /// adapter.
    ///
    /// `summarizer` is optional — if `None`, [`Self::summarize`] returns
    /// [`AdapterError::Summarizer`] explaining no LLM is wired.
    pub fn new(
        raw_hub: Arc<PerceptionStreamHub>,
        derived_hub: Arc<DerivedStreamHub>,
        temporal: Arc<DefaultTemporalProcessor>,
        summarizer: Option<Arc<dyn PerceptionSummarizer>>,
        focus: Focus,
        config: AdapterConfig,
    ) -> Arc<Self> {
        let gate = AttentionGate::new(focus.clone());
        let filter = SalienceFilter::new(focus.salience.clone());

        let inner = Arc::new(Inner {
            focus: Mutex::new(focus),
            gate: Mutex::new(gate),
            filter: Mutex::new(filter),
            pending: Mutex::new(VecDeque::with_capacity(config.max_pending)),
            notify: Notify::new(),
            recent: Mutex::new(VecDeque::with_capacity(config.max_recent)),
            temporal,
            summarizer,
            shutdown: AtomicBool::new(false),
            config,
        });

        let derived_handle = spawn_derived_task(inner.clone(), derived_hub);
        let raw_handle = spawn_raw_task(inner.clone(), raw_hub);

        Arc::new(Self {
            inner,
            handles: Mutex::new(vec![derived_handle, raw_handle]),
        })
    }
}

fn spawn_derived_task(inner: Arc<Inner>, derived_hub: Arc<DerivedStreamHub>) -> JoinHandle<()> {
    let mut rx = derived_hub.subscribe();
    tokio::spawn(async move {
        loop {
            if inner.shutdown.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv().await {
                Ok(ev) => {
                    let admitted = {
                        let mut g = inner.gate.lock().expect("gate poisoned");
                        g.admit(&ev)
                    };
                    if admitted {
                        inner.push_event(ev);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("adapter derived: lagged, skipped {} events", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        inner.notify.notify_waiters();
    })
}

fn spawn_raw_task(inner: Arc<Inner>, raw_hub: Arc<PerceptionStreamHub>) -> JoinHandle<()> {
    let mut rx = raw_hub.subscribe();
    tokio::spawn(async move {
        loop {
            if inner.shutdown.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv().await {
                Ok(obs) => {
                    // Cheap pre-filter using current focus.
                    let pre_pass = {
                        let f = inner.focus.lock().expect("focus poisoned");
                        f.admits_modality(obs.modality) && f.admits_source(&obs.source)
                    };
                    if !pre_pass {
                        continue;
                    }
                    let maybe_change = {
                        let mut sf = inner.filter.lock().expect("filter poisoned");
                        sf.evaluate(&obs)
                    };
                    if let Some(ev) = maybe_change {
                        let admitted = {
                            let mut g = inner.gate.lock().expect("gate poisoned");
                            g.admit(&ev)
                        };
                        if admitted {
                            inner.push_event(ev);
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("adapter raw: lagged, skipped {} obs", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        inner.notify.notify_waiters();
    })
}

#[async_trait]
impl AgentPerceptionAdapter for MinimalAdapter {
    async fn focus(&self, focus: Focus) {
        // Per spec: gate whitelist replaced + freq reset; salience
        // baselines retained, dedup cleared.
        {
            let mut g = self.inner.gate.lock().expect("gate poisoned");
            g.set_focus(focus.clone());
        }
        {
            let mut sf = self.inner.filter.lock().expect("filter poisoned");
            sf.set_config(focus.salience.clone());
        }
        {
            let mut f = self.inner.focus.lock().expect("focus poisoned");
            *f = focus;
        }
    }

    async fn current_focus(&self) -> Focus {
        self.inner.focus.lock().expect("focus poisoned").clone()
    }

    fn now(&self) -> Snapshot {
        let aggregates = self.inner.temporal.snapshot_aggregates();
        let recent: Vec<Event> = self
            .inner
            .recent
            .lock()
            .expect("recent poisoned")
            .iter()
            .cloned()
            .collect();
        Snapshot {
            at: SystemTime::now(),
            entities: Vec::new(),
            aggregates,
            recent_events: recent,
        }
    }

    async fn next_event(&self) -> Option<Event> {
        loop {
            let notified = self.inner.notify.notified();
            {
                let mut q = self.inner.pending.lock().expect("pending poisoned");
                if let Some(ev) = q.pop_front() {
                    return Some(ev);
                }
                if self.inner.shutdown.load(Ordering::SeqCst) {
                    return None;
                }
            }
            notified.await;
        }
    }

    async fn summarize(&self, dur: Duration) -> Result<String, AdapterError> {
        let summarizer = self
            .inner
            .summarizer
            .clone()
            .ok_or_else(|| AdapterError::Summarizer("no summarizer configured".into()))?;

        let cutoff = SystemTime::now()
            .checked_sub(dur)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let recent: Vec<Event> = self
            .inner
            .recent
            .lock()
            .expect("recent poisoned")
            .iter()
            .filter(|e| e.at() >= cutoff)
            .cloned()
            .collect();

        let aggregates = self.inner.temporal.snapshot_aggregates();
        let aggregates_vec: Vec<_> = aggregates.values().collect();

        let user_msg = serde_json::json!({
            "duration_ms": dur.as_millis(),
            "recent_events": recent,
            "aggregates": aggregates_vec,
        })
        .to_string();
        let system =
            "You are a perception summariser. Given recent events and sliding-window \
             aggregates from an agent's sensors, write ONE concise sentence describing \
             what is happening in the agent's environment.";

        summarizer.summarize(system, &user_msg).await
    }

    async fn shutdown(self: Arc<Self>) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
        let handles = std::mem::take(&mut *self.handles.lock().expect("handles poisoned"));
        for h in handles {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{
        AnomalyKind, FusedEntity, Modality, MockPerceptionSource, Observation, ObservationId,
        PerceptionSource,
    };
    use std::collections::HashMap;
    use std::time::Instant;

    fn obs(source: &str, modality: Modality, data: serde_json::Value) -> Observation {
        Observation {
            id: ObservationId::new(),
            source: source.to_string(),
            modality,
            timestamp: Instant::now(),
            created_at: SystemTime::now(),
            confidence: 1.0,
            data,
        }
    }

    fn anomaly_event(source: &str) -> Event {
        Event::Anomaly {
            source: source.to_string(),
            reason: AnomalyKind::SourceFault,
            severity: 200,
            at: SystemTime::now(),
        }
    }

    fn entity_event(id: &str) -> Event {
        let now_inst = Instant::now();
        Event::Entity {
            entity: FusedEntity {
                id: id.into(),
                label: id.into(),
                created_at: now_inst,
                updated_at: now_inst,
                confidence: 1.0,
                modalities: vec![Modality::System],
                observation_ids: vec![],
                properties: HashMap::new(),
                correlation_key: "k".into(),
            },
            at: SystemTime::now(),
        }
    }

    async fn build_adapter(
        focus: Focus,
    ) -> (
        Arc<MinimalAdapter>,
        Arc<PerceptionStreamHub>,
        Arc<DerivedStreamHub>,
        Arc<DefaultTemporalProcessor>,
        broadcast::Sender<Observation>,
    ) {
        let raw_hub = Arc::new(PerceptionStreamHub::new(64));
        let derived_hub = Arc::new(DerivedStreamHub::new(64));
        let temporal = Arc::new(DefaultTemporalProcessor::with_default_window());

        // Streaming source so raw_hub has a forwarder.
        let (mock, tx) = MockPerceptionSource::new("cpu").with_streaming(64);
        raw_hub
            .attach_source("cpu", Arc::new(mock) as Arc<dyn PerceptionSource>)
            .await;

        let adapter = MinimalAdapter::new(
            raw_hub.clone(),
            derived_hub.clone(),
            temporal.clone(),
            None,
            focus,
            AdapterConfig::default(),
        );
        (adapter, raw_hub, derived_hub, temporal, tx)
    }

    #[tokio::test]
    async fn test_now_returns_empty_initially() {
        let (adapter, _raw, _der, _t, _tx) = build_adapter(Focus::default()).await;
        let snap = adapter.now();
        assert_eq!(snap.item_count(), 0);
    }

    #[tokio::test]
    async fn test_focus_round_trip() {
        let (adapter, _raw, _der, _t, _tx) = build_adapter(Focus::default()).await;
        let f = Focus::default().with_modalities([Modality::System]);
        adapter.focus(f).await;
        let got = adapter.current_focus().await;
        assert!(got.admits_modality(Modality::System));
        assert!(!got.admits_modality(Modality::Audio));
    }

    #[tokio::test]
    async fn test_anomaly_event_reaches_next_event() {
        let (adapter, _raw, derived_hub, _t, _tx) = build_adapter(Focus::default()).await;
        derived_hub.publish(anomaly_event("cpu"));
        let ev = tokio::time::timeout(Duration::from_secs(1), adapter.next_event())
            .await
            .expect("timeout")
            .expect("pipeline closed");
        assert!(ev.is_anomaly());
    }

    #[tokio::test]
    async fn test_entity_event_reaches_recent_events() {
        let (adapter, _raw, derived_hub, _t, _tx) = build_adapter(Focus::default()).await;
        derived_hub.publish(entity_event("win1"));
        // Wait for it to be queued.
        for _ in 0..50 {
            let snap = adapter.now();
            if !snap.recent_events.is_empty() {
                assert!(matches!(snap.recent_events[0], Event::Entity { .. }));
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("entity never appeared in recent_events");
    }

    #[tokio::test]
    async fn test_modality_whitelist_blocks_event() {
        let focus = Focus::default().with_modalities([Modality::FileSystem]);
        let (adapter, _raw, derived_hub, _t, _tx) = build_adapter(focus).await;
        // Change event for System — should be blocked by gate.
        let blocked = Event::Change {
            source: "cpu".into(),
            modality: Modality::System,
            from: serde_json::json!(0),
            to: serde_json::json!(100),
            at: SystemTime::now(),
        };
        derived_hub.publish(blocked);
        // Should NOT receive (check briefly).
        let res = tokio::time::timeout(Duration::from_millis(150), adapter.next_event()).await;
        assert!(res.is_err(), "expected timeout — gate should have blocked");
    }

    #[tokio::test]
    async fn test_raw_obs_through_salience_filter_emits_change() {
        let focus = Focus::default()
            .with_modalities([Modality::System])
            .with_delta_threshold(Modality::System, 5.0);
        let (adapter, _raw, _der, _t, tx) = build_adapter(focus).await;

        // First obs installs baseline (no event).
        tx.send(obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0})))
            .unwrap();
        // Allow forwarder to process.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Second obs is a 100% jump → SalienceFilter emits Change.
        tx.send(obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 20.0})))
            .unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(2), adapter.next_event())
            .await
            .expect("timeout")
            .expect("pipeline closed");
        match ev {
            Event::Change { source, modality, .. } => {
                assert_eq!(source, "cpu");
                assert_eq!(modality, Modality::System);
            }
            other => panic!("expected Change, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_summarize_without_summarizer_returns_error() {
        let (adapter, _raw, _der, _t, _tx) = build_adapter(Focus::default()).await;
        let res = adapter.summarize(Duration::from_secs(5)).await;
        assert!(matches!(res, Err(AdapterError::Summarizer(_))));
    }

    #[tokio::test]
    async fn test_summarize_with_stub_summarizer() {
        struct StubSum;
        #[async_trait]
        impl PerceptionSummarizer for StubSum {
            async fn summarize(
                &self,
                _system: &str,
                user: &str,
            ) -> Result<String, AdapterError> {
                Ok(format!("got-{}-bytes", user.len()))
            }
        }

        let raw_hub = Arc::new(PerceptionStreamHub::new(64));
        let derived_hub = Arc::new(DerivedStreamHub::new(64));
        let temporal = Arc::new(DefaultTemporalProcessor::with_default_window());
        let summarizer: Arc<dyn PerceptionSummarizer> = Arc::new(StubSum);

        let adapter = MinimalAdapter::new(
            raw_hub,
            derived_hub.clone(),
            temporal,
            Some(summarizer),
            Focus::default(),
            AdapterConfig::default(),
        );

        derived_hub.publish(anomaly_event("cpu"));
        // Let it land in `recent`.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let s = adapter.summarize(Duration::from_secs(60)).await.unwrap();
        assert!(s.starts_with("got-"));
    }

    #[tokio::test]
    async fn test_shutdown_returns_none_from_next_event() {
        let (adapter, _raw, _der, _t, _tx) = build_adapter(Focus::default()).await;
        let a = adapter.clone();
        a.shutdown().await;
        // Should now return None promptly.
        let res = tokio::time::timeout(Duration::from_secs(1), adapter.next_event()).await;
        match res {
            Ok(None) => {}
            other => panic!("expected None on shutdown, got {other:?}"),
        }
    }
}
