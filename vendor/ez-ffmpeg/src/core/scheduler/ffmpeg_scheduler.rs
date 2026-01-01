use crate::core::context::demuxer::Demuxer;
use crate::core::context::ffmpeg_context::FfmpegContext;
use crate::core::context::muxer::Muxer;
use crate::core::context::obj_pool::ObjPool;
use crate::core::context::{in_fmt_ctx_free, out_fmt_ctx_free};
use crate::core::scheduler::dec_task::dec_init;
use crate::core::scheduler::demux_task::demux_init;
use crate::core::scheduler::enc_task::enc_init;
use crate::core::scheduler::filter_task::filter_graph_init;
use crate::core::scheduler::frame_filter_pipeline::{input_pipeline_init, output_pipeline_init};
use crate::core::scheduler::input_controller::InputController;
use crate::core::scheduler::mux_task::{mux_init, ready_to_init_mux};
use crate::error::{AllocFrameError, AllocPacketError};
use crate::util::thread_synchronizer::ThreadSynchronizer;
use ffmpeg_next::packet::{Mut, Ref};
use ffmpeg_next::{Frame, Packet};
use ffmpeg_sys_next::{av_frame_alloc, av_frame_unref, av_packet_unref};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Initialization;
pub struct Running;
pub struct Paused;
pub struct Ended;

// Internal wrapper for Running state to enable Drop implementation.
//
// This guard ensures that when a FfmpegScheduler<Running> (or Paused) is dropped,
// all worker threads are properly terminated and output files are finalized.
//
// IMPORTANT: Drop only runs when the scheduler goes out of scope normally.
// If the process exits abruptly (e.g., via std::process::exit() or panic in main),
// Drop will NOT run and files may be corrupted.
//
// The guard is passed through state transitions (Running -> Paused -> Running)
// via into_state() to maintain Drop protection across all active states.
struct RunningGuard {
    status: Arc<AtomicUsize>,
    thread_sync: ThreadSynchronizer,
}

impl Drop for RunningGuard {
    /// Ensures graceful shutdown when scheduler is dropped.
    /// Blocks until all threads complete to prevent data corruption.
    fn drop(&mut self) {
        // Always ensure STATUS_END is set
        if !is_stopping(self.status.load(Ordering::Acquire)) {
            log::debug!("Drop called, setting STATUS_END");
            self.status.store(STATUS_END, Ordering::Release);
        }

        // Always wait for all threads to complete
        // This is safe to call multiple times - if threads are already done (counter==0),
        // wait_for_all_threads() returns immediately without blocking.
        // This ensures cleanup works correctly after both abort() and stop().
        log::debug!("Drop waiting for all threads to finish");
        self.thread_sync.wait_for_all_threads();
    }
}

pub struct FfmpegScheduler<S> {
    ffmpeg_context: FfmpegContext,
    status: Arc<AtomicUsize>,
    thread_sync: ThreadSynchronizer,
    result: Arc<Mutex<Option<crate::error::Result<()>>>>,
    state: PhantomData<S>,
    // Guard for Running state - Some only when in Running state
    _guard: Option<RunningGuard>,
}
unsafe impl<S> Send for FfmpegScheduler<S> {}
unsafe impl<S> Sync for FfmpegScheduler<S> {}

pub(crate) const STATUS_INIT: usize = 0;
pub(crate) const STATUS_RUN: usize = 1;
pub(crate) const STATUS_PAUSE: usize = 2;
pub(crate) const STATUS_ABORT: usize = 3;
pub(crate) const STATUS_END: usize = 4;

/// Checks if scheduler is in a stopping state (abort or normal end)
pub(crate) fn is_stopping(status: usize) -> bool {
    status == STATUS_END || status == STATUS_ABORT
}

impl<S: 'static> FfmpegScheduler<S> {
    /// Determines if this scheduler’s **state type** (`S`) matches a specified
    /// type `T`. Primarily used internally to check state transitions at runtime.
    ///
    /// # Returns
    /// - `true` if `S` and `T` are the same type.
    /// - `false` otherwise.
    #[allow(dead_code)]
    fn is_state<T: 'static>(&self) -> bool {
        std::any::TypeId::of::<S>() == std::any::TypeId::of::<T>()
    }

    /// Consumes this scheduler and **transitions** it to a new state type `T`.
    /// Used internally for changing from `Initialization` to `Running`, etc.
    ///
    /// # Returns
    /// A new `FfmpegScheduler<T>` retaining the same internal data,
    /// but typed with the new state `T`.
    fn into_state<T>(self) -> FfmpegScheduler<T> {
        FfmpegScheduler {
            ffmpeg_context: self.ffmpeg_context,
            status: self.status,
            thread_sync: self.thread_sync,
            result: self.result,
            state: Default::default(),
            _guard: self._guard,  // Pass guard to maintain Drop protection across state transitions
        }
    }

    /// Internal method to signal the scheduler to stop.
    /// This sets the scheduler's status to "END" and signals all worker threads
    /// to finish up and exit gracefully.
    ///
    /// # Note
    /// This method only sets the signal flag. It does not wait for threads to complete.
    /// Calling code is responsible for deciding whether to wait or not.
    fn signal_stop(&self) {
        self.status.store(STATUS_END, Ordering::Release);
    }

    /// Checks whether the FFmpeg job has ended. The job can end because it
    /// completed successfully, encountered an error, or was manually aborted.
    ///
    /// # Returns
    /// - `true` if the FFmpeg job is in the ended state.
    /// - `false` otherwise.
    pub fn is_ended(&self) -> bool {
        is_stopping(self.status.load(Ordering::Acquire))
    }
}

impl FfmpegScheduler<Initialization> {

    /// Creates a new [`FfmpegScheduler`] in the **initialization** state from the given [`FfmpegContext`].
    /// This is the first step to orchestrating an FFmpeg job: you prepare your
    /// inputs, outputs, and filters using [`FfmpegContext`], then pass it here.
    ///
    /// # Example
    /// ```rust
    /// let context = FfmpegContext::builder()
    ///     .input("input.mp4")
    ///     .output("output.mkv")
    ///     .build()
    ///     .unwrap();
    ///
    /// let scheduler = FfmpegScheduler::new(context);
    /// // At this point, no actual FFmpeg threads are running; call `start()` to begin.
    /// ```
    pub fn new(ffmpeg_context: FfmpegContext) -> FfmpegScheduler<Initialization> {
        FfmpegScheduler {
            ffmpeg_context,
            state: Default::default(),
            thread_sync: ThreadSynchronizer::new(),
            status: Arc::new(AtomicUsize::new(STATUS_INIT)),
            result: Arc::new(Mutex::new(None)),
            _guard: None,
        }
    }

    /// Initializes all FFmpeg components (demuxers, encoders, filters, muxers)
    /// and transitions the scheduler from **Initialization** to **Running**.
    ///
    /// If any part of the setup fails (e.g., an invalid codec, a missing file),
    /// this method returns an error and cleans up resources.
    ///
    /// # Returns
    /// - `Ok(FfmpegScheduler<Running>)` if initialization succeeded.
    /// - `Err(...)` if an error occurred during FFmpeg setup.
    ///
    /// # Example
    /// ```rust
    /// let scheduler = FfmpegScheduler::new(ffmpeg_context);
    /// let running_scheduler = scheduler.start().expect("Failed to start FFmpeg");
    /// // Now it's in Running state, you can wait or pause/abort, etc.
    /// ```
    pub fn start(mut self) -> crate::error::Result<FfmpegScheduler<Running>> {
        let packet_pool = ObjPool::new(64, new_packet, unref_packet, packet_is_null)?;
        let frame_pool = ObjPool::new(64, new_frame, unref_frame, frame_is_null)?;
        let scheduler_status = self.status.clone();
        scheduler_status.store(STATUS_RUN, Ordering::Release);
        let thread_sync = self.thread_sync.clone();
        let scheduler_result = self.result.clone();

        let demux_nodes = self.ffmpeg_context.demuxs.iter().map(|demux| demux.node.clone()).collect::<Vec<_>>();
        let mux_stream_nodes = self.ffmpeg_context.muxs.iter().flat_map(|mux| mux.mux_stream_nodes.clone()).map(|mux_stream| mux_stream.clone()).collect::<Vec<_>>();
        let input_controller = InputController::new(demux_nodes, mux_stream_nodes);
        let input_controller = Arc::new(input_controller);

        // Muxer
        for (mux_idx, mux) in self.ffmpeg_context.muxs.iter_mut().enumerate() {
            // Even if it's not ready here, it's going to be ready later, so it locks first
            thread_sync.thread_start();
            if mux.is_ready() {
                if let Err(e) = mux_init(
                    mux_idx,
                    mux,
                    packet_pool.clone(),
                    input_controller.clone(),
                    mux.mux_stream_nodes.clone(),
                    scheduler_status.clone(),
                    thread_sync.clone(),
                    scheduler_result.clone(),
                ) {
                    Self::cleanup(&scheduler_status, &self.ffmpeg_context);
                    return Err(e);
                }
            }
        }

        // Output frame filter pipeline
        let ffmpeg_context = &mut self.ffmpeg_context;
        for (mux_idx, mux) in ffmpeg_context.muxs.iter_mut().enumerate() {
            if let Some(frame_pipelines) = mux.frame_pipelines.take() {
                for frame_pipeline in frame_pipelines {
                    if let Err(e) = output_pipeline_init(
                        mux_idx,
                        frame_pipeline,
                        mux.get_streams_mut(),
                        frame_pool.clone(),
                        scheduler_status.clone(),
                        scheduler_result.clone(),
                    ) {
                        Self::cleanup(&scheduler_status, ffmpeg_context);
                        return Err(e);
                    }
                }
            }
        }

        // Encoder
        let ffmpeg_context = &mut self.ffmpeg_context;
        for (mux_idx, mux) in &mut ffmpeg_context.muxs.iter_mut().enumerate() {
            let ready_sender = ready_to_init_mux(
                mux_idx,
                mux,
                packet_pool.clone(),
                input_controller.clone(),
                scheduler_status.clone(),
                thread_sync.clone(),
                scheduler_result.clone(),
            );

            for enc_stream in mux.take_streams_mut() {
                if let Err(e) = enc_init(
                    mux_idx,
                    enc_stream,
                    ready_sender.clone(),
                    mux.start_time_us,
                    mux.recording_time_us,
                    mux.bits_per_raw_sample,
                    mux.max_video_frames,
                    mux.max_audio_frames,
                    mux.max_subtitle_frames,
                    &mux.video_codec_opts,
                    &mux.audio_codec_opts,
                    &mux.subtitle_codec_opts,
                    mux.oformat_flags,
                    frame_pool.clone(),
                    packet_pool.clone(),
                    scheduler_status.clone(),
                    scheduler_result.clone(),
                ) {
                    Self::cleanup(&scheduler_status, ffmpeg_context);
                    return Err(e);
                }
            }
        }

        // Filter graph
        let ffmpeg_context = &mut self.ffmpeg_context;
        for (i, filter_graph) in ffmpeg_context.filter_graphs.iter_mut().enumerate() {
            if let Err(e) = filter_graph_init(
                i,
                filter_graph,
                frame_pool.clone(),
                input_controller.clone(),
                filter_graph.node.clone(),
                scheduler_status.clone(),
                scheduler_result.clone(),
            ) {
                Self::cleanup(&scheduler_status, ffmpeg_context);
                return Err(e);
            }
        }

        // Input frame filter pipeline
        let ffmpeg_context = &mut self.ffmpeg_context;
        for (demux_idx, demux) in ffmpeg_context.demuxs.iter_mut().enumerate() {
            if let Some(frame_pipelines) = demux.frame_pipelines.take() {
                for frame_pipeline in frame_pipelines {
                    if let Err(e) = input_pipeline_init(
                        demux_idx,
                        frame_pipeline,
                        demux.get_streams_mut(),
                        frame_pool.clone(),
                        scheduler_status.clone(),
                        scheduler_result.clone(),
                    ) {
                        Self::cleanup(&scheduler_status, ffmpeg_context);
                        return Err(e);
                    }
                }
            }
        }

        // Decoder
        for (demux_idx, demux) in &mut ffmpeg_context.demuxs.iter_mut().enumerate() {
            let exit_on_error = demux.exit_on_error;

            for dec_stream in demux.get_streams_mut() {
                if let Err(e) = dec_init(
                    demux_idx,
                    dec_stream,
                    exit_on_error,
                    frame_pool.clone(),
                    packet_pool.clone(),
                    scheduler_status.clone(),
                    scheduler_result.clone(),
                ) {
                    Self::cleanup(&scheduler_status, ffmpeg_context);
                    return Err(e);
                }
            }
        }

        // Demuxer
        for (demux_idx, demux) in ffmpeg_context.demuxs.iter_mut().enumerate() {
            if let Err(e) = demux_init(
                demux_idx,
                demux,
                ffmpeg_context.independent_readrate,
                packet_pool.clone(),
                demux.node.clone(),
                scheduler_status.clone(),
                scheduler_result.clone(),
            ) {
                Self::cleanup(&scheduler_status, ffmpeg_context);
                return Err(e);
            }
        }

        input_controller.as_ref().update_locked(&scheduler_status);

        // Create Running state with guard for Drop implementation
        let mut running_scheduler = self.into_state::<Running>();
        running_scheduler._guard = Some(RunningGuard {
            status: running_scheduler.status.clone(),
            thread_sync: running_scheduler.thread_sync.clone(),
        });

        Ok(running_scheduler)
    }

    /// Cleans up Muxers/Demuxers and signals the job to end if an error occurs
    /// during initialization. This is invoked internally when `start()` fails.
    fn cleanup(scheduler_status: &Arc<AtomicUsize>, ffmpeg_context: &FfmpegContext) {
        muxs_free(&ffmpeg_context.muxs);
        demuxs_free(&ffmpeg_context.demuxs);
        scheduler_status.store(STATUS_END, Ordering::Release);
    }
}

impl FfmpegScheduler<Running> {

    /// Pauses a running FFmpeg job, transitioning from `Running` to `Paused`.
    ///
    /// Internally sets the FFmpeg pipeline threads to a paused state. Depending
    /// on your FFmpeg pipeline design, it may take a moment for all threads to
    /// acknowledge this request. If the scheduler is already ended, this does nothing.
    ///
    /// # Returns
    /// A new [`FfmpegScheduler<Paused>`] representing the paused job.
    pub fn pause(self) -> FfmpegScheduler<Paused> {
        if !is_stopping(self.status.load(Ordering::Acquire)) {
            self.status.store(STATUS_PAUSE, Ordering::Release);
        }
        self.into_state()
    }

    /// Blocks the current thread until the FFmpeg job finishes (success, error, or abort).
    ///
    /// This method is only available in **non-async** builds.
    ///
    /// # Returns
    /// - `Ok(())` if the job completed successfully.
    /// - `Err(...)` if an error was encountered (also logs the error).
    ///
    /// # Notes
    /// - If you enable the `async` feature, this method is replaced by an async `.await`.
    ///   See the `Future` implementation below.
    ///
    /// # Example
    /// ```rust
    /// // After calling `.start()`:
    /// let result = scheduler.wait();
    /// assert!(result.is_ok());
    /// ```
    pub fn wait(self) -> crate::error::Result<()> {
        if !is_stopping(self.status.load(Ordering::Acquire)) {
            self.thread_sync.wait_for_all_threads();
            self.status.store(STATUS_END, Ordering::Release);
        }

        let option = self.result.lock().unwrap().take();
        match option {
            None => {
                log::info!("FFmpeg task succeeded.");
                Ok(())
            }
            Some(result) => {
                log::error!("FFmpeg task failed.");
                result
            }
        }
    }

    /// **WARNING: Immediately aborts the FFmpeg job without waiting for threads to complete.**
    ///
    /// This method signals all threads to stop and returns **immediately**. It does NOT wait
    /// for threads to finish their work, which means:
    ///
    /// - **Output files WILL BE UNUSABLE** (missing trailer, encoder buffers not flushed)
    /// - **Encoded data in buffers will be lost** (B-frames, delayed frames)
    /// - **Files will NOT be seekable or playable** in most media players
    /// - Only use this when you **do not need the output files at all**
    ///
    /// # When to Use
    ///
    /// - Emergency shutdown scenarios where speed is critical
    /// - Preview/test runs where output is discarded
    /// - User cancellation where output is not needed
    ///
    /// # When NOT to Use
    ///
    /// - **NEVER** when you need valid output files → **Use [`stop()`](Self::stop) instead**
    ///
    /// # Why Files Are Unusable
    ///
    /// `abort()` causes:
    /// 1. Encoder to skip flush -> buffered frames lost
    /// 2. Muxer to skip trailer -> no seeking, no playback in most players
    ///
    /// **The ONLY way to get valid files is to use `stop()` instead of `abort()`.**
    ///
    /// # Comparison
    ///
    /// ```rust
    /// // WRONG: Files will be unusable
    /// scheduler.abort();
    ///
    /// // CORRECT: Files will be valid and playable
    /// scheduler.stop();
    /// ```
    ///
    /// # Example
    /// ```rust
    /// let running_scheduler = scheduler.start().unwrap();
    /// // User clicked "Cancel" - don't need output
    /// running_scheduler.abort();
    /// ```
    pub fn abort(self) {
        self.status.store(STATUS_ABORT, Ordering::Release);
    }

    /// Gracefully stops the FFmpeg job and waits for all threads to complete.
    ///
    /// This method **blocks** until all processing is finished and ensures **complete data integrity**.
    /// All encoder buffers are flushed, all output files are properly finalized with trailers,
    /// and all threads are cleanly terminated.
    ///
    /// # Guarantees
    ///
    /// - All buffered frames are encoded and written
    /// - Output files contain proper headers and trailers (e.g., MP4 moov atom, MKV Cues)
    /// - Files are seekable and playable in all media players
    /// - No data loss or corruption
    ///
    /// # When to Use
    ///
    /// - **Always use this** when you need valid output files
    /// - Production transcoding workflows
    /// - Before exiting the application
    /// - Any scenario where output quality matters
    ///
    /// # Comparison with abort()
    ///
    /// | Method | Blocks? | Data Integrity | Use Case |
    /// |--------|---------|----------------|----------|
    /// | `stop()` | Yes | Guaranteed | Production, need valid files |
    /// | `abort()` | No | Not guaranteed | Emergency, don't care about files |
    ///
    /// # Example
    /// ```rust
    /// let running_scheduler = scheduler.start().unwrap();
    /// // ... processing ...
    /// running_scheduler.stop(); // Blocks until complete, files are valid
    /// ```
    pub fn stop(self) {
        self.signal_stop();
        self.thread_sync.wait_for_all_threads();
        log::debug!("stop() completed, all threads finished");
    }
}

#[cfg(feature = "async")]
impl std::future::Future for FfmpegScheduler<Running> {
    type Output = crate::error::Result<()>;

    /// An asynchronous wait for the FFmpeg job. This `Future` **completes**
    /// when the job finishes (either success, error, or abort).
    ///
    /// # Returns
    /// - `Ok(())` if the FFmpeg job completed successfully.
    /// - `Err(...)` if the job was aborted or encountered an error.
    ///
    /// # Example
    /// ```rust,ignore
    /// # // Requires enabling the "async" feature in your Cargo.toml
    /// #[tokio::main]
    /// async fn main() {
    ///     let scheduler = FfmpegScheduler::new(ffmpeg_context).start().unwrap();
    ///     let result = scheduler.await;  // same as scheduler.wait().await
    ///     assert!(result.is_ok());
    /// }
    /// ```
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();

        if is_stopping(this.status.load(Ordering::Acquire)) {
            let option = this.result.lock().unwrap().take();
            std::task::Poll::Ready(match option {
                None => {
                    log::info!("FFmpeg task succeeded.");
                    Ok(())
                }
                Some(result) => {
                    log::error!("FFmpeg task failed.");
                    result
                }
            })
        } else {
            let thread_sync = this.thread_sync.clone();
            let waker = cx.waker().clone();
            thread_sync.set_waker(waker);
            std::task::Poll::Pending
        }
    }
}

impl FfmpegScheduler<Paused> {

    /// Resumes a paused FFmpeg job, transitioning from `Paused` back to `Running`.
    ///
    /// If the scheduler is in an ended state, this has no effect. Otherwise,
    /// it unpauses the pipeline so it can continue processing.
    ///
    /// # Returns
    /// A new [`FfmpegScheduler<Running>`] representing the resumed job.
    pub fn resume(self) -> FfmpegScheduler<Running> {
        if !is_stopping(self.status.load(Ordering::Acquire)) {
            self.status.store(STATUS_RUN, Ordering::Release);
        }
        self.into_state()
    }

    /// **WARNING: Immediately aborts the paused FFmpeg job without waiting for threads to complete.**
    ///
    /// This method has the **exact same behavior and consequences** as [`FfmpegScheduler<Running>::abort()`]:
    ///
    /// - **Output files WILL BE UNUSABLE** (missing trailer, encoder buffers not flushed)
    /// - **Encoded data in buffers will be lost** (B-frames, delayed frames)
    /// - **Files will NOT be seekable or playable** in most media players
    /// - Only use this when you **do not need the output files at all**
    ///
    /// **The ONLY way to get valid files is to use [`stop()`](Running::stop) instead of `abort()`.**
    ///
    /// See [`FfmpegScheduler<Running>::abort()`] for complete documentation.
    ///
    /// # Example
    /// ```rust
    /// let paused_scheduler = running_scheduler.pause();
    /// // User clicked "Cancel" - don't need output
    /// paused_scheduler.abort();
    /// ```
    pub fn abort(self) {
        self.status.store(STATUS_ABORT, Ordering::Release)
    }
}



fn new_frame() -> crate::error::Result<Frame> {
    let frame = unsafe { av_frame_alloc() };
    if frame.is_null() {
        return Err(AllocFrameError::OutOfMemory.into());
    }
    Ok(unsafe { Frame::wrap(frame) })
}

fn new_packet() -> crate::error::Result<Packet> {
    let packet = Packet::empty();
    if packet.as_ptr().is_null() {
        return Err(AllocPacketError::OutOfMemory.into());
    }
    Ok(packet)
}

pub(crate) fn unref_frame(frame: &mut Frame) {
    unsafe { av_frame_unref(frame.as_mut_ptr()) };
}

pub(crate) fn unref_packet(packet: &mut Packet) {
    unsafe { av_packet_unref(packet.as_mut_ptr()) };
}

pub(crate) fn frame_is_null(frame: &Frame) -> bool {
    unsafe { frame.as_ptr().is_null() }
}

pub(crate) fn packet_is_null(packet: &Packet) -> bool {
    packet.as_ptr().is_null()
}

fn demuxs_free(demuxs: &Vec<Demuxer>) {
    for demux in demuxs {
        in_fmt_ctx_free(demux.in_fmt_ctx, demux.is_set_read_callback);
    }
}

fn muxs_free(muxs: &Vec<Muxer>) {
    for mux in muxs {
        out_fmt_ctx_free(mux.out_fmt_ctx, mux.is_set_write_callback);
    }
}

pub(crate) fn wait_until_not_paused(scheduler_status: &Arc<AtomicUsize>) -> usize {
    loop {
        let status = scheduler_status.load(Ordering::Acquire);
        if status == STATUS_PAUSE {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        return status;
    }
}

pub(crate) fn set_scheduler_error(
    scheduler_status: &Arc<AtomicUsize>,
    scheduler_result: &Arc<Mutex<Option<crate::error::Result<()>>>>,
    error: impl Into<crate::error::Error>,
) {
    let mut scheduler_result = scheduler_result.lock().unwrap();
    if scheduler_result.is_none() {
        scheduler_result.replace(Err(error.into()));
        scheduler_status.store(STATUS_END, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use crate::core::context::ffmpeg_context::FfmpegContext;
    use crate::core::context::input::Input;
    use crate::core::context::output::Output;
    use crate::core::filter::frame_filter::NoopFilter;
    use crate::core::scheduler::ffmpeg_scheduler::{
        FfmpegScheduler, Initialization, Paused, Running, STATUS_INIT, STATUS_PAUSE, STATUS_RUN,
    };
    use ffmpeg_sys_next::AVMediaType;
    use log::{info, warn};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::thread::sleep;
    use std::time::Duration;
    use crate::filter::frame_pipeline_builder::FramePipelineBuilder;

    #[test]
    fn test_img_to_video() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let result = FfmpegContext::builder()
            .input(Input::from("logo.jpg")
                .set_input_opt("loop", "1")
                .set_recording_time_us(10 * 1000_000)
            )
            .filter_desc("scale=1280:720")
            .output(Output::from("output.mp4"))
            .build().unwrap()
            .start().unwrap()
            .wait();

        assert!(result.is_ok());
    }
    #[test]
    fn test_copy() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let result = FfmpegContext::builder()
            .input("test.mp4")
            .output( Output::from("output.mp4")
                .add_stream_map_with_copy("0:v")
                .add_stream_map_with_copy("0:a")
            )
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());
    }

    #[test]
    fn test_concat() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let result = FfmpegContext::builder()
            .input("test.mp4")
            .input("test.mp4")
            .input("test.mp4")
            .filter_desc("concat=n=3:v=1:a=1")
            .output("output.mp4")
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());

        std::thread::sleep(Duration::from_secs(1));
    }

    #[test]
    fn test_to_stdout() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let input: Input = "test.mp4".into();
        let output: Output = "-".into();

        let result = FfmpegContext::builder()
            .input(input.set_hwaccel("videotoolbox"))
            .output(output.set_format("null").set_video_codec("h264"))
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());
    }

    #[test]
    fn test_thumbnail() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut input: Input = "test.mp4".into();
        let output: Output = "output.jpg".into();

        input.hwaccel = Some("videotoolbox".to_string());

        let result = FfmpegContext::builder()
            .input(input)
            .filter_desc("scale='min(160,iw)':-1")
            .output(output.set_max_video_frames(1))
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());
    }

    #[test]
    fn test_read_write_callback_mp4() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        use std::fs::File;
        use std::io::{Read, Seek, SeekFrom, Write};

        let input_file = "test.mp4";
        let output_file = "output.mp4";

        let input_file = Arc::new(Mutex::new(
            File::open(input_file).expect("Failed to open input file"),
        ));
        let read_callback: Box<dyn FnMut(&mut [u8]) -> i32> = {
            let input = Arc::clone(&input_file);
            Box::new(move |buf: &mut [u8]| -> i32 {
                let mut input = input.lock().unwrap();
                match input.read(buf) {
                    Ok(0) => ffmpeg_sys_next::AVERROR_EOF,
                    Ok(bytes_read) => bytes_read as i32,
                    Err(_) => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO),
                }
            })
        };

        let seek_callback: Box<dyn FnMut(i64, i32) -> i64> = {
            let input = Arc::clone(&input_file);
            Box::new(move |offset: i64, whence: i32| -> i64 {
                let mut input = input.lock().unwrap();
                // ✅ Handle AVSEEK_SIZE: Return total file size
                if whence == ffmpeg_sys_next::AVSEEK_SIZE {
                    if let Ok(size) = input.metadata().map(|m| m.len() as i64) {
                        info!("FFmpeg requested stream size: {}", size);
                        return size;
                    }
                    return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
                }
                let seek_result = match whence {
                    ffmpeg_sys_next::SEEK_SET => input.seek(SeekFrom::Start(offset as u64)),
                    ffmpeg_sys_next::SEEK_CUR => input.seek(SeekFrom::Current(offset)),
                    ffmpeg_sys_next::SEEK_END => input.seek(SeekFrom::End(offset)),
                    _ => {
                        warn!("Unsupported seek mode: {whence}");
                        return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::ESPIPE) as i64;
                    }
                };

                match seek_result {
                    Ok(new_pos) => new_pos as i64,
                    Err(_) => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64,
                }
            })
        };
        let mut input: Input = read_callback.into();
        input.seek_callback = Some(seek_callback);

        let output_file = Arc::new(Mutex::new(
            File::create(output_file).expect("Failed to create output file"),
        ));
        let write_callback: Box<dyn FnMut(&[u8]) -> i32> = {
            let output_file = Arc::clone(&output_file);
            Box::new(move |buf: &[u8]| -> i32 {
                let mut output_file = output_file.lock().unwrap();
                match output_file.write_all(buf) {
                    Ok(_) => buf.len() as i32,
                    Err(_) => ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO),
                }
            })
        };

        let seek_callback: Box<dyn FnMut(i64, i32) -> i64> = {
            let output_file = Arc::clone(&output_file);
            Box::new(move |offset: i64, whence: i32| -> i64 {
                let mut file = output_file.lock().unwrap();

                if whence == ffmpeg_sys_next::AVSEEK_SIZE {
                    if let Ok(size) = file.metadata().map(|m| m.len() as i64) {
                        println!("FFmpeg requested stream size: {}", size);
                        return size;
                    }
                    return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
                }

                if (whence & ffmpeg_sys_next::AVSEEK_FLAG_BYTE) != 0 {
                    println!(
                        "FFmpeg requested byte-based seeking. Seeking to byte offset: {}",
                        offset
                    );
                    if let Ok(new_pos) = file.seek(SeekFrom::Start(offset as u64)) {
                        return new_pos as i64;
                    }
                    return ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64;
                }

                let normalized_whence = whence & !ffmpeg_sys_next::AVSEEK_FORCE;

                match normalized_whence {
                    ffmpeg_sys_next::SEEK_SET => file.seek(SeekFrom::Start(offset as u64)),
                    ffmpeg_sys_next::SEEK_CUR => file.seek(SeekFrom::Current(offset)),
                    ffmpeg_sys_next::SEEK_END => file.seek(SeekFrom::End(offset)),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Unsupported seek mode",
                    )),
                }
                .map_or(
                    ffmpeg_sys_next::AVERROR(ffmpeg_sys_next::EIO) as i64,
                    |pos| pos as i64,
                )
            })
        };
        let mut output: Output = write_callback.into();
        output.seek_callback = Some(seek_callback);
        output.format = Some("mp4".to_string());

        let context = FfmpegContext::builder()
            .input(input)
            .filter_desc("hue=s=0")
            .output(output)
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();
        let result = scheduler.wait();
        assert!(result.is_ok());
    }

    #[ignore]
    #[test]
    fn test_pipeline() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let output: Output = "output.mp4".into();
        let frame_pipeline_builder: FramePipelineBuilder = AVMediaType::AVMEDIA_TYPE_VIDEO.into();
        let frame_pipeline_builder = frame_pipeline_builder.filter("test", Box::new(NoopFilter::new(AVMediaType::AVMEDIA_TYPE_VIDEO)));
        let output = output.add_frame_pipeline(frame_pipeline_builder);

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output(output)
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();
        scheduler.wait().unwrap();
    }

    #[test]
    fn test_is_ended() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();

        sleep(Duration::from_secs(2));

        assert!(scheduler.is_ended())
    }

    #[test]
    fn test_hwaccel() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let input: Input = "test.mp4".into();
        let output: Output = "output.mp4".into();

        let result = FfmpegContext::builder()
            .input(input.set_hwaccel("videotoolbox"))
            .filter_desc("hue=s=0")
            .output(output.set_video_codec("h264_videotoolbox"))
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_async() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();
        let result = scheduler.await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_pause() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();
        let scheduler = scheduler.pause();
        assert!(scheduler.is_state::<Paused>());
        sleep(Duration::from_secs(1));
        let scheduler = scheduler.resume();
        let result = scheduler.wait();
        assert!(result.is_ok());
    }

    #[test]
    fn test_pause_abort() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();
        let scheduler = scheduler.pause();
        assert!(scheduler.is_state::<Paused>());
        sleep(Duration::from_secs(1));
        scheduler.abort();
    }

    #[test]
    fn test_wait() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let result = FfmpegScheduler::new(context)
            .start()
            .unwrap()
            .wait();
        assert!(result.is_ok());
    }

    #[test]
    fn test_status() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        assert!(!scheduler.is_state::<Paused>());
        assert!(scheduler.is_state::<Initialization>());
        assert_eq!(scheduler.status.load(Ordering::Acquire), STATUS_INIT);
        let scheduler = scheduler.start().unwrap();
        assert_eq!(scheduler.status.load(Ordering::Acquire), STATUS_RUN);
        assert!(scheduler.is_state::<Running>());
        let scheduler = scheduler.pause();
        assert_eq!(scheduler.status.load(Ordering::Acquire), STATUS_PAUSE);
        assert!(scheduler.is_state::<Paused>());
        let scheduler = scheduler.resume();
        assert_eq!(scheduler.status.load(Ordering::Acquire), STATUS_RUN);
        assert!(scheduler.is_state::<Running>());
        scheduler.abort();

        sleep(Duration::from_secs(1));
    }

    #[test]
    fn test_stop() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let context = FfmpegContext::builder()
            .input("test.mp4")
            .filter_desc("hue=s=0")
            .output("output.mp4")
            .build()
            .unwrap();

        let scheduler = FfmpegScheduler::new(context);
        let scheduler = scheduler.start().unwrap();

        // Let the job process some frames before stopping
        sleep(Duration::from_millis(500));

        // stop() should block until all threads complete
        scheduler.stop();

        // Verify output file exists and has content
        let metadata = std::fs::metadata("output.mp4").unwrap();
        assert!(metadata.len() > 0, "Output file should have content after stop()");
    }
}
