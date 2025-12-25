use crate::core::scheduler::ffmpeg_scheduler::is_stopping;
use crate::util::sch_waiter::SchWaiter;
use ffmpeg_sys_next::AV_NOPTS_VALUE;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) enum SchNode {
    Demux {
        waiter: Arc<SchWaiter>,
        task_exited: Arc<AtomicBool>,
    },
    Filter {
        inputs: Vec<Arc<SchNode>>,
        best_input: Arc<AtomicUsize>,
    },
    MuxStream {
        src: Arc<SchNode>,
        last_dts: Arc<AtomicI64>,
        source_finished: Arc<AtomicBool>,
    },
}

const SCHEDULE_TOLERANCE: i64 = 100 * 1000;
pub(crate) struct InputController {
    lock: Mutex<()>,
    demuxs: Vec<Arc<SchNode>>,
    mux_streams: Vec<Arc<SchNode>>,
}

impl InputController {
    pub(crate) fn new(demuxs: Vec<Arc<SchNode>>, mux_streams: Vec<Arc<SchNode>>) -> Self {
        assert!(
            demuxs
                .iter()
                .all(|node| matches!(**node, SchNode::Demux { .. })),
            "demuxs must contain only SchNode::Demux variants."
        );

        assert!(
            mux_streams
                .iter()
                .all(|node| matches!(**node, SchNode::MuxStream { .. })),
            "mux_streams must contain only SchNode::EncStream variants."
        );

        Self {
            lock: Mutex::new(()),
            demuxs,
            mux_streams,
        }
    }

    pub(crate) fn update_locked(&self, scheduler_status: &Arc<AtomicUsize>) {
        let _guard = self.lock.lock().unwrap();
        if is_stopping(scheduler_status.load(Ordering::Acquire)) {
            return;
        }

        let mut have_unchoked = false;

        let dts = self.trailing_dts();

        // initialize our internal state
        self.demuxs.iter().for_each(|demux| {
            let node = demux.as_ref();
            let SchNode::Demux { waiter, .. } = node else {
                unreachable!()
            };
            waiter.set_choked_prev(waiter.get_choked());
            waiter.set_choked_next(true);
        });

        // figure out the sources that are allowed to proceed
        for mux_stream in self.mux_streams.iter() {
            let node = mux_stream.as_ref();
            let SchNode::MuxStream {
                src,
                last_dts,
                source_finished,
            } = node
            else {
                unreachable!()
            };

            // unblock sources for output streams that are not finished
            // and not too far ahead of the trailing stream
            if source_finished.load(Ordering::Acquire) {
                continue;
            }
            let last_dts = last_dts.load(Ordering::Acquire);
            if dts == AV_NOPTS_VALUE && last_dts != AV_NOPTS_VALUE {
                continue;
            }
            if dts != AV_NOPTS_VALUE && last_dts - dts >= SCHEDULE_TOLERANCE {
                continue;
            }

            // resolve the source to unchoke
            Self::unchoke_for_stream(src);
            have_unchoked = true;
        }

        // make sure to unchoke at least one source, if still available
        if !have_unchoked {
            for demux in self.demuxs.iter() {
                let node = demux.as_ref();
                let SchNode::Demux {
                    waiter,
                    task_exited,
                } = node
                else {
                    unreachable!()
                };
                if !task_exited.load(Ordering::Acquire) {
                    waiter.set_choked_next(false);
                    // have_unchoked = true;
                    break;
                }
            }
        }

        for demux in self.demuxs.iter() {
            let node = demux.as_ref();
            let SchNode::Demux { waiter, .. } = node else {
                unreachable!()
            };
            let choked_next = waiter.get_choked_next();
            if waiter.get_choked_prev() != choked_next {
                waiter.set(choked_next);
            }
        }
    }

    fn unchoke_for_stream(mut src: &Arc<SchNode>) {
        loop {
            let node = src.as_ref();
            // fed directly by a demuxer (i.e. not through a filtergraph)
            if let SchNode::Demux { waiter, .. } = node {
                waiter.set_choked_next(false);
                return;
            }

            assert!(matches!(node, SchNode::Filter { .. }));

            let SchNode::Filter { inputs, best_input } = node else {
                unreachable!()
            };

            src = &inputs[best_input.load(Ordering::Acquire)];
        }
    }

    fn trailing_dts(&self) -> i64 {
        let min_dts = self
            .mux_streams
            .iter()
            .filter_map(|mux_stream| {
                let node = mux_stream.as_ref();
                let SchNode::MuxStream {
                    src: _,
                    last_dts,
                    source_finished,
                } = node
                else {
                    unreachable!()
                };
                if source_finished.load(Ordering::Acquire) {
                    None
                } else {
                    let last_dts = last_dts.load(Ordering::Acquire);
                    if last_dts == AV_NOPTS_VALUE {
                        None
                    } else {
                        Some(last_dts)
                    }
                }
            })
            .min();

        match min_dts {
            Some(min_dts) => min_dts,
            None => AV_NOPTS_VALUE,
        }
    }
}
