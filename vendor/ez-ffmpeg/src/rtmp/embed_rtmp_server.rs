use crate::core::context::output::Output;
use crate::error::Error::{RtmpCreateStream, RtmpStreamAlreadyExists};
use crate::flv::flv_buffer::FlvBuffer;
use crate::flv::flv_tag::FlvTag;
use crate::rtmp::rtmp_connection::{ConnectionError, ReadResult, RtmpConnection};
use crate::rtmp::rtmp_scheduler::{RtmpScheduler, ServerResult};
use bytes::{BufMut, Bytes};
use log::{debug, error, info, warn};
use rml_rtmp::chunk_io::ChunkSerializer;
use rml_rtmp::messages::{MessagePayload, RtmpMessage};
use rml_rtmp::rml_amf0::Amf0Value;
use rml_rtmp::time::RtmpTimestamp;
use slab::Slab;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use crate::core::scheduler::type_to_symbol;
use crate::error::OpenDecoderOperationError;

#[derive(Clone)]
pub struct Initialization;
#[derive(Clone)]
pub struct Running;
#[derive(Clone)]
pub struct Ended;

#[derive(Clone)]
pub struct EmbedRtmpServer<S> {
    address: String,
    status: Arc<AtomicUsize>,
    stream_keys: dashmap::DashSet<String>,
    // stream_key bytes_receiver
    publisher_sender: Option<crossbeam_channel::Sender<(String, crossbeam_channel::Receiver<Vec<u8>>)>>,
    gop_limit: usize,
    state: PhantomData<S>,
}

const STATUS_INIT: usize = 0;
const STATUS_RUN: usize = 1;
const STATUS_END: usize = 2;

impl<S: 'static> EmbedRtmpServer<S> {
    fn is_state<T: 'static>(&self) -> bool {
        std::any::TypeId::of::<S>() == std::any::TypeId::of::<T>()
    }

    fn into_state<T>(self) -> EmbedRtmpServer<T> {
        EmbedRtmpServer {
            address: self.address,
            status: self.status,
            stream_keys: self.stream_keys,
            publisher_sender: self.publisher_sender,
            gop_limit: self.gop_limit,
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
            status: Arc::new(AtomicUsize::new(STATUS_INIT)),
            stream_keys: Default::default(),
            publisher_sender: None,
            gop_limit,
            state: Default::default(),
        }
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

        listener
            .set_nonblocking(true)
            .map_err(|e| <std::io::Error as Into<crate::error::Error>>::into(e))?;

        self.status.store(STATUS_RUN, Ordering::Release);

        let (stream_sender, stream_receiver) = crossbeam_channel::unbounded();
        let (publisher_sender, publisher_receiver) = crossbeam_channel::bounded(1024);
        self.publisher_sender = Some(publisher_sender);
        let stream_keys = self.stream_keys.clone();
        let status = self.status.clone();
        let result = std::thread::Builder::new()
            .name("rtmp-server-worker".to_string())
            .spawn(move || handle_connections(stream_receiver, publisher_receiver, stream_keys, self.gop_limit, status));
        if let Err(e) = result {
            error!("Thread[rtmp-server-worker] exited with error: {e}");
            return Err(crate::error::Error::RtmpThreadExited);
        }

        info!(
            "Embed rtmp server listening for connections on {}.",
            &self.address
        );

        let status = self.status.clone();
        let result = std::thread::Builder::new()
            .name("rtmp-server-io".to_string())
            .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        debug!("New rtmp connection.");
                        if let Err(_) = stream_sender.send(stream) {
                            error!("Error sending stream to rtmp connection handler");
                            status.store(STATUS_END, Ordering::Release);
                            return;
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
    /// ```rust
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
        let write_callback: Box<dyn FnMut(&[u8]) -> i32> = Box::new(move |buf: &[u8]| -> i32 {
            flv_buffer.write_data(buf);
            if let Some(mut flv_tag) = flv_buffer.get_flv_tag() {
                flv_tag.header.stream_id = 1;
                let packet = serializer
                    .serialize(&flv_tag_to_message_payload(flv_tag), false, true)
                    .unwrap();
                message_sender.send(packet.bytes).unwrap();
            }
            buf.len() as i32
        });

        let mut output: Output = write_callback.into();

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

        let (sender, receiver) = crossbeam_channel::unbounded();

        let publisher_sender = self.publisher_sender.as_ref().unwrap();
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
        .into_message_payload(RtmpTimestamp { value: 0 }, 0)
        .unwrap();

        let connect_packet = serializer.serialize(&connect_cmd, false, true).unwrap();
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
        .into_message_payload(RtmpTimestamp { value: 0 }, 1)
        .unwrap();

        let create_stream_packet = serializer
            .serialize(&create_stream_cmd, false, true)
            .unwrap();
        if let Err(_) = sender.send(create_stream_packet.bytes) {
            error!("Can't send createStream command to rtmp server.");
            return Err(RtmpCreateStream.into());
        }

        // send publish
        let mut arguments = Vec::new();
        arguments.push(Amf0Value::Utf8String(stream_key));
        arguments.push(Amf0Value::Utf8String("live".into()));
        let create_stream_cmd = RtmpMessage::Amf0Command {
            command_name: "publish".to_string(),
            transaction_id: 3.0,
            command_object: Amf0Value::Null,
            additional_arguments: arguments,
        }
        .into_message_payload(RtmpTimestamp { value: 0 }, 1)
        .unwrap();

        let create_stream_packet = serializer
            .serialize(&create_stream_cmd, false, true)
            .unwrap();
        if let Err(_) = sender.send(create_stream_packet.bytes) {
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
    /// ```rust
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

fn handle_connections(
    connection_receiver: crossbeam_channel::Receiver<TcpStream>,
    publisher_receiver: crossbeam_channel::Receiver<(String, crossbeam_channel::Receiver<Vec<u8>>)>,
    stream_keys: dashmap::DashSet<String>,
    gop_limit: usize,
    status: Arc<AtomicUsize>,
) {
    let mut connections = Slab::new();
    let mut publishers = Slab::new();
    let mut scheduler = RtmpScheduler::new(gop_limit);

    loop {
        crossbeam::channel::select! {
            // receive new tcp connection
            recv(connection_receiver) -> msg => match msg {
                Ok(stream) => {
                    let entry = connections.vacant_entry();
                    let connection_id = entry.key();
                    let result = RtmpConnection::new(connection_id, stream);
                    match result {
                        Ok(connection) => {
                            entry.insert(connection);
                            debug!("Rtmp connection {connection_id} started");
                        }
                        Err(e) => debug!("Rtmp connection error: {e:?}"),
                    }
                }
                Err(_) => {
                    debug!("Embed rtmp server disconnected.");
                    return;
                }
            },
            // receive new publisher
            recv(publisher_receiver) -> msg => match msg {
                Ok((stream_key, bytes_receiver)) => {
                    let entry = publishers.vacant_entry();
                    let connection_id = entry.key();

                    if scheduler.new_channel(stream_key.clone(), connection_id) {
                        entry.insert((stream_key, bytes_receiver));
                        debug!("Publisher {connection_id} started");
                    }
                }
                Err(_) => {
                    error!("Embed rtmp server publisher_sender closed.");
                    return;
                }
            },
            default(Duration::from_millis(5)) => {}
        }

        if status.load(Ordering::Acquire) == STATUS_END {
            info!("Embed rtmp server stopped.");
            break;
        }

        let mut packets_to_write = Vec::new();
        let mut publisher_ids_to_clear = Vec::new();
        let mut ids_to_clear = Vec::new();
        for (connection_id, (_stream_key, bytes_receiver)) in publishers.iter_mut() {
            loop {
                match bytes_receiver.try_recv() {
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        debug!("Rtmp publisher closed for id {connection_id}");
                        publisher_ids_to_clear.push(connection_id);

                        let mut arguments = Vec::new();
                        arguments.push(Amf0Value::Number(1.0));
                        let create_stream_cmd = RtmpMessage::Amf0Command {
                            command_name: "deleteStream".to_string(),
                            transaction_id: 4.0,
                            command_object: Amf0Value::Null,
                            additional_arguments: arguments,
                        }
                        .into_message_payload(RtmpTimestamp { value: 0 }, 1)
                        .unwrap();

                        let mut serializer = ChunkSerializer::new();
                        let create_stream_packet = serializer
                            .serialize(&create_stream_cmd, false, true)
                            .unwrap();

                        let server_results = match scheduler
                            .publish_bytes_received(connection_id, create_stream_packet.bytes)
                        {
                            Ok(results) => results,
                            Err(_) => {
                                break;
                            }
                        };

                        for result in server_results.into_iter() {
                            match result {
                                ServerResult::OutboundPacket {
                                    target_connection_id,
                                    packet,
                                } => {
                                    packets_to_write.push((target_connection_id, packet));
                                }

                                ServerResult::DisconnectConnection {
                                    connection_id: id_to_close,
                                } => {
                                    ids_to_clear.push(id_to_close);
                                }
                            }
                        }
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Ok(bytes) => {
                        let server_results =
                            match scheduler.publish_bytes_received(connection_id, bytes) {
                                Ok(results) => results,
                                Err(error) => {
                                    debug!("Input caused the following server error: {}", error);
                                    publisher_ids_to_clear.push(connection_id);
                                    break;
                                }
                            };

                        for result in server_results.into_iter() {
                            match result {
                                ServerResult::OutboundPacket {
                                    target_connection_id,
                                    packet,
                                } => {
                                    packets_to_write.push((target_connection_id, packet));
                                }

                                ServerResult::DisconnectConnection {
                                    connection_id: id_to_close,
                                } => {
                                    ids_to_clear.push(id_to_close);
                                }
                            }
                        }
                    }
                }
            }
        }

        for (connection_id, connection) in connections.iter_mut() {
            loop {
                match connection.read() {
                    Err(ConnectionError::SocketClosed) => {
                        debug!("Rtmp socket closed for id {connection_id}");
                        ids_to_clear.push(connection_id);
                        break;
                    }
                    Err(error) => {
                        debug!(
                            "I/O error while reading rtmp connection {connection_id}: {:?}",
                            error
                        );
                        ids_to_clear.push(connection_id);
                        break;
                    }
                    Ok(result) => match result {
                        ReadResult::NoBytesReceived => break,
                        ReadResult::HandshakingInProgress => break,
                        ReadResult::BytesReceived { buffer, byte_count } => {
                            let server_results =
                                match scheduler.bytes_received(connection_id, &buffer[..byte_count]) {
                                    Ok(results) => results,
                                    Err(error) => {
                                        debug!("Rtmp input caused the following server error: {error}");
                                        ids_to_clear.push(connection_id);
                                        break;
                                    }
                                };

                            for result in server_results.into_iter() {
                                match result {
                                    ServerResult::OutboundPacket {
                                        target_connection_id,
                                        packet,
                                    } => {
                                        packets_to_write.push((target_connection_id, packet));
                                    }

                                    ServerResult::DisconnectConnection {
                                        connection_id: id_to_close,
                                    } => {
                                        ids_to_clear.push(id_to_close);
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }

        for publisher_id in publisher_ids_to_clear {
            debug!("Rtmp publisher {publisher_id} closed");
            let (stream_key, _bytes_receiver) = publishers.remove(publisher_id);
            scheduler.notify_publisher_closed(publisher_id);
            stream_keys.remove(&stream_key);
        }

        for (connection_id, packet) in packets_to_write.into_iter() {
            if let Some(connection) = connections.get_mut(connection_id) {
                connection.write(packet.bytes);
            }
        }

        for closed_id in ids_to_clear {
            debug!("Rtmp connection {closed_id} closed");
            let _ = connections.try_remove(closed_id);
            scheduler.notify_connection_closed(closed_id);
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context::ffmpeg_context::FfmpegContext;
    use crate::core::context::input::Input;
    use crate::core::context::output::Output;
    use crate::core::scheduler::ffmpeg_scheduler::FfmpegScheduler;
    use ffmpeg_next::time::current;
    use std::thread::sleep;

    #[test]
    fn test_concat_stream_loop() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
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
    fn test_stream_loop() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
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
    fn test_concat_realtime() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
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
    fn test_realtime() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
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
    fn test_embed_rtmp_server() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut embed_rtmp_server = EmbedRtmpServer::new("localhost:1935");
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
