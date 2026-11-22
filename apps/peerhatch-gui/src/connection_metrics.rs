//! Short per-connection histories sampled by the existing gateway status refresh.
use crate::components::{Ui, tooltip};
use gpui::{prelude::*, *};
use peerhatch_control::{ConnectionTraffic, PathKind, PeerConnection, Status, format_bytes};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const MIN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Hash, PartialEq, Eq)]
struct Route {
    peer_id: String,
    protocol: String,
    kind: PathKind,
    address: Option<String>,
    connected_at_ms: Option<i64>,
}

impl Route {
    fn new(id: &str, path: &PeerConnection) -> Self {
        Self {
            peer_id: id.into(),
            protocol: path.protocol.clone(),
            kind: path.path,
            address: path.address.clone(),
            connected_at_ms: path.traffic.map(|t| t.connected_at_ms),
        }
    }
}

struct Sample {
    at: Instant,
    traffic: Option<ConnectionTraffic>,
    upload: Option<u64>,
    download: Option<u64>,
    latency: Option<u64>,
}

fn sample(samples: &mut VecDeque<Sample>, now: Instant, path: &PeerConnection) {
    while samples.front().is_some_and(|s| now - s.at > WINDOW) {
        samples.pop_front();
    }
    if samples.back().is_some_and(|s| now - s.at < MIN_INTERVAL) {
        return;
    }
    let rates = samples.back().and_then(|last| {
        path.traffic?.rates_since(last.traffic?, now - last.at)
    });
    samples.push_back(Sample {
        at: now,
        traffic: path.traffic,
        upload: rates.map(|r| r.upload_bytes_per_second),
        download: rates.map(|r| r.download_bytes_per_second),
        latency: path.rtt_ms,
    });
}

#[derive(Default)]
pub(crate) struct ConnectionHistory {
    routes: HashMap<Route, VecDeque<Sample>>,
}

impl ConnectionHistory {
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
                let route = Route::new(id, path);
                let mut samples = previous.remove(&route).unwrap_or_default();
                sample(&mut samples, now, path);
                self.routes.insert(route, samples);
            }
        }
    }

    pub(crate) fn metrics(&self, id: &str, path: &PeerConnection, ui: Ui) -> Div {
        let history = self.routes.get(&Route::new(id, path));
        let last = history.and_then(|h| h.back());
        let mut row = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_2();
        for (metric, rate, total) in [
            (
                Metric::Upload,
                last.and_then(|s| s.upload),
                path.traffic.map(|t| t.sent_bytes),
            ),
            (
                Metric::Download,
                last.and_then(|s| s.download),
                path.traffic.map(|t| t.received_bytes),
            ),
            (Metric::Latency, path.rtt_ms, None),
        ] {
            let value = match (metric, rate) {
                (_, None) => "—".to_owned(),
                (Metric::Latency, Some(0)) => "<1 ms".to_owned(),
                (Metric::Latency, Some(ms)) => format!("{ms} ms"),
                (_, Some(bytes)) => format!("{}/s", format_bytes(bytes)),
            };
            let label = metric.label();
            let title = ui.muted(label).text_xs();
            let hint = match total {
                Some(bytes) => format!(
                    "{} {} on this connection",
                    format_bytes(bytes),
                    if matches!(metric, Metric::Upload) {
                        "sent"
                    } else {
                        "received"
                    }
                ),
                None if matches!(metric, Metric::Latency) => "Round-trip time".into(),
                None => "Traffic statistics unavailable".into(),
            };
            row = row.child(
                div()
                    .min_w(px(145.))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .min_w(px(58.))
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .whitespace_nowrap()
                                    .flex_none()
                                    .child(value),
                            )
                            .children(history.and_then(|h| sparkline(h, metric, ui))),
                    )
                    .id(SharedString::from(format!(
                        "{id}-{}-{label}",
                        path.address.as_deref().unwrap_or(&path.protocol)
                    )))
                    .tooltip(tooltip(hint.into())),
            );
        }
        row
    }
}

#[derive(Clone, Copy)]
enum Metric {
    Upload,
    Download,
    Latency,
}

impl Metric {
    fn label(self) -> &'static str {
        match self {
            Self::Upload => "Upload",
            Self::Download => "Download",
            Self::Latency => "Latency",
        }
    }
    fn value(self, sample: &Sample) -> Option<u64> {
        match self {
            Self::Upload => sample.upload,
            Self::Download => sample.download,
            Self::Latency => sample.latency,
        }
    }
}

fn sparkline(samples: &VecDeque<Sample>, metric: Metric, ui: Ui) -> Option<impl IntoElement> {
    let start = samples.front()?.at;
    let span = (samples.back()?.at - start).as_secs_f32();
    let ceiling = samples.iter().filter_map(|s| metric.value(s)).max()?.max(
        if matches!(metric, Metric::Latency) {
            10
        } else {
            1
        },
    ) as f32;
    let points: Vec<_> = samples
        .iter()
        .filter_map(|s| {
            Some((
                if span > 0. {
                    (s.at - start).as_secs_f32() / span
                } else {
                    1.
                },
                1. - metric.value(s)? as f32 / ceiling,
            ))
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
                let marker = Bounds::new(tip - point(inset, inset), size(inset * 2., inset * 2.));
                window.paint_quad(fill(marker, color).corner_radii(inset));
            },
        )
        .w(px(76.))
        .min_w_0()
        .h(px(22.)),
    )
}

#[cfg(test)]
mod tests {
    use super::sample;
    use peerhatch_control::{ConnectionTraffic, PathKind, PeerConnection};
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };

    #[test]
    fn rates_use_elapsed_time_and_expire_with_the_history() {
        let mut path = PeerConnection {
            protocol: "test".into(),
            streams: Default::default(),
            path: PathKind::Direct,
            address: None,
            rtt_ms: Some(4),
            traffic: Some(ConnectionTraffic {
                connected_at_ms: 1,
                sent_bytes: 100,
                received_bytes: 200,
            }),
        };
        let start = Instant::now();
        let mut samples = VecDeque::new();
        sample(&mut samples, start, &path);
        assert_eq!(samples[0].upload, None);
        let totals = path.traffic.as_mut().unwrap();
        totals.sent_bytes += 3072;
        totals.received_bytes += 1536;
        sample(&mut samples, start + Duration::from_millis(150), &path);
        sample(&mut samples, start + Duration::from_secs(3), &path);
        assert_eq!(samples.len(), 2);
        assert_eq!(
            (samples[1].upload, samples[1].download),
            (Some(1024), Some(512))
        );
        sample(&mut samples, start + Duration::from_secs(6), &path);
        assert_eq!(samples[2].upload, Some(0));
        sample(&mut samples, start + Duration::from_secs(70), &path);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].download, None);
    }
}
