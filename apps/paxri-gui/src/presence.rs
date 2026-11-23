use super::{Command, Dashboard, Status};
use gpui::Context;
use tokio::io::AsyncReadExt as _;

impl Dashboard {
    pub(super) fn peer_online(&self, peer: &str) -> bool {
        self.status.as_ref().is_some_and(|s| s.peer_online(peer))
    }

    pub(super) fn can_open_shell(&self, peer: &str) -> bool {
        self.status.as_ref().is_some_and(|s| s.can_open_shell(peer))
    }

    pub(super) fn watch_presence(&mut self, status: &Status, cx: &mut Context<Self>) {
        let Some(address) = status.ipc_path.as_ref().filter(|_| status.peer_presence) else {
            self.presence_watch = None;
            return;
        };
        if self
            .presence_watch
            .as_ref()
            .is_some_and(|(path, _)| path == address)
        {
            return;
        }
        let (notify, events) = async_channel::bounded(1);
        let path = address.clone();
        self.runtime.spawn(async move {
            let Ok(mut stream) = paxri_control::watch_peer_connections(&path).await else {
                return;
            };
            loop {
                tokio::select! {
                    _ = notify.closed() => return,
                    byte = stream.read_u8() => {
                        if byte.is_err() { return; }
                        let _ = notify.try_send(());
                    }
                }
            }
        });
        let watch = cx.spawn(async move |this, cx| {
            while events.recv().await.is_ok() {
                if this
                    .update(cx, |view, cx| {
                        if view.busy {
                            view.refresh_presence = true;
                        } else {
                            view.send(Command::Status, cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |view, _| view.presence_watch = None);
        });
        self.presence_watch = Some((address.clone(), watch));
    }
}
