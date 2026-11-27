//! Embedded mobile gateway runtime decoupled from UI/Activity lifecycles.
//!
//! Mirrors the desktop `paxri-gateway` model inside the mobile process: the gateway
//! owns the Tokio runtime, the authenticated [`Client`] transport, network-change
//! refreshes, and OS background-service synchronization through [`GatewayHost`].
use crate::{Update, client::Client};
use async_channel::Sender;
use std::{
    path::PathBuf,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::runtime::Handle;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServiceState {
    pub(crate) running: bool,
    pub(crate) background_service: bool,
    pub(crate) screen_off_keep_alive: bool,
    pub(crate) connected_count: usize,
    pub(crate) paired_count: usize,
}

/// Platform hook implemented by each mobile host (Android Foreground Service today,
/// iOS background task / Network Extension in the future).
pub(crate) trait GatewayHost: Send + Sync + 'static {
    fn sync_service(&self, state: ServiceState);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GatewayPolicy {
    pub(crate) background_service: bool,
    pub(crate) screen_off_keep_alive: bool,
}

impl Default for GatewayPolicy {
    fn default() -> Self {
        Self {
            background_service: true,
            screen_off_keep_alive: true,
        }
    }
}

impl From<&paxri_config::startup::GatewaySettings> for GatewayPolicy {
    fn from(settings: &paxri_config::startup::GatewaySettings) -> Self {
        Self {
            background_service: settings.background_service,
            screen_off_keep_alive: settings.screen_off_keep_alive,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum GatewayStatus {
    #[default]
    Stopped,
    Starting,
    Running {
        device_id: String,
        relay_url: Option<String>,
        connected_count: usize,
        paired_count: usize,
    },
    Stopping,
    Restarting,
    Failed(String),
}

impl GatewayStatus {
    pub(crate) fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    pub(crate) fn is_transitioning(&self) -> bool {
        matches!(self, Self::Starting | Self::Stopping | Self::Restarting)
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running { .. } => "Running",
            Self::Stopping => "Stopping",
            Self::Restarting => "Restarting",
            Self::Failed(_) => "Offline",
        }
    }

    pub(crate) fn device_id(&self) -> Option<&str> {
        match self {
            Self::Running { device_id, .. } => Some(device_id.as_str()),
            _ => None,
        }
    }

    pub(crate) fn relay_url(&self) -> Option<&str> {
        match self {
            Self::Running { relay_url, .. } => relay_url.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct UpdateSink(Arc<Mutex<Option<Sender<Update>>>>);

impl UpdateSink {
    pub(crate) fn replace(&self, tx: Sender<Update>) {
        *self.0.lock().unwrap() = Some(tx);
    }

    pub(crate) fn clear_if_same(&self, tx: &Sender<Update>) {
        let mut slot = self.0.lock().unwrap();
        if slot.as_ref().is_some_and(|active| active.same_channel(tx)) {
            slot.take();
        }
    }

    pub(crate) async fn send(&self, event: Update) {
        let tx = self.0.lock().unwrap().clone();
        if let Some(tx) = tx {
            let _ = tx.send(event).await;
        }
    }

    pub(crate) fn try_send(&self, event: Update) {
        let tx = self.0.lock().unwrap().clone();
        if let Some(tx) = tx {
            let _ = tx.try_send(event);
        }
    }
}

struct GatewayInner {
    data_dir: Option<PathBuf>,
    policy: GatewayPolicy,
    manually_stopped: bool,
    status: GatewayStatus,
    client: Option<Arc<Client>>,
    host: Option<Arc<dyn GatewayHost>>,
    sink: UpdateSink,
}

impl Default for GatewayInner {
    fn default() -> Self {
        Self {
            data_dir: None,
            policy: GatewayPolicy::default(),
            manually_stopped: false,
            status: GatewayStatus::Stopped,
            client: None,
            host: None,
            sink: UpdateSink::default(),
        }
    }
}

impl GatewayInner {
    fn service_state(&self) -> ServiceState {
        match &self.status {
            GatewayStatus::Running {
                connected_count,
                paired_count,
                ..
            } => ServiceState {
                running: true,
                background_service: self.policy.background_service,
                screen_off_keep_alive: self.policy.screen_off_keep_alive,
                connected_count: *connected_count,
                paired_count: *paired_count,
            },
            GatewayStatus::Starting | GatewayStatus::Restarting => ServiceState {
                running: self.client.is_some(),
                background_service: self.policy.background_service,
                screen_off_keep_alive: self.policy.screen_off_keep_alive,
                connected_count: 0,
                paired_count: 0,
            },
            GatewayStatus::Stopped | GatewayStatus::Stopping | GatewayStatus::Failed(_) => {
                ServiceState {
                    running: false,
                    background_service: self.policy.background_service,
                    screen_off_keep_alive: self.policy.screen_off_keep_alive,
                    connected_count: 0,
                    paired_count: 0,
                }
            }
        }
    }
}

pub(crate) struct MobileGateway {
    rt: tokio::runtime::Runtime,
    lifecycle: tokio::sync::Mutex<()>,
    inner: Mutex<GatewayInner>,
    refresh_in_flight: AtomicBool,
    refresh_queued: AtomicBool,
}

pub(crate) static GATEWAY: LazyLock<MobileGateway> = LazyLock::new(MobileGateway::new);

impl MobileGateway {
    fn new() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("Mobile gateway runtime");
        Self {
            rt,
            lifecycle: tokio::sync::Mutex::new(()),
            inner: Mutex::new(GatewayInner::default()),
            refresh_in_flight: AtomicBool::new(false),
            refresh_queued: AtomicBool::new(false),
        }
    }

    pub(crate) fn handle(&self) -> Handle {
        self.rt.handle().clone()
    }

    pub(crate) fn status(&self) -> GatewayStatus {
        self.inner.lock().unwrap().status.clone()
    }

    pub(crate) fn sync_host(&self) {
        let (host, state) = {
            let guard = self.inner.lock().unwrap();
            (guard.host.clone(), guard.service_state())
        };
        if let Some(host) = host {
            host.sync_service(state);
        }
    }

    fn publish_status(&self, status: GatewayStatus) {
        let (sink, host, service_state) = {
            let mut guard = self.inner.lock().unwrap();
            guard.status = status.clone();
            (guard.sink.clone(), guard.host.clone(), guard.service_state())
        };
        if let Some(host) = host {
            host.sync_service(service_state);
        }
        sink.try_send(Update::GatewayStatus(status));
    }

    pub(crate) fn set_policy(&self, policy: GatewayPolicy) {
        {
            let mut guard = self.inner.lock().unwrap();
            guard.policy = policy;
        }
        self.sync_host();
    }

    pub(crate) fn attach_ui(
        &self,
        data_dir: Option<PathBuf>,
        policy: GatewayPolicy,
        host: Arc<dyn GatewayHost>,
        tx: Sender<Update>,
    ) {
        enum AttachPlan {
            Reattach(Arc<Client>, GatewayStatus),
            ReportOnly(GatewayStatus),
            Start,
        }

        let plan = {
            let mut guard = self.inner.lock().unwrap();
            if data_dir.is_some() {
                guard.data_dir = data_dir;
            }
            guard.policy = policy;
            guard.host = Some(host.clone());
            guard.sink.replace(tx.clone());
            if let Some(client) = guard.client.clone() {
                AttachPlan::Reattach(client, guard.status.clone())
            } else if guard.status.is_transitioning()
                || (guard.manually_stopped && matches!(guard.status, GatewayStatus::Stopped))
            {
                AttachPlan::ReportOnly(guard.status.clone())
            } else {
                AttachPlan::Start
            }
        };

        self.sync_host();

        match plan {
            AttachPlan::Reattach(client, status) => {
                self.rt.spawn(async move {
                    let _ = tx.send(Update::GatewayStatus(status)).await;
                    let peers = client.list().await;
                    client.notify_ui_attached();
                    let _ = tx.send(Update::Ready(client, peers)).await;
                });
            }
            AttachPlan::ReportOnly(status) => {
                let _ = tx.try_send(Update::GatewayStatus(status));
            }
            AttachPlan::Start => {
                self.start();
            }
        }
    }

    pub(crate) fn detach_ui(&self, tx: &Sender<Update>) {
        self.inner.lock().unwrap().sink.clear_if_same(tx);
    }

    pub(crate) fn on_activity_finishing(&self) {
        let should_stop = {
            let mut guard = self.inner.lock().unwrap();
            if !guard.policy.background_service && guard.client.is_some() {
                true
            } else {
                // If the user previously stopped the gateway and is now exiting the app,
                // start it again on the next fresh launch (matching desktop `startup = gui`).
                if matches!(guard.status, GatewayStatus::Stopped) {
                    guard.manually_stopped = false;
                }
                false
            }
        };
        if should_stop {
            self.stop_internal(false);
        }
    }

    pub(crate) fn on_task_removed(&self) {
        let should_stop = {
            let mut guard = self.inner.lock().unwrap();
            if !guard.policy.background_service {
                guard.manually_stopped = false;
                guard.client.is_some() || guard.status.is_running()
            } else {
                false
            }
        };
        if should_stop {
            self.stop_internal(false);
        }
    }

    pub(crate) fn start(&self) {
        self.rt.spawn(async {
            GATEWAY.start_locked(false).await;
        });
    }

    pub(crate) fn stop(&self) {
        self.stop_internal(true);
    }

    pub(crate) fn restart(&self) {
        self.rt.spawn(async {
            GATEWAY.restart_locked().await;
        });
    }

    fn stop_internal(&self, manual: bool) {
        self.rt.spawn(async move {
            GATEWAY.stop_locked(manual).await;
        });
    }

    async fn start_locked(&self, restarting: bool) {
        let _guard = self.lifecycle.lock().await;
        {
            let mut guard = self.inner.lock().unwrap();
            guard.manually_stopped = false;
            if guard.client.is_some() {
                return;
            }
        }
        self.publish_status(if restarting {
            GatewayStatus::Restarting
        } else {
            GatewayStatus::Starting
        });

        let (dir, sink) = {
            let guard = self.inner.lock().unwrap();
            (guard.data_dir.clone(), guard.sink.clone())
        };
        let Some(dir) = dir else {
            let err = "App storage unavailable".to_owned();
            self.publish_status(GatewayStatus::Failed(err.clone()));
            sink.send(Update::Error(err)).await;
            return;
        };

        let relay_url = match crate::settings::load_dir(&dir) {
            Ok(local) => {
                self.inner.lock().unwrap().policy = GatewayPolicy::from(&local.gateway);
                local
                    .native
                    .relay_url
                    .as_ref()
                    .map(|url| url.as_str().to_owned())
            }
            Err(error) => {
                log::warn!("Reading node.toml before gateway start: {error:#}");
                None
            }
        };

        let start_result = Client::start(
            &dir,
            relay_url.clone(),
            sink.clone(),
            |paired_count, connected_count| {
                GATEWAY.on_connections_changed(paired_count, connected_count);
            },
        )
        .await;

        match start_result {
            Ok(client) => {
                let peers = client.list().await;
                let status = GatewayStatus::Running {
                    device_id: client.device_id().to_owned(),
                    relay_url,
                    connected_count: 0,
                    paired_count: peers.len(),
                };
                {
                    let mut guard = self.inner.lock().unwrap();
                    guard.client = Some(client.clone());
                }
                self.publish_status(status);
                sink.send(Update::Ready(client, peers)).await;
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.publish_status(GatewayStatus::Failed(message.clone()));
                sink.send(Update::Error(message)).await;
            }
        }
    }

    async fn stop_locked(&self, manual: bool) {
        let _guard = self.lifecycle.lock().await;
        let (client, sink) = {
            let mut guard = self.inner.lock().unwrap();
            guard.manually_stopped = manual;
            (guard.client.take(), guard.sink.clone())
        };
        if let Some(client) = client {
            self.publish_status(GatewayStatus::Stopping);
            client.shutdown().await;
        }
        self.publish_status(GatewayStatus::Stopped);
        sink.send(Update::GatewayStopped).await;
    }

    async fn restart_locked(&self) {
        let _guard = self.lifecycle.lock().await;
        let (old_client, sink, dir) = {
            let mut guard = self.inner.lock().unwrap();
            guard.manually_stopped = false;
            (
                guard.client.take(),
                guard.sink.clone(),
                guard.data_dir.clone(),
            )
        };
        self.publish_status(GatewayStatus::Restarting);
        sink.send(Update::GatewayStopped).await;
        if let Some(client) = old_client {
            client.shutdown().await;
        }

        let Some(dir) = dir else {
            let err = "App storage unavailable".to_owned();
            self.publish_status(GatewayStatus::Failed(err.clone()));
            sink.send(Update::Error(err)).await;
            return;
        };

        let relay_url = match crate::settings::load_dir(&dir) {
            Ok(local) => {
                self.inner.lock().unwrap().policy = GatewayPolicy::from(&local.gateway);
                local
                    .native
                    .relay_url
                    .as_ref()
                    .map(|url| url.as_str().to_owned())
            }
            Err(error) => {
                log::warn!("Reading node.toml before gateway restart: {error:#}");
                None
            }
        };

        match Client::start(
            &dir,
            relay_url.clone(),
            sink.clone(),
            |paired_count, connected_count| {
                GATEWAY.on_connections_changed(paired_count, connected_count);
            },
        )
        .await
        {
            Ok(client) => {
                let peers = client.list().await;
                let status = GatewayStatus::Running {
                    device_id: client.device_id().to_owned(),
                    relay_url,
                    connected_count: 0,
                    paired_count: peers.len(),
                };
                {
                    let mut guard = self.inner.lock().unwrap();
                    guard.client = Some(client.clone());
                }
                self.publish_status(status);
                sink.send(Update::Ready(client, peers)).await;
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.publish_status(GatewayStatus::Failed(message.clone()));
                sink.send(Update::Error(message)).await;
            }
        }
    }

    fn on_connections_changed(&self, paired_count: usize, connected_count: usize) {
        let update = {
            let mut guard = self.inner.lock().unwrap();
            if let GatewayStatus::Running {
                connected_count: ref mut conn,
                paired_count: ref mut paired,
                ..
            } = guard.status
            {
                if *conn == connected_count && *paired == paired_count {
                    return;
                }
                *conn = connected_count;
                *paired = paired_count;
                Some((guard.status.clone(), guard.sink.clone(), guard.host.clone(), guard.service_state()))
            } else {
                None
            }
        };
        if let Some((status, sink, host, service_state)) = update {
            if let Some(host) = host {
                host.sync_service(service_state);
            }
            sink.try_send(Update::GatewayStatus(status));
        }
    }

    pub(crate) fn trigger_network_refresh(&self) {
        if self.refresh_in_flight.swap(true, Ordering::AcqRel) {
            self.refresh_queued.store(true, Ordering::Release);
            return;
        }
        self.rt.spawn(async {
            loop {
                GATEWAY.refresh_queued.store(false, Ordering::Release);
                // Coalesce bursty link-property updates during Wi-Fi/cellular handoff.
                tokio::time::sleep(Duration::from_millis(350)).await;
                let target = {
                    let guard = GATEWAY.inner.lock().unwrap();
                    guard
                        .client
                        .clone()
                        .map(|client| (client, guard.sink.clone()))
                };
                if let Some((client, sink)) = target {
                    client.refresh_network().await;
                    sink.send(Update::NetworkRefreshed).await;
                }
                if !GATEWAY.refresh_queued.load(Ordering::Acquire) {
                    GATEWAY.refresh_in_flight.store(false, Ordering::Release);
                    if !GATEWAY.refresh_queued.load(Ordering::Acquire)
                        || GATEWAY.refresh_in_flight.swap(true, Ordering::AcqRel)
                    {
                        break;
                    }
                }
            }
        });
    }
}
