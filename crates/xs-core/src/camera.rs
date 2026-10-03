//! A single owner releases the webcam on disable and never reopens it for late frames.
use crate::session::CameraRequest;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use xs_proto::{flags, CameraState, CameraStatus};

pub(crate) trait CameraOutput: Send {
    fn push(&mut self, frame: &xs_transport::Frame) -> Result<(), String>;
    fn failure(&self) -> Option<String>;
}
impl CameraOutput for xs_camera::V4l2Writer {
    fn push(&mut self, frame: &xs_transport::Frame) -> Result<(), String> {
        self.push(
            &frame.payload,
            frame.header.pts_us,
            frame.header.flags & flags::CODEC_CONFIG != 0,
        )
        .map_err(|error| error.to_string())
    }
    fn failure(&self) -> Option<String> {
        self.failure()
    }
}

pub(crate) async fn run<S: CameraOutput>(
    mut requests: watch::Receiver<CameraRequest>,
    status: watch::Sender<CameraStatus>,
    mut incoming: mpsc::Receiver<xs_transport::frame::Result<xs_transport::Frame>>,
    mut open: impl FnMut() -> Result<S, String>,
) {
    let mut channel_closed = false;
    let mut writer: Option<S> = None;
    let mut failed = false;
    let mut waiting_keyframe = true;
    let mut health = tokio::time::interval(Duration::from_millis(250));
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = requests.changed() => {
                if changed.is_err() { break; }
                let request = *requests.borrow_and_update();
                writer = None; failed = false; waiting_keyframe = true;
                // Already-buffered frames belonged to the old camera request.
                while let Ok(frame) = incoming.try_recv() {
                    if frame.is_err() { channel_closed = true; }
                }
                if request.enabled {
                    let _ = status.send(CameraStatus {
                        state: if channel_closed { CameraState::Failed } else { CameraState::Pending },
                        message: if channel_closed { "Camera channel closed. Reconnect the display." } else { "Starting camera…" }.into(),
                    });
                }
            }
            _ = health.tick() => {
                if let Some(message) = writer.as_ref().and_then(|writer| writer.failure()) {
                    writer = None; failed = true;
                    let _ = status.send(CameraStatus { state: CameraState::Failed, message });
                }
            }
            frame = incoming.recv(), if !channel_closed => {
                let Some(frame) = frame else {
                    channel_closed = true;
                    if requests.borrow().enabled {
                        writer = None; failed = true;
                        let _ = status.send(CameraStatus { state: CameraState::Failed, message: "Camera channel closed. Reconnect the display.".into() });
                    }
                    continue;
                };
                if frame.is_err() { channel_closed = true; }
                if !requests.borrow().enabled || failed { continue; }
                let result = match frame {
                    Ok(frame) => {
                        let config = frame.header.flags & flags::CODEC_CONFIG != 0;
                        let keyframe = frame.header.flags & flags::KEYFRAME != 0;
                        if waiting_keyframe && !config && !keyframe { continue; }
                        if writer.is_none() {
                            match open() {
                                Ok(opened) => writer = Some(opened),
                                Err(message) => {
                                    failed = true;
                                    let _ = status.send(CameraStatus { state: CameraState::Failed, message });
                                    continue;
                                }
                            }
                        }
                        let result = writer.as_mut().unwrap().push(&frame);
                        if result.is_ok() && keyframe { waiting_keyframe = false; }
                        if result.is_ok() && !config {
                            status.send_if_modified(|current| {
                                if current.state != CameraState::Pending { return false; }
                                *current = CameraStatus { state: CameraState::Running, message: "Tablet camera is running".into() }; true
                            });
                        }
                        result
                    }
                    Err(error) => Err(error.to_string()),
                };
                if let Err(message) = result {
                    writer = None; failed = true;
                    let _ = status.send(CameraStatus { state: CameraState::Failed, message });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Fake {
        dropped: Arc<AtomicUsize>,
        pushed: Arc<AtomicUsize>,
        failure: bool,
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    impl CameraOutput for Fake {
        fn push(&mut self, _: &xs_transport::Frame) -> Result<(), String> {
            self.pushed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn failure(&self) -> Option<String> {
            self.failure.then(|| "Sink stopped".into())
        }
    }
    fn frame(flags: u16) -> xs_transport::frame::Result<xs_transport::Frame> {
        Ok(xs_transport::Frame {
            header: xs_proto::Header {
                channel: xs_proto::Channel::CameraUp,
                kind: 0,
                flags,
                len: 1,
                pts_us: 0,
            },
            payload: bytes::Bytes::from_static(&[1]),
        })
    }
    async fn wait_for(predicate: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !predicate() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn disabling_drops_the_writer_and_late_frames_do_not_reopen_it() {
        let (requests, request_rx) = watch::channel(CameraRequest {
            enabled: true,
            revision: 0,
        });
        let (status, _) = watch::channel(CameraStatus {
            state: CameraState::Pending,
            message: String::new(),
        });
        let (frames, rx) = mpsc::channel(4);
        let dropped = Arc::new(AtomicUsize::new(0));
        let pushed = Arc::new(AtomicUsize::new(0));
        let drops = dropped.clone();
        let pushes = pushed.clone();
        let task = tokio::spawn(run(request_rx, status, rx, move || {
            Ok(Fake {
                dropped: drops.clone(),
                pushed: pushes.clone(),
                failure: false,
            })
        }));
        // A delta before an initial keyframe must not open the device.
        frames.send(frame(0)).await.unwrap();
        frames.send(frame(flags::KEYFRAME)).await.unwrap();
        wait_for(|| pushed.load(Ordering::Relaxed) == 1).await;
        requests
            .send(CameraRequest {
                enabled: false,
                revision: 1,
            })
            .unwrap();
        frames.send(frame(flags::KEYFRAME)).await.unwrap();
        wait_for(|| dropped.load(Ordering::Relaxed) == 1).await;
        assert_eq!(pushed.load(Ordering::Relaxed), 1);
        drop(requests);
        task.await.unwrap();
    }
    #[tokio::test]
    async fn asynchronous_sink_failure_drops_writer_and_stays_failed() {
        let (requests, request_rx) = watch::channel(CameraRequest {
            enabled: true,
            revision: 0,
        });
        let (status, status_rx) = watch::channel(CameraStatus {
            state: CameraState::Pending,
            message: String::new(),
        });
        let (frames, rx) = mpsc::channel(4);
        let dropped = Arc::new(AtomicUsize::new(0));
        let pushed = Arc::new(AtomicUsize::new(0));
        let drops = dropped.clone();
        let pushes = pushed.clone();
        let task = tokio::spawn(run(request_rx, status, rx, move || {
            Ok(Fake {
                dropped: drops.clone(),
                pushed: pushes.clone(),
                failure: true,
            })
        }));
        frames.send(frame(flags::KEYFRAME)).await.unwrap();
        wait_for(|| status_rx.borrow().state == CameraState::Failed).await;
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        frames.send(frame(flags::KEYFRAME)).await.unwrap();
        drop(requests);
        task.await.unwrap();
        assert_eq!(pushed.load(Ordering::Relaxed), 1);
    }
}
