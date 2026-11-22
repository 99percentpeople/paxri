//! Recent RTT observations from the GUI's existing gateway status refresh.
use crate::components::Ui;
use gpui::{prelude::*, *};
use peerhatch_control::{PathKind, PeerConnection, Status};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const BUCKET: Duration = Duration::from_secs(1);

#[derive(Hash, PartialEq, Eq)]
struct Route {
    peer_id: String,
    protocol: String,
    kind: PathKind,
    address: Option<String>,
}

impl Route {
    fn new(peer_id: &str, path: &PeerConnection) -> Self {
        Self {
            peer_id: peer_id.into(),
            protocol: path.protocol.clone(),
            kind: path.path,
            address: path.address.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct LatencyHistory {
    routes: HashMap<Route, VecDeque<(Instant, u64)>>,
}

impl LatencyHistory {
    pub(crate) fn record(&mut self, status: &Status) {
        let now = Instant::now();
        let mut previous = std::mem::take(&mut self.routes);
        let Some(peers) = &status.peer_connections else {
            return;
        };
        for (name, paths) in peers {
            let Some(id) = status.peers.get(name) else {
                continue;
            };
            for path in paths {
                let Some(ms) = path.rtt_ms else { continue };
                let route = Route::new(id, path);
                let mut samples = previous.remove(&route).unwrap_or_default();
                while samples
                    .front()
                    .is_some_and(|(at, _)| now.duration_since(*at) > WINDOW)
                {
                    samples.pop_front();
                }
                // Edits and presence events may produce several snapshots between periodic reads.
                if let Some((at, value)) = samples.back_mut()
                    && now.duration_since(*at) < BUCKET
                {
                    *value = ms;
                } else {
                    samples.push_back((now, ms));
                }
                self.routes.insert(route, samples);
            }
        }
    }

    pub(crate) fn sparkline(
        &self,
        id: &str,
        path: &PeerConnection,
        ui: Ui,
    ) -> Option<impl IntoElement> {
        let samples = self.routes.get(&Route::new(id, path))?;
        let &(start, _) = samples.front()?;
        let &(end, _) = samples.back()?;
        let duration = end.duration_since(start).as_secs_f32();
        // A zero baseline and a small minimum range keep sub-millisecond jitter in proportion.
        let ceiling = samples.iter().map(|(_, ms)| *ms).max()?.max(10) as f32;
        let points: Vec<_> = samples
            .iter()
            .map(|&(at, ms)| {
                let x = if duration == 0. {
                    1.
                } else {
                    at.duration_since(start).as_secs_f32() / duration
                };
                (x, 1. - ms as f32 / ceiling)
            })
            .collect();
        Some(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let inset = px(2.);
                    let mut line = PathBuilder::stroke(px(1.4));
                    let mut tip = bounds.origin;
                    for (i, &(x, y)) in points.iter().enumerate() {
                        tip = bounds.origin
                            + point(
                                inset + (bounds.size.width - inset * 2.) * x,
                                inset + (bounds.size.height - inset * 2.) * y,
                            );
                        if i == 0 {
                            line.move_to(tip);
                        } else {
                            line.line_to(tip);
                        }
                    }
                    let color = rgb(ui.colors.muted_foreground);
                    if points.len() > 1
                        && let Ok(path) = line.build()
                    {
                        window.paint_path(path, color);
                    }
                    let marker =
                        Bounds::new(tip - point(inset, inset), size(inset * 2., inset * 2.));
                    window.paint_quad(fill(marker, color).corner_radii(inset));
                },
            )
            .w(px(80.))
            .min_w_0()
            .h(px(22.)),
        )
    }
}
