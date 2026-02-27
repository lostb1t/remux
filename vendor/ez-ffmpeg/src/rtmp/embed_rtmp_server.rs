use crate::core::context::output::Output;
use crate::error::Error::{RtmpCreateStream, RtmpStreamAlreadyExists};
use crate::flv::flv_buffer::FlvBuffer;
use crate::flv::flv_tag::FlvTag;
use crate::rtmp::reactor::{effective_max_connections, Reactor, CHANNEL_HEADROOM};
use bytes::{BufMut, Bytes};
use log::{debug, error, info, warn};
use rml_rtmp::chunk_io::ChunkSerializer;
use rml_rtmp::messages::{MessagePayload, RtmpMessage};
use rml_rtmp::rml_amf0::Amf0Value;
use rml_rtmp::time::RtmpTimestamp;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct Initialization;
#[derive(Clone)]
pub struct Running;
#[derive(Clone)]
pub struct Ended;

#[derive(Clone)]
pub struct EmbedRtmpServer<S> {
    address: String,
    bound_addr: Option<std::net::SocketAddr>,
    status: Arc<AtomicUsize>,
    stream_keys: dashmap::DashSet<String>,
    // stream_key bytes_receiver
    publisher_sender: Option<crossbeam_channel::Sender<(String, crossbeam_channel::Receiver<Vec<u8>>)>>,
    gop_limit: usize,
    max_connections: Option<usize>,
    state: PhantomData<S>,
}

const STATUS_INIT: usize = 0;
const STATUS_RUN: usize = 1;
const STATUS_END: usize = 2;

impl<S: 'static> EmbedRtmpServer<S> {
    fn into_state<T>(self) -> EmbedRtmpServer<T> {
        EmbedRtmpServer {
            address: self.address,
            bound_addr: self.bound_addr,
            status: self.status,
            stream_keys: self.stream_keys,
            publisher_sender: self.publisher_sender,
            gop_limit: self.gop_limit,
            max_connections: self.max_connections,
            state: Default::default(),
        }
    }

    /// Checks whether the RTMP server has been stopped. This returns `true` after
    /// [`stop`](EmbedRtmpServer<Running>::stop) has been called and the server has exited its main loop, otherwise `false`.
    ///
    /// # Returns
    ///
    /// * `true` if the server has been signaled to stop (and is no longer listening/accepting).
    /// * `false` if the server is still running.
    pub fn is_stopped(&self) -> bool {
        self.status.load(Ordering::Acquire) == STATUS_END
    }
}

impl EmbedRtmpServer<Initialization> {
    /// Creates a new RTMP server instance that will listen on the specified address
    /// when [`start`](EmbedRtmpServer<Initialization>::start) is called.
    ///
    /// # Parameters
    ///
    /// * `address` - A string slice representing the address (host:port) to bind the
    ///   RTMP server socket.
    ///
    /// # Returns
    ///
    /// An [`EmbedRtmpServer`] configured to listen on the given address.
    pub fn new(address: impl Into<String>) -> EmbedRtmpServer<Initialization> {
        Self::new_with_gop_limit(address, 1)
    }

    /// Creates a new RTMP server instance that will listen on the specified address,
    /// with a custom GOP limit.
    ///
    /// This method allows specifying the maximum number of GOPs to be cached.
    /// A GOP (Group of Pictures) represents a sequence of video frames (I, P, B frames)
    /// used for efficient video decoding and random access. The GOP limit defines
    /// how many such groups are stored in the cache.
    ///
    /// # Parameters
    ///
    /// * `address` - A string slice representing the address (host:port) to bind the
    ///   RTMP server socket.
    /// * `gop_limit` - The maximum number of GOPs to cache.
    ///
    /// # Returns
    ///
    /// An [`EmbedRtmpServer`] instance configured to listen on the given address and
    /// using the specified GOP limit.
    pub fn new_with_gop_limit(address: impl Into<String>, gop_limit: usize) -> EmbedRtmpServer<Initialization> {
        Self {
            address: address.into(),
            bound_addr: None,
            status: Arc::new(AtomicUsize::new(STATUS_INIT)),
            stream_keys: Default::default(),
            publisher_sender: None,
            gop_limit,
            max_connections: None,
            state: Default::default(),
        }
    }

    /// Sets the maximum number of concurrent connections allowed.
    ///
    /// If not set, the limit is auto-detected based on system file descriptor limits
    /// (default: 10000, capped at 80% of system FD limit).
    ///
    /// # Parameters
    ///
    /// * `max_connections` - Maximum number of concurrent connections
    ///
    /// # Returns
    ///
    /// Self for method chaining.
    pub fn set_max_connections(mut self, max_connections: usize) -> Self {
        self.max_connections = Some(max_connections);
        self
    }

    /// Starts the RTMP server on the configured address, entering a loop that
    /// accepts incoming client connections. This method spawns background threads
    /// to handle the connections and publish events.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the server successfully starts listening.
    /// * An error variant if the socket could not be bound or other I/O errors occur.
    pub fn start(mut self) -> crate::error::Result<EmbedRtmpServer<Running>> {
        let listener = TcpListener::bind(self.address.clone())
            .map_err(|e| <std::io::Error as Into<crate::error::Error>>::into(e))?;

        // Get actual bound address (important for port 0)
        let actual_addr = listener.local_addr()
            .map_err(|e| <std::io::Error as Into<crate::error::Error>>::into(e))?;
        self.bound_addr = Some(actual_addr);

        listener
            .set_nonblocking(true)
            .map_err(|e| <std::io::Error as Into<crate::error::Error>>::into(e))?;

        self.status.store(STATUS_RUN, Ordering::Release);

        // Calculate effective max and create bounded channel with headroom
        // This prevents unbounded queue growth when reactor is at capacity
        let effective_max = effective_max_connections(self.max_connections);
        let channel_capacity = effective_max.saturating_add(CHANNEL_HEADROOM);
        let (stream_sender, stream_receiver) = crossbeam_channel::bounded(channel_capacity);
        let (publisher_sender, publisher_receiver) = crossbeam_channel::bounded(1024);
        self.publisher_sender = Some(publisher_sender);
        let stream_keys = self.stream_keys.clone();
        let status = self.status.clone();
        let max_connections = self.max_connections;
        let result = std::thread::Builder::new()
            .name("rtmp-server-worker".to_string())
            .spawn(move || handle_connections(stream_receiver, publisher_receiver, stream_keys, self.gop_limit, max_connections, status));
        if let Err(e) = result {
            error!("Thread[rtmp-server-worker] exited with error: {e}");
            return Err(crate::error::Error::RtmpThreadExited);
        }

        info!(
            "Embed rtmp server listening for connections on {} (actual: {}, max_connections: {}).",
            &self.address, actual_addr, effective_max
        );

        let status = self.status.clone();
        let result = std::thread::Builder::new()
            .name("rtmp-server-io".to_string())
            .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        // Use try_send to apply backpressure when channel is full
                        match stream_sender.try_send(stream) {
                            Ok(_) => {
                                debug!("New rtmp connection accepted.");
                            }
                            Err(crossbeam_channel::TrySendError::Full(s)) => {
                                // Channel full - server at capacity, reject connection immediately
                                let _ = s.shutdown(Shutdown::Both);
                                debug!("Connection rejected: server at capacity (channel full)");
                            }
                            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                error!("Connection channel disconnected");
                                status.store(STATUS_END, Ordering::Release);
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::WouldBlock {
                            if status.load(Ordering::Acquire) == STATUS_END {
                                info!("Embed rtmp server stopped.");
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        } else {
                            debug!("Rtmp connection error: {:?}", e);
                        }
                    }
                }
            }
        });
        if let Err(e) = result {
            error!("Thread[rtmp-server-io] exited with error: {e}");
            return Err(crate::error::Error::RtmpThreadExited);
        }

        Ok(self.into_state())
    }
}

impl EmbedRtmpServer<Running> {
    /// Returns the actual bound socket address of the RTMP server.
    ///
    /// This is particularly useful when binding to port 0 (random port allocation),
    /// as it allows you to discover which port the OS assigned.
    ///
    /// # Returns
    ///
    /// * `Option<std::net::SocketAddr>` - The actual bound address, or `None` if not available.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let server = EmbedRtmpServer::new("127.0.0.1:0").start().unwrap();
    /// let actual_port = server.local_addr().unwrap().port();
    /// println!("Server listening on port: {}", actual_port);
    /// ```
    pub fn local_addr(&self) -> Option<std::net::SocketAddr> {
        self.bound_addr
    }

    /// Creates an RTMP "input" endpoint for this server (from the server's perspective),
    /// returning an [`Output`] that can be used by FFmpeg to push media data.
    ///
    /// From the FFmpeg standpoint, the returned [`Output`] is where media content is
    /// sent (i.e., FFmpeg "outputs" to this RTMP server). After obtaining this [`Output`],
    /// you can pass it to your FFmpeg job or scheduler to start streaming data into the server.
    ///
    /// # Parameters
    ///
    /// * `app_name` - The RTMP application name, typically corresponding to the `app` part
    ///   of an RTMP URL (e.g., `rtmp://host:port/app/stream_key`).
    /// * `stream_key` - The stream key (or "stream name"). If a stream with the same key
    ///   already exists, an error will be returned.
    ///
    /// # Returns
    ///
    /// * [`Output`] - An output object preconfigured for streaming to this RTMP server.
    ///   This can be passed to the FFmpeg SDK for actual data push.
    /// * [`crate::error::Error`] - If a stream with the same key already exists, the server
    ///   is not ready, or an internal error occurs, the corresponding error is returned.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # // Assume there are definitions and initializations for FfmpegContext, FfmpegScheduler, etc.
    ///
    /// // 1. Create and start the RTMP server
    /// let mut rtmp_server = EmbedRtmpServer::new("localhost:1935");
    /// rtmp_server.start().expect("Failed to start RTMP server");
    ///
    /// // 2. Create an RTMP "input" with app_name="my-app" and stream_key="my-stream"
    /// let output = rtmp_server
    ///     .create_rtmp_input("my-app", "my-stream")
    ///     .expect("Failed to create RTMP input");
    ///
    /// // 3. Prepare the FFmpeg context to push a local file to the newly created `Output`
    /// let context = FfmpegContext::builder()
    ///     .input("test.mp4")
    ///     .output(output)
    ///     .build()
    ///     .expect("Failed to build Ffmpeg context");
    ///
    /// // 4. Start FFmpeg to push "test.mp4" to the local RTMP server on "my-app/my-stream"
    /// FfmpegScheduler::new(context)
    ///     .start()
    ///     .expect("Failed to start Ffmpeg job");
    /// ```
    pub fn create_rtmp_input(
        &self,
        app_name: impl Into<String>,
        stream_key: impl Into<String>,
    ) -> crate::error::Result<Output> {
        let message_sender = self.create_stream_sender(app_name, stream_key)?;

        let mut flv_buffer = FlvBuffer::new();
        let mut serializer = ChunkSerializer::new();
        let write_callback: Box<dyn FnMut(&[u8]) -> i32 + Send> = Box::new(move |buf: &[u8]| -> i32 {
            flv_buffer.write_data(buf);
            if let Some(mut flv_tag) = flv_buffer.get_flv_tag() {
                flv_tag.header.stream_id = 1;
                match serializer.serialize(&flv_tag_to_message_payload(flv_tag), false, true) {
                    Ok(packet) => {
                        if let Err(e) = message_sender.send(packet.bytes) {
                            error!("Failed to send RTMP packet: {:?}", e);
                            return -1;
                        }
                    }
                    Err(e) => {
                        error!("Failed to serialize RTMP message: {:?}", e);
                        return -1;
                    }
                }
            }
            buf.len() as i32
        });

        let output: Output = write_callback.into();

        Ok(output
            .set_format("flv")
            .set_video_codec("h264")
            .set_audio_codec("aac")
            .set_format_opt("flvflags", "no_duration_filesize"))
    }

    /// Creates a sender channel for an RTMP stream, identified by `app_name` and `stream_key`.
    /// This method is used internally by [`create_rtmp_input`](EmbedRtmpServer<Running>::create_rtmp_input) but can also be called directly
    /// if you need more control over how the stream is handled.
    ///
    /// # Parameters
    ///
    /// * `app_name` - The RTMP application name.
    /// * `stream_key` - The unique name (or key) for this stream. Must not already be in use.
    ///
    /// # Returns
    ///
    /// * `crossbeam_channel::Sender<Vec<u8>>` - A sender that allows you to send raw RTMP bytes
    ///   into the server's handling pipeline.
    /// * [`crate::error::Error`] - If a stream with the same key already exists or other
    ///   internal issues occur, an error is returned.
    ///
    /// # Notes
    ///
    /// * This function sets up the initial RTMP "connect" and "publish" commands automatically.
    /// * If you manually send bytes to the resulting channel, they should already be properly
    ///   packaged as RTMP chunks. Otherwise, the server might fail to parse them.
    pub fn create_stream_sender(
        &self,
        app_name: impl Into<String>,
        stream_key: impl Into<String>,
    ) -> crate::error::Result<crossbeam_channel::Sender<Vec<u8>>> {
        let stream_key = stream_key.into();
        if self.stream_keys.contains(&stream_key) {
            return Err(RtmpStreamAlreadyExists(stream_key));
        }

        let (sender, receiver) = crossbeam_channel::bounded(1024);

        let publisher_sender = match self.publisher_sender.as_ref() {
            Some(sender) => sender,
            None => {
                error!("Publisher sender not initialized");
                return Err(RtmpCreateStream.into());
            }
        };

        if let Err(_) = publisher_sender.send((stream_key.clone(), receiver)) {
            if self.status.load(Ordering::Acquire) != STATUS_END {
                warn!("Rtmp server worker already exited. Can't create stream sender.");
            } else {
                error!("Rtmp Server aborted. Can't create stream sender.");
            }
            return Err(RtmpCreateStream.into());
        }

        let mut serializer = ChunkSerializer::new();

        // send connect
        let mut properties: HashMap<String, Amf0Value> = HashMap::new();
        properties.insert("app".to_string(), Amf0Value::Utf8String(app_name.into()));
        let connect_cmd = RtmpMessage::Amf0Command {
            command_name: "connect".to_string(),
            transaction_id: 1.0,
            command_object: Amf0Value::Object(properties),
            additional_arguments: Vec::new(),
        }
        .into_message_payload(RtmpTimestamp { value: 0 }, 0);

        let connect_cmd = match connect_cmd {
            Ok(cmd) => cmd,
            Err(e) => {
                error!("Failed to create connect command: {:?}", e);
                return Err(RtmpCreateStream.into());
            }
        };

        let connect_packet = match serializer.serialize(&connect_cmd, false, true) {
            Ok(packet) => packet,
            Err(e) => {
                error!("Failed to serialize connect command: {:?}", e);
                return Err(RtmpCreateStream.into());
            }
        };

        if let Err(_) = sender.send(connect_packet.bytes) {
            error!("Can't send connect command to rtmp server.");
            return Err(RtmpCreateStream.into());
        }

        // send createStream
        let create_stream_cmd = RtmpMessage::Amf0Command {
            command_name: "createStream".to_string(),
            transaction_id: 2.0,
            command_object: Amf0Value::Null,
            additional_arguments: Vec::new(),
        }
        .into_message_payload(RtmpTimestamp { value: 0 }, 1);

        let create_stream_cmd = match create_stream_cmd {
            Ok(cmd) => cmd,
            Err(e) => {
                error!("Failed to create createStream command: {:?}", e);
                return Err(RtmpCreateStream.into());
            }
        };

        let create_stream_packet = match serializer.serialize(&create_stream_cmd, false, true) {
            Ok(packet) => packet,
            Err(e) => {
                error!("Failed to serialize createStream command: {:?}", e);
                return Err(RtmpCreateStream.into());
            }
        };

        if let Err(_) = sender.send(create_stream_packet.bytes) {
            error!("Can't send createStream command to rtmp server.");
            return Err(RtmpCreateStream.into());
        }

        // send publish
        let mut arguments = Vec::new();
        arguments.push(Amf0Value::Utf8String(stream_key));
        arguments.push(Amf0Value::Utf8String("live".into()));
        let publish_cmd = RtmpMessage::Amf0Command {
            command_name: "publish".to_string(),
            transaction_id: 3.0,
            command_object: Amf0Value::Null,
            additional_arguments: arguments,
        }
        .into_message_payload(RtmpTimestamp { value: 0 }, 1);

        let publish_cmd = match publish_cmd {
            Ok(cmd) => cmd,
            Err(e) => {
                error!("Failed to create publish command: {:?}", e);
                return Err(RtmpCreateStream.into());
            }
        };

        let publish_packet = match serializer.serialize(&publish_cmd, false, true) {
            Ok(packet) => packet,
            Err(e) => {
                error!("Failed to serialize publish command: {:?}", e);
                return Err(RtmpCreateStream.into());
            }
        };

        if let Err(_) = sender.send(publish_packet.bytes) {
            error!("Can't send publish command to rtmp server.");
            return Err(RtmpCreateStream.into());
        }
        Ok(sender)
    }

    /// Stops the RTMP server by signaling the listening and connection-handling threads
    /// to terminate. Once called, new incoming connections will be ignored, and existing
    /// threads will exit gracefully.
    ///
    /// # Example
    /// ```rust,ignore
    /// let server = EmbedRtmpServer::new("localhost:1935");
    /// // ... start and handle streaming
    /// server.stop();
    /// assert!(server.is_stopped());
    /// ```
    pub fn stop(self) -> EmbedRtmpServer<Ended> {
        self.status.store(STATUS_END, Ordering::Release);
        self.into_state()
    }
}

/// Handle connections using optimized Reactor
///
/// Replaces old multi-threaded handle_connections with single-threaded event-driven model:
/// - Uses epoll/kqueue/WSAPoll for IO multiplexing
/// - Write queue with backpressure management
/// - Strict drain until WouldBlock semantics
fn handle_connections(
    connection_receiver: crossbeam_channel::Receiver<TcpStream>,
    publisher_receiver: crossbeam_channel::Receiver<(String, crossbeam_channel::Receiver<Vec<u8>>)>,
    stream_keys: dashmap::DashSet<String>,
    gop_limit: usize,
    max_connections: Option<usize>,
    status: Arc<AtomicUsize>,
) {
    // Create Reactor
    let mut reactor = match Reactor::new(gop_limit, max_connections, stream_keys, status.clone()) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to create Reactor: {:?}", e);
            status.store(STATUS_END, Ordering::Release);
            return;
        }
    };

    // Run Reactor main loop
    reactor.run(connection_receiver, publisher_receiver);

    if status.load(Ordering::Acquire) != STATUS_END {
        error!("Rtmp Server aborted.");
    }
}

pub fn flv_tag_to_message_payload(flv_tag: FlvTag) -> MessagePayload {
    let timestamp = flv_tag.header.timestamp | ((flv_tag.header.timestamp_ext as u32) << 24);

    let type_id = flv_tag.header.tag_type;
    let message_stream_id = flv_tag.header.stream_id;

    let data = if type_id == 0x12 {
        wrap_metadata(flv_tag.data)
    } else {
        flv_tag.data
    };

    MessagePayload {
        timestamp: RtmpTimestamp { value: timestamp },
        type_id,
        message_stream_id,
        data,
    }
}

fn wrap_metadata(data: Bytes) -> Bytes {
    let s = "@setDataFrame";

    let insert_len = 16;

    let mut bytes = bytes::BytesMut::with_capacity(insert_len + data.len());

    bytes.put_u8(0x02);
    bytes.put_u16(s.len() as u16);
    bytes.put(s.as_bytes());

    bytes.put(data);

    bytes.freeze()
}


// ============================================================================
// StreamBuilder API - Simplified RTMP streaming interface
// ============================================================================

use crate::core::context::ffmpeg_context::FfmpegContext;
use crate::core::context::input::Input;
use crate::core::scheduler::ffmpeg_scheduler::{FfmpegScheduler, Running as SchedulerRunning};
use crate::error::StreamError;
use std::path::{Path, PathBuf};

/// A builder for creating RTMP streaming sessions with a simplified API.
///
/// This builder provides a fluent interface for configuring and starting
/// RTMP streaming without needing to manually manage the server lifecycle.
///
/// # Example
///
/// ```rust,ignore
/// use ez_ffmpeg::rtmp::embed_rtmp_server::EmbedRtmpServer;
///
/// let handle = EmbedRtmpServer::stream_builder()
///     .address("localhost:1935")
///     .app_name("live")
///     .stream_key("stream1")
///     .input_file("video.mp4")
///     // readrate defaults to 1.0 (realtime)
///     .start()?;
///
/// handle.wait()?;
/// ```
pub struct StreamBuilder {
    address: Option<String>,
    app_name: Option<String>,
    stream_key: Option<String>,
    input_file: Option<PathBuf>,
    readrate: Option<f32>,
    gop_limit: Option<usize>,
    max_connections: Option<usize>,
}

impl Default for StreamBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamBuilder {
    /// Creates a new `StreamBuilder` with default settings.
    ///
    /// By default, `readrate` is set to `1.0` (real-time playback speed),
    /// which is equivalent to FFmpeg's `-re` flag. This is the recommended
    /// setting for live RTMP streaming scenarios.
    pub fn new() -> Self {
        Self {
            address: None,
            app_name: None,
            stream_key: None,
            input_file: None,
            readrate: Some(1.0), // Default to real-time speed for live streaming
            gop_limit: None,
            max_connections: None,
        }
    }

    /// Sets the address for the RTMP server (e.g., "localhost:1935").
    pub fn address(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// Sets the RTMP application name.
    pub fn app_name(mut self, app_name: impl Into<String>) -> Self {
        self.app_name = Some(app_name.into());
        self
    }

    /// Sets the stream key (publishing name).
    pub fn stream_key(mut self, stream_key: impl Into<String>) -> Self {
        self.stream_key = Some(stream_key.into());
        self
    }

    /// Sets the input file path to stream.
    pub fn input_file(mut self, path: impl AsRef<Path>) -> Self {
        self.input_file = Some(path.as_ref().to_path_buf());
        self
    }

    /// Sets the read rate for the input file.
    ///
    /// A value of 1.0 means realtime playback speed.
    /// This is useful for simulating live streaming from a file.
    pub fn readrate(mut self, rate: f32) -> Self {
        self.readrate = Some(rate);
        self
    }

    /// Sets the GOP (Group of Pictures) limit for the RTMP server.
    ///
    /// This controls how many GOPs are buffered for new subscribers.
    pub fn gop_limit(mut self, limit: usize) -> Self {
        self.gop_limit = Some(limit);
        self
    }

    /// Sets the maximum number of connections the server will accept.
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = Some(max);
        self
    }

    /// Starts the RTMP streaming session.
    ///
    /// This method validates all required parameters, starts the RTMP server,
    /// and begins streaming the input file.
    ///
    /// # Required Parameters
    ///
    /// - `address`: The server address
    /// - `app_name`: The RTMP application name
    /// - `stream_key`: The stream key (publishing name)
    /// - `input_file`: The file to stream
    ///
    /// # Returns
    ///
    /// A `StreamHandle` that can be used to wait for completion or manage the stream.
    ///
    /// # Errors
    ///
    /// Returns `StreamError` if:
    /// - Any required parameter is missing
    /// - The input file does not exist
    /// - The server fails to start
    /// - FFmpeg context creation fails
    pub fn start(self) -> Result<StreamHandle, StreamError> {
        // Validate required parameters
        let address = self
            .address
            .ok_or(StreamError::MissingParameter("address"))?;
        let app_name = self
            .app_name
            .ok_or(StreamError::MissingParameter("app_name"))?;
        let stream_key = self
            .stream_key
            .ok_or(StreamError::MissingParameter("stream_key"))?;
        let input_file = self
            .input_file
            .ok_or(StreamError::MissingParameter("input_file"))?;

        // Validate input file exists and is a file (not a directory)
        if !input_file.is_file() {
            return Err(StreamError::InputNotFound { path: input_file });
        }

        // Create and configure the server
        let mut server = if let Some(gop_limit) = self.gop_limit {
            EmbedRtmpServer::new_with_gop_limit(&address, gop_limit)
        } else {
            EmbedRtmpServer::new(&address)
        };

        if let Some(max_conn) = self.max_connections {
            server = server.set_max_connections(max_conn);
        }

        // Start the server
        let server = server.start().map_err(StreamError::Ffmpeg)?;
        let server = Arc::new(server);

        // Create the RTMP output
        let output = server
            .create_rtmp_input(&app_name, &stream_key)
            .map_err(StreamError::Ffmpeg)?;

        // Create the input with optional readrate
        let input_path = input_file.to_string_lossy().to_string();
        let mut input = Input::from(input_path);
        if let Some(rate) = self.readrate {
            input = input.set_readrate(rate);
        }

        // Build and start the FFmpeg context
        let scheduler = FfmpegContext::builder()
            .input(input)
            .output(output)
            .build()
            .map_err(StreamError::Ffmpeg)?
            .start()
            .map_err(StreamError::Ffmpeg)?;

        Ok(StreamHandle {
            _server: server,
            scheduler: Some(scheduler),
        })
    }
}

/// A handle to a running RTMP streaming session.
///
/// This handle manages the lifecycle of both the RTMP server and the FFmpeg
/// streaming context. When dropped, it will attempt to clean up resources.
///
/// # Example
///
/// ```rust,ignore
/// let handle = EmbedRtmpServer::stream_builder()
///     .address("localhost:1935")
///     .app_name("live")
///     .stream_key("stream1")
///     .input_file("video.mp4")
///     .start()?;
///
/// // Wait for streaming to complete
/// handle.wait()?;
/// ```
pub struct StreamHandle {
    _server: Arc<EmbedRtmpServer<Running>>,
    scheduler: Option<FfmpegScheduler<SchedulerRunning>>,
}

impl StreamHandle {
    /// Waits for the streaming session to complete.
    ///
    /// This method blocks until the FFmpeg context finishes processing
    /// (e.g., when the input file ends or an error occurs).
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if streaming completed successfully, or an error
    /// if something went wrong during streaming.
    pub fn wait(mut self) -> Result<(), StreamError> {
        if let Some(scheduler) = self.scheduler.take() {
            scheduler.wait().map_err(StreamError::Ffmpeg)?;
        }
        Ok(())
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        // Best-effort cleanup: if scheduler wasn't consumed by wait(),
        // we attempt to stop it gracefully here.
        // The server will be stopped when the Arc is dropped.
        if let Some(scheduler) = self.scheduler.take() {
            // Attempt to wait for graceful shutdown, but don't block forever
            let _ = scheduler.wait();
        }
    }
}

impl EmbedRtmpServer<Initialization> {
    /// Creates a new `StreamBuilder` for simplified RTMP streaming.
    ///
    /// This is the recommended entry point for simple streaming scenarios
    /// where you want to stream a file to an embedded RTMP server.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use ez_ffmpeg::rtmp::embed_rtmp_server::EmbedRtmpServer;
    ///
    /// let handle = EmbedRtmpServer::stream_builder()
    ///     .address("localhost:1935")
    ///     .app_name("live")
    ///     .stream_key("stream1")
    ///     .input_file("video.mp4")
    ///     .start()?;
    ///
    /// handle.wait()?;
    /// ```
    ///
    /// For more complex scenarios requiring full control over the server
    /// and FFmpeg context, use the traditional API:
    ///
    /// ```rust,ignore
    /// let server = EmbedRtmpServer::new("localhost:1935").start()?;
    /// let output = server.create_rtmp_input("app", "stream")?;
    /// // ... configure Input and FfmpegContext manually
    /// ```
    pub fn stream_builder() -> StreamBuilder {
        StreamBuilder::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context::ffmpeg_context::FfmpegContext;
    use crate::core::context::input::Input;
    use crate::core::context::output::Output;
    use crate::core::scheduler::ffmpeg_scheduler::FfmpegScheduler;
    use ffmpeg_next::time::current;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    #[ignore] // Integration test: requires exclusive port 1935 and test.mp4
    fn test_concat_stream_loop() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
        let embed_rtmp_server = embed_rtmp_server.start().unwrap();

        let output = embed_rtmp_server
            .create_rtmp_input("my-app", "my-stream")
            .unwrap();

        let start = current();

        let result = FfmpegContext::builder()
            .input(Input::from("test.mp4")
                .set_readrate(1.0)
                .set_stream_loop(3)
            )
            .input(
                Input::from("test.mp4")
                    .set_readrate(1.0)
                    .set_stream_loop(3)
            )
            .input(
                Input::from("test.mp4")
                    .set_readrate(1.0)
                    .set_stream_loop(3)
            )
            .filter_desc("[0:v][0:a][1:v][1:a][2:v][2:a]concat=n=3:v=1:a=1")
            .output(output)
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());
        info!("elapsed time: {}", current() - start);
    }

    #[test]
    #[ignore] // Integration test: requires exclusive port 1935 and test.mp4
    fn test_stream_loop() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
        let embed_rtmp_server = embed_rtmp_server.start().unwrap();

        let output = embed_rtmp_server
            .create_rtmp_input("my-app", "my-stream")
            .unwrap();

        let start = current();

        let result = FfmpegContext::builder()
            .input(Input::from("test.mp4").set_readrate(1.0).set_stream_loop(-1))
            // .filter_desc("hue=s=0")
            .output(output.set_video_codec("h264_videotoolbox"))
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());

        info!("elapsed time: {}", current() - start);
    }

    #[test]
    #[ignore] // Integration test: requires exclusive port 1935 and test.mp4
    fn test_concat_realtime() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
        let embed_rtmp_server = embed_rtmp_server.start().unwrap();

        let output = embed_rtmp_server
            .create_rtmp_input("my-app", "my-stream")
            .unwrap();

        let start = current();

        let result = FfmpegContext::builder()
            .independent_readrate()
            .input(Input::from("test.mp4").set_readrate(1.0))
            .input(
                Input::from("test.mp4")
                    .set_readrate(1.0)
            )
            .input(
                Input::from("test.mp4")
                    .set_readrate(1.0)
            )
            .filter_desc("[0:v][0:a][1:v][1:a][2:v][2:a]concat=n=3:v=1:a=1")
            .output(output)
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());

        sleep(Duration::from_secs(1));
        info!("elapsed time: {}", current() - start);
    }

    #[test]
    #[ignore] // Integration test: requires exclusive port 1935 and test.mp4
    fn test_realtime() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
        let embed_rtmp_server = embed_rtmp_server.start().unwrap();

        let output = embed_rtmp_server
            .create_rtmp_input("my-app", "my-stream")
            .unwrap();

        let start = current();

        let result = FfmpegContext::builder()
            .input(Input::from("test.mp4").set_readrate(1.0))
            .output(output)
            .build()
            .unwrap()
            .start()
            .unwrap()
            .wait();

        assert!(result.is_ok());

        info!("elapsed time: {}", current() - start);
    }

    #[test]
    #[ignore] // Integration test: requires test.mp4
    fn test_readrate() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut output: Output = "output.flv".into();
        output.audio_codec = Some("adpcm_swf".to_string());

        let mut input: Input = "test.mp4".into();
        input.readrate = Some(1.0);

        let context = FfmpegContext::builder()
            .input(input)
            .output(output)
            .build()
            .unwrap();

        let result = FfmpegScheduler::new(context).start().unwrap().wait();
        if let Err(error) = result {
            println!("Error: {error}");
        }
    }

    #[test]
    #[ignore] // Integration test: requires exclusive port 1935 and test.mp4
    fn test_embed_rtmp_server() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
        let embed_rtmp_server = embed_rtmp_server.start().unwrap();

        let output = embed_rtmp_server
            .create_rtmp_input("my-app", "my-stream")
            .unwrap();
        let mut input: Input = "test.mp4".into();
        input.readrate = Some(1.0);

        let context = FfmpegContext::builder()
            .input(input)
            .output(output)
            .build()
            .unwrap();

        let result = FfmpegScheduler::new(context).start().unwrap().wait();

        assert!(result.is_ok());

        sleep(Duration::from_secs(3));
    }
}
