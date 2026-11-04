//! A bounded raw-frame mailbox, paced by actual dispatch time. Idle time never
//! earns credit for a later burst. Keep the latest pending image, including the
//! final update before the desktop goes idle; never manufacture duplicate frames.
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct State<T> {
    pending: Option<T>,
    stopped: bool,
}

pub(crate) struct FramePacer<T> {
    shared: Arc<(Mutex<State<T>>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> FramePacer<T> {
    #[cfg(test)]
    pub fn new(fps: u32, mut deliver: impl FnMut(T) + Send + 'static) -> std::io::Result<Self> {
        let shared = Arc::new((
            Mutex::new(State {
                pending: None,
                stopped: false,
            }),
            Condvar::new(),
        ));
        let receiver = Arc::clone(&shared);
        let period = Duration::from_nanos(1_000_000_000u64.div_ceil(u64::from(fps.max(1))));
        let worker = thread::Builder::new()
            .name("xs-frame-pacer".into())
            .spawn(move || {
                let (lock, changed) = &*receiver;
                let mut next = Instant::now();
                loop {
                    let mut state = lock.lock().expect("frame pacer lock");
                    while !state.stopped && state.pending.is_none() {
                        state = changed.wait(state).expect("frame pacer wait");
                    }
                    if state.stopped {
                        break;
                    }
                    if let Some(wait) = next.checked_duration_since(Instant::now()) {
                        let (waiting, _) = changed
                            .wait_timeout(state, wait)
                            .expect("frame pacer deadline");
                        drop(waiting);
                        continue;
                    }
                    let frame = state.pending.take().expect("pending frame");
                    drop(state);
                    // Reset on every actual dispatch. No accumulated idle credit or
                    // catch-up bursts after a delayed encoder/transport operation.
                    next = Instant::now() + period;
                    deliver(frame);
                }
            })?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn submit(&self, frame: T) {
        let (lock, changed) = &*self.shared;
        let mut state = lock.lock().expect("frame pacer lock");
        if state.stopped {
            return;
        }
        let was_empty = state.pending.replace(frame).is_none();
        // Replacing an already pending frame does not wake the timed wait.
        if was_empty {
            changed.notify_one();
        }
    }
}

impl<T: Clone + Send + 'static> FramePacer<T> {
    /// Like [`Self::new`], plus a single static-settle refinement re-push after
    /// `settle_delay` of silence so the encoder can spend a full frame budget
    /// sharpening static text once motion stops.
    ///
    /// Frames may also leave up to a quarter period early. Capture runs at the
    /// same nominal rate but its own phase; with a hard deadline, a frame that
    /// lands just before it waits a whole period, and measured waits averaged
    /// half a frame. The schedule advances by whole periods, so the long-run
    /// rate is still bounded, and idle time earns at most the slack.
    pub fn new_with_settle(
        fps: u32,
        settle_delay: Duration,
        mut deliver: impl FnMut(T) + Send + 'static,
    ) -> std::io::Result<Self> {
        let shared = Arc::new((
            Mutex::new(State::<T> {
                pending: None,
                stopped: false,
            }),
            Condvar::new(),
        ));
        let receiver = Arc::clone(&shared);
        let period = Duration::from_nanos(1_000_000_000u64.div_ceil(u64::from(fps.max(1))));
        let slack = period / 4;
        let worker = thread::Builder::new()
            .name("xs-frame-pacer".into())
            .spawn(move || {
                let (lock, changed) = &*receiver;
                let mut next = Instant::now();
                let mut settle_candidate: Option<T> = None;
                loop {
                    let mut state = lock.lock().expect("frame pacer lock");
                    while !state.stopped && state.pending.is_none() {
                        if let Some(ref candidate) = settle_candidate {
                            let (waiting, timeout) = changed
                                .wait_timeout(state, settle_delay)
                                .expect("frame pacer settle wait");
                            state = waiting;
                            if !state.stopped && state.pending.is_none() && timeout.timed_out() {
                                let settled = candidate.clone();
                                settle_candidate = None;
                                drop(state);
                                next = Instant::now() + period;
                                deliver(settled);
                                state = lock.lock().expect("frame pacer lock");
                            }
                        } else {
                            state = changed.wait(state).expect("frame pacer wait");
                        }
                    }
                    if state.stopped {
                        break;
                    }
                    let now = Instant::now();
                    if let Some(wait) = next
                        .checked_duration_since(now)
                        .filter(|wait| *wait > slack)
                    {
                        let (waiting, _) = changed
                            .wait_timeout(state, wait - slack)
                            .expect("frame pacer deadline");
                        drop(waiting);
                        continue;
                    }
                    let frame = state.pending.take().expect("pending frame");
                    settle_candidate = Some(frame.clone());
                    drop(state);
                    next = (next + period).max(now + period - slack);
                    deliver(frame);
                }
            })?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
}

impl<T> Drop for FramePacer<T> {
    fn drop(&mut self) {
        let (lock, changed) = &*self.shared;
        let mut state = lock.lock().expect("frame pacer lock");
        state.stopped = true;
        state.pending = None;
        changed.notify_one();
        drop(state);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn idle_has_no_duplicates_or_burst_credit_and_the_final_image_is_delivered() {
        let (tx, rx) = mpsc::channel();
        let pacer = FramePacer::new(60, move |frame| {
            tx.send((frame, Instant::now())).unwrap();
        })
        .unwrap();
        pacer.submit(0);
        let (_, first) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        // Longer than several frame periods. The next image must wake directly,
        // without permitting the following 1,000 images to burst through.
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        let resumed = Instant::now();
        pacer.submit(1);
        let (value, second) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(value, 1);
        assert!(second.duration_since(resumed) < Duration::from_millis(50));
        assert!(second.duration_since(first) >= Duration::from_millis(100));
        for frame in 2..=1001 {
            pacer.submit(frame);
        }
        let (last, third) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(last, 1001, "the final image must survive coalescing");
        assert!(
            third.duration_since(second) >= Duration::from_nanos(1_000_000_000u64.div_ceil(60))
        );
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(pacer);
    }

    #[test]
    fn a_frame_slightly_ahead_of_schedule_is_not_held_for_a_period() {
        let (tx, rx) = mpsc::channel();
        let pacer = FramePacer::new_with_settle(50, Duration::from_secs(10), move |frame| {
            tx.send((frame, Instant::now())).unwrap();
        })
        .unwrap();
        pacer.submit(1u32);
        let (_, first) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        // 20 ms period: arrive 3 ms early, inside the 5 ms slack.
        thread::sleep(Duration::from_millis(17).saturating_sub(first.elapsed()));
        let submitted = Instant::now();
        pacer.submit(2u32);
        let (frame, delivered) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(frame, 2);
        assert!(delivered.duration_since(submitted) < Duration::from_millis(2));
        // Far ahead of schedule is still held back.
        let submitted = Instant::now();
        pacer.submit(3u32);
        let (_, delivered) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(delivered.duration_since(submitted) >= Duration::from_millis(10));
        drop(pacer);
    }

    #[test]
    fn settle_pass_delivers_one_refinement_frame_after_motion_stops() {
        let (tx, rx) = mpsc::channel();
        let pacer = FramePacer::new_with_settle(60, Duration::from_millis(40), move |frame| {
            tx.send(frame).unwrap();
        })
        .unwrap();
        pacer.submit(42u32);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 42);
        // Settle refinement frame arrives once after ~40ms of silence.
        assert_eq!(rx.recv_timeout(Duration::from_millis(200)).unwrap(), 42);
        // And does not repeat while still idle.
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(pacer);
    }

    #[test]
    fn shutdown_wakes_an_idle_worker_and_cancels_a_pending_deadline() {
        let idle = FramePacer::<u32>::new(1, |_| {}).unwrap();
        let start = Instant::now();
        drop(idle);
        assert!(start.elapsed() < Duration::from_millis(100));
        let (tx, rx) = mpsc::channel();
        let active = FramePacer::new(1, move |frame| {
            let _ = tx.send(frame);
        })
        .unwrap();
        active.submit(1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        active.submit(2);
        let start = Instant::now();
        drop(active);
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(rx.try_recv().is_err());
    }
}
