//! Live connection paths and recent traffic and latency samples for device details.
use crate::components::{Ui, heading, single_line, tooltip};
use gpui::{prelude::*, *};
use lyon_path::LineJoin;
use paxri_core::{
    StreamKind,
    connection::{ConnectionTraffic, PathKind, PeerConnection, format_bytes},
};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
// Coalesce incidental status events while allowing normal IPC jitter between one-second ticks.
const MIN_INTERVAL: Duration = Duration::from_millis(500);
const MAX_LINE_GAP: Duration = Duration::from_secs(5);
const CHART_STROKE: StrokeOptions = StrokeOptions::DEFAULT
    .with_line_width(1.4)
    .with_line_join(LineJoin::Round);

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
    // Keep the outgoing point until its connected successor reaches the left edge.
    while samples.front().is_some_and(|s| now - s.at > WINDOW + MAX_LINE_GAP) {
        samples.pop_front();
    }
    if samples.back().is_some_and(|s| now - s.at < MIN_INTERVAL) {
        return;
    }
    let rates = samples
        .back()
        .filter(|last| now - last.at <= WINDOW)
        .and_then(|last| path.traffic?.rates_since(last.traffic?, now - last.at));
    samples.push_back(Sample {
        at: now,
        traffic: path.traffic,
        upload: rates.map(|r| r.upload_bytes_per_second),
        download: rates.map(|r| r.download_bytes_per_second),
        latency: path.rtt_ms,
    });
}

#[derive(Default)]
pub struct ConnectionHistory {
    routes: HashMap<Route, VecDeque<Sample>>,
}

impl ConnectionHistory {
    pub fn record_peer(&mut self, id: &str, paths: &[PeerConnection]) {
        let now = Instant::now();
        let routes: Vec<_> = paths.iter().map(|path| Route::new(id, path)).collect();
        self.routes
            .retain(|route, _| route.peer_id != id || routes.contains(route));
        for (route, path) in routes.into_iter().zip(paths) {
            sample(self.routes.entry(route).or_default(), now, path);
        }
    }

    /// Replace the current set of routes, retaining samples only for connections still present.
    pub fn record<'a>(&mut self, peers: impl IntoIterator<Item = (&'a str, &'a [PeerConnection])>) {
        let now = Instant::now();
        let mut previous = std::mem::take(&mut self.routes);
        for (id, paths) in peers {
            for path in paths {
                let route = Route::new(id, path);
                let mut samples = previous.remove(&route).unwrap_or_default();
                sample(&mut samples, now, path);
                self.routes.insert(route, samples);
            }
        }
    }

    fn metrics(&self, id: &str, path: &PeerConnection, ui: Ui) -> Div {
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

fn chart_segments(
    visible: &[(Instant, u64)],
    single_reading: bool,
) -> impl Iterator<Item = &[(Instant, u64)]> {
    visible
        .chunk_by(|previous, next| next.0 - previous.0 <= MAX_LINE_GAP)
        .filter(move |segment| single_reading || segment.len() >= 2)
}

fn chart_x(at: Instant, now: Instant) -> f32 {
    // Keep offscreen coordinates so the content mask clips the original slope.
    (1.0 - now.duration_since(at).div_duration_f64(WINDOW)) as f32
}

fn chart_window(points: &[(Instant, u64)], now: Instant) -> &[(Instant, u64)] {
    let first = points.partition_point(|&(at, _)| now.duration_since(at) > WINDOW);
    let Some(&(at, _)) = points.get(first) else { return &[] };
    if first > 0 && now.duration_since(at) < WINDOW && at - points[first - 1].0 <= MAX_LINE_GAP {
        &points[first - 1..]
    } else {
        &points[first..]
    }
}

fn sparkline(samples: &VecDeque<Sample>, metric: Metric, ui: Ui) -> Option<impl IntoElement> {
    let points: Vec<_> = samples
        .iter()
        .filter_map(|s| Some((s.at, metric.value(s)?)))
        .collect();
    let floor = if matches!(metric, Metric::Latency) {
        10
    } else {
        1
    };
    if points.is_empty() {
        return None;
    }
    Some(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _cx| {
                if !bounds.intersects(&window.content_mask().bounds) {
                    return;
                }
                let now = Instant::now();
                let visible = chart_window(&points, now);
                let Some(maximum) = visible.iter().map(|&(_, value)| value).max() else { return };
                let mut segments = chart_segments(visible, points.len() == 1).peekable();
                if segments.peek().is_none() {
                    return;
                }
                #[cfg(target_os = "android")]
                let animate = !_cx.reduce_motion();
                #[cfg(not(target_os = "android"))]
                let animate = true;
                if animate {
                    window.request_animation_frame();
                }
                let ceiling = maximum.max(floor) as f32;
                let inset = px(2.);
                let color = rgb(ui.colors.muted_foreground);
                let position = |(at, value): (Instant, u64)| {
                    let x = chart_x(at, now);
                    let y = 1. - value as f32 / ceiling;
                    bounds.origin
                        + point(
                            (bounds.size.width - inset) * x,
                            inset + (bounds.size.height - inset * 2.) * y,
                        )
                };
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for segment in segments {
                        let first = position(segment[0]);
                        if segment.len() == 1 {
                            let center = point(first.x, bounds.center().y);
                            let diameter = px(CHART_STROKE.line_width);
                            let dot = Bounds::centered_at(center, size(diameter, diameter));
                            window.paint_quad(fill(dot, color).corner_radii(diameter / 2.));
                            continue;
                        }
                        let mut line =
                            PathBuilder::default().with_style(PathStyle::Stroke(CHART_STROKE));
                        line.move_to(first);
                        for &sample in &segment[1..] {
                            line.line_to(position(sample));
                        }
                        if let Ok(path) = line.build() {
                            window.paint_path(path, color);
                        }
                    }
                });
            },
        )
        .w(px(76.))
        .min_w_0()
        .h(px(22.)),
    )
}

#[cfg(test)]
mod tests {
    use super::{WINDOW, chart_segments, chart_window, chart_x, sample};
    use paxri_core::connection::{ConnectionTraffic, PathKind, PeerConnection};
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };

    #[test]
    fn chart_breaks_only_across_gaps_longer_than_five_seconds() {
        let start = Instant::now();
        let points = [0, 5_000, 10_001, 16_001, 17_001, 23_000]
            .map(|ms| (start + Duration::from_millis(ms), 42));
        assert_eq!(chart_segments(&points[..0], false).count(), 0);
        assert_eq!(chart_segments(&points[..1], true).collect::<Vec<_>>(), [&points[..1]]);
        assert_eq!(chart_segments(&points[..2], false).collect::<Vec<_>>(), [&points[..2]]);
        let segments: Vec<_> = chart_segments(&points, false).collect();
        assert_eq!(segments, [&points[..2], &points[3..5]]);
    }

    #[test]
    fn chart_scrolls_the_whole_segment_across_the_left_clip_edge() {
        let start = Instant::now();
        let next = start + Duration::from_secs(1);
        let last = next + Duration::from_secs(5);
        let points = [(start, 12), (next, 9), (last, 15)];
        let between = start + Duration::from_millis(500);
        assert_eq!(chart_x(start, start), 1.);
        assert!(chart_x(start, next) < chart_x(start, between) && chart_x(start, between) < 1.);
        assert_eq!(chart_x(next, next), 1.);
        let initial_width = chart_x(next, start + WINDOW);
        for elapsed_ms in [250, 500, 999] {
            let now = start + WINDOW + Duration::from_millis(elapsed_ms);
            assert_eq!(chart_window(&points, now), &points);
            let left = chart_x(start, now);
            let right = chart_x(next, now);
            assert!(left < 0. && right > 0.);
            assert!((right - left - initial_width).abs() < 1e-6);
            let fraction_at_edge = -left / (right - left);
            assert!((fraction_at_edge - elapsed_ms as f32 / 1000.).abs() < 1e-6);
        }
        assert_eq!(chart_window(&points, next + WINDOW), &points[1..]);
        assert_eq!(chart_x(next, next + WINDOW), 0.);
        assert_eq!(chart_window(&points, last + WINDOW - Duration::from_millis(1)), &points[1..]);

        let tail = chart_window(&points, last + WINDOW);
        assert_eq!(chart_segments(tail, false).count(), 0);
        assert!(chart_window(&points, last + WINDOW + Duration::from_millis(1)).is_empty());

        let after_gap = [(start, 12), (last, 15)];
        assert_eq!(chart_window(&after_gap, between + WINDOW), &after_gap[1..]);
    }

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
        sample(&mut samples, start + Duration::from_secs(1), &path);
        assert_eq!(samples.len(), 2);
        assert_eq!(
            (samples[1].upload, samples[1].download),
            (Some(3072), Some(1536))
        );
        sample(&mut samples, start + Duration::from_millis(1990), &path);
        assert_eq!(samples[2].upload, Some(0));
        sample(&mut samples, start + Duration::from_secs(70), &path);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].download, None);
    }
}

fn connection_host(address: &str) -> String {
    let Ok(url) = url::Url::parse(address) else {
        return address.to_owned();
    };
    let host = url.host_str().unwrap_or(address);
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

impl ConnectionHistory {
    pub fn connection_section(&self, peer_id: &str, paths: Option<&[PeerConnection]>, presence: bool, ui: Ui) -> Div {
        let mut list = ui.section().child(heading("Connection"));
        let Some(paths) = paths.filter(|paths| !paths.is_empty()) else {
            return list.child(ui.muted(paxri_core::connection::peer_state(paths, presence)));
        };
        for (i, path) in paths.iter().enumerate() {
            let streams = path
                .stream_labels_matching(|kind| kind != StreamKind::Terminal)
                .collect::<Vec<_>>()
                .join(" · ");
            let route = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .child(path.path.label()),
                )
                .when_some(path.address.as_ref(), |r, address| {
                    r.child(
                        single_line(ui.muted(connection_host(address)))
                            .flex_1()
                            .id(("path-address", i))
                            .tooltip(tooltip(address.clone().into())),
                    )
                });
            let details = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .child(route)
                .child(self.metrics(peer_id, path, ui))
                .when(!streams.is_empty(), |d| {
                    d.child(ui.muted(streams).text_xs())
                });
            list = list.when(i > 0, |d| d.child(ui.separator())).child(details);
        }
        list
    }
}

pub fn presence_detail(state: &str, now: i64, changed: Option<i64>) -> String {
    let Some(changed) = changed else {
        return state.to_owned();
    };
    let minutes = now.saturating_sub(changed).max(0) / 60_000;
    let age: String = match minutes {
        0 => "just now".into(),
        1..60 => format!("{minutes}m ago"),
        60..1440 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    };
    format!("{state} · {age}")
}
