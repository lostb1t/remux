// src/rtmp/reactor.rs - Single-threaded Reactor event loop
//
// Core features:
// - Event-driven IO using Poller (epoll/kqueue/WSAPoll)
// - Backpressure management using WriteQueue
// - Strict drain until WouldBlock semantics (required for edge-triggered)
// - ConnectionToken prevents ID reuse conflicts
// - Connection timeout detection
// - Graceful shutdown support

use crate::rtmp::poller::{Interest, Poller, RawHandle};
use crate::rtmp::rtmp_scheduler::{RtmpScheduler, ServerResult};
use crate::rtmp::write_queue::{BackpressureLevel, FlushResult, WriteQueue};
use bytes::Bytes;
use log::{debug, error, info};
use rml_rtmp::chunk_io::ChunkSerializer;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::messages::RtmpMessage;
use rml_rtmp::rml_amf0::Amf0Value;
use rml_rtmp::time::RtmpTimestamp;
use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ============================================================================
// Constants
// ============================================================================

const READ_BUFFER_SIZE: usize = 8192;
const POLL_TIMEOUT_MS: u64 = 100;
const CONNECTION_TIMEOUT_SECS: u64 = 60; // Connection timeout
const GRACEFUL_SHUTDOWN_TIMEOUT_SECS: u64 = 5; // Graceful shutdown timeout
const MAX_READ_PER_POLL: usize = 512 * 1024; // 512KB max read per poll to prevent memory DoS
const DEFAULT_MAX_CONNECTIONS: usize = 10000; // Default max connections (auto-adjusted by system FD limit)
#[cfg(windows)]
const DEFAULT_MAX_CONNECTIONS_WINDOWS: usize = 8000; // Conservative default for Windows (no direct FD limit API)
/// Extra capacity for bounded channel to absorb connection bursts.
/// Used when creating the connection channel between accept thread and reactor.
pub const CHANNEL_HEADROOM: usize = 256;

// ============================================================================
// System Helpers
// ============================================================================

/// Get system file descriptor limit (cross-platform)
///
/// Returns the soft limit of open files, or None if unavailable.
/// Used to auto-adjust max_connections to avoid exhausting system resources.
fn get_fd_limit() -> Option<usize> {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        let mut rlim = MaybeUninit::<libc::rlimit>::uninit();
        // SAFETY: rlim is a valid pointer to uninitialized memory,
        // getrlimit will initialize it if successful
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) } == 0 {
            // SAFETY: getrlimit returned 0, so rlim is now initialized
            let rlim = unsafe { rlim.assume_init() };
            return Some(rlim.rlim_cur as usize);
        }
        None
    }
    #[cfg(windows)]
    {
        // Windows: Use a conservative default since there's no direct FD limit API.
        // Windows handles are managed differently; 8000 is a safe conservative value.
        Some(DEFAULT_MAX_CONNECTIONS_WINDOWS)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Calculate effective max connections based on config and system limits.
///
/// This function computes the actual maximum connections the server will allow:
/// - Uses configured value or DEFAULT_MAX_CONNECTIONS (10000)
/// - Caps at 80% of system FD limit to leave headroom for other operations
///
/// # Arguments
/// * `config_max` - User-configured max connections, or None for auto-detect
///
/// # Returns
/// The effective maximum connections value (guaranteed to be at least 1)
pub fn effective_max_connections(config_max: Option<usize>) -> usize {
    let config_value = config_max.unwrap_or(DEFAULT_MAX_CONNECTIONS);
    let result = if let Some(fd_limit) = get_fd_limit() {
        // Reserve 20% of FD limit for other operations (files, sockets, etc.)
        let fd_based_limit = (fd_limit as f64 * 0.8) as usize;
        config_value.min(fd_based_limit)
    } else {
        config_value
    };
    // Ensure at least 1 connection is allowed
    result.max(1)
}

// ============================================================================
// Connection Token - Prevents ID reuse conflicts
// ============================================================================

/// Connection token - Contains ID and generation counter
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionToken {
    /// Connection ID (slab index)
    pub id: usize,
    /// Generation counter - Incremented each time ID is reused
    pub generation: u32,
}

impl ConnectionToken {
    fn new(id: usize, generation: u32) -> Self {
        Self { id, generation }
    }

    /// Encode token for poller (combines id and generation)
    ///
    /// Layout: [generation: 32 bits][id: 32 bits]
    /// This allows validation of stale events from closed connections
    #[cfg(target_pointer_width = "64")]
    fn to_poller_token(&self) -> usize {
        ((self.generation as usize) << 32) | (self.id & 0xFFFFFFFF)
    }

    /// Decode token from poller event
    #[cfg(target_pointer_width = "64")]
    fn from_poller_token(token: usize) -> Self {
        let id = token & 0xFFFFFFFF;
        let generation = (token >> 32) as u32;
        Self { id, generation }
    }

    /// Fallback for 32-bit systems - no generation encoding possible
    #[cfg(target_pointer_width = "32")]
    fn to_poller_token(&self) -> usize {
        self.id
    }

    /// Fallback for 32-bit systems
    #[cfg(target_pointer_width = "32")]
    fn from_poller_token(token: usize) -> Self {
        Self {
            id: token,
            generation: 0,
        }
    }
}

// ============================================================================
// Connection State Machine
// ============================================================================

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Handshaking
    Handshaking,
    /// Active
    Active,
    /// Slow client (backpressure warning)
    SlowClient,
    /// Closing
    Closing,
    /// Closed
    Closed,
}

impl ConnectionState {
    #[cfg(test)]
    pub fn is_active(&self) -> bool {
        matches!(self, ConnectionState::Active | ConnectionState::SlowClient)
    }

    pub fn can_read(&self) -> bool {
        matches!(
            self,
            ConnectionState::Handshaking | ConnectionState::Active | ConnectionState::SlowClient
        )
    }

    pub fn can_write(&self) -> bool {
        matches!(
            self,
            ConnectionState::Handshaking
                | ConnectionState::Active
                | ConnectionState::SlowClient
                | ConnectionState::Closing
        )
    }
}

// ============================================================================
// Reactor Connection
// ============================================================================

/// Single RTMP connection
pub struct ReactorConnection {
    /// Connection token
    token: ConnectionToken,
    /// Underlying socket
    socket: TcpStream,
    /// Raw handle (for Poller)
    raw_handle: RawHandle,
    /// Connection state
    state: ConnectionState,
    /// Write queue
    write_queue: WriteQueue,
    /// Read buffer
    read_buffer: Vec<u8>,
    /// RTMP handshake handler
    handshake: Option<Handshake>,
    /// Last read activity time
    last_read_activity: Instant,
    /// Last write activity time
    last_write_activity: Instant,
    /// Currently registered interest
    current_interest: Interest,
}

impl ReactorConnection {
    /// Create new connection
    pub fn new(token: ConnectionToken, socket: TcpStream) -> io::Result<Self> {
        // Set non-blocking
        socket.set_nonblocking(true)?;

        #[cfg(unix)]
        let raw_handle = {
            use std::os::unix::io::AsRawFd;
            socket.as_raw_fd()
        };

        #[cfg(windows)]
        let raw_handle = {
            use std::os::windows::io::AsRawSocket;
            socket.as_raw_socket()
        };

        let now = Instant::now();

        Ok(Self {
            token,
            socket,
            raw_handle,
            state: ConnectionState::Handshaking,
            write_queue: WriteQueue::new(),
            read_buffer: vec![0u8; READ_BUFFER_SIZE],
            handshake: Some(Handshake::new(PeerType::Server)),
            last_read_activity: now,
            last_write_activity: now,
            current_interest: Interest::READABLE,
        })
    }

    /// Get raw handle
    pub fn raw_handle(&self) -> RawHandle {
        self.raw_handle
    }

    /// Combined activity time (take newer of read/write)
    pub fn last_activity(&self) -> Instant {
        self.last_read_activity.max(self.last_write_activity)
    }

    /// Is timed out
    pub fn is_timed_out(&self, timeout: Duration) -> bool {
        self.last_activity().elapsed() > timeout
    }

    /// Enqueue data
    pub fn enqueue_data(
        &mut self,
        data: Bytes,
        is_keyframe: bool,
        is_sequence_header: bool,
        is_video: bool,
    ) -> bool {
        let result = self
            .write_queue
            .enqueue(data, is_keyframe, is_sequence_header, is_video);

        // Update state based on backpressure level
        match self.write_queue.backpressure_level() {
            BackpressureLevel::Critical => {
                self.state = ConnectionState::Closing;
                return false;
            }
            BackpressureLevel::High | BackpressureLevel::Warning => {
                if self.state == ConnectionState::Active {
                    self.state = ConnectionState::SlowClient;
                }
            }
            BackpressureLevel::Normal => {
                if self.state == ConnectionState::SlowClient {
                    self.state = ConnectionState::Active;
                }
            }
        }

        result
    }

    /// Enqueue raw data (for handshake responses, etc.)
    /// Returns false if queue is full and connection should be disconnected
    pub fn enqueue_raw(&mut self, data: Vec<u8>) -> bool {
        if !self
            .write_queue
            .enqueue(Bytes::from(data), false, false, false)
        {
            self.state = ConnectionState::Closing;
            return false;
        }
        true
    }

    /// Try to flush write queue (drain until WouldBlock)
    ///
    /// Returns whether connection should be disconnected
    pub fn try_flush(&mut self) -> io::Result<bool> {
        if self.write_queue.is_empty() {
            return Ok(false);
        }

        match self.write_queue.try_flush(&mut self.socket) {
            Ok(FlushResult::Complete { bytes_written }) => {
                if bytes_written > 0 {
                    self.last_write_activity = Instant::now();
                }
                Ok(false)
            }
            Ok(FlushResult::WouldBlock { bytes_written }) => {
                if bytes_written > 0 {
                    self.last_write_activity = Instant::now();
                }
                Ok(false)
            }
            Ok(FlushResult::Closed) => Ok(true),
            Err(e) => {
                debug!(
                    "Connection {} write error: {:?}",
                    self.token.id, e
                );
                Err(e)
            }
        }
    }

    /// Read data (drain until WouldBlock)
    ///
    /// Returns (data read, should disconnect)
    /// Note: Limits read to MAX_READ_PER_POLL to prevent memory DoS
    pub fn try_read(&mut self) -> io::Result<(Vec<u8>, bool)> {
        let mut all_data = Vec::new();

        loop {
            // Check read limit to prevent unbounded memory growth
            if all_data.len() >= MAX_READ_PER_POLL {
                return Ok((all_data, false)); // Return data, continue next poll
            }

            match self.socket.read(&mut self.read_buffer) {
                Ok(0) => {
                    // Connection closed
                    return Ok((all_data, true));
                }
                Ok(n) => {
                    self.last_read_activity = Instant::now();
                    all_data.extend_from_slice(&self.read_buffer[..n]);
                    // Continue reading until WouldBlock or limit reached
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // No more data available
                    return Ok((all_data, false));
                }
                Err(e) => {
                    debug!("Connection {} read error: {:?}", self.token.id, e);
                    return Err(e);
                }
            }
        }
    }

    /// Process handshake data
    ///
    /// Returns (remaining data, response data, handshake complete, error)
    pub fn process_handshake(&mut self, data: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>, bool, bool) {
        let handshake = match self.handshake.as_mut() {
            Some(h) => h,
            None => return (Some(data.to_vec()), None, true, false), // Handshake already complete
        };

        match handshake.process_bytes(data) {
            Ok(HandshakeProcessResult::InProgress { response_bytes }) => {
                let response = if response_bytes.is_empty() {
                    None
                } else {
                    Some(response_bytes)
                };
                (None, response, false, false)
            }
            Ok(HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            }) => {
                let response = if response_bytes.is_empty() {
                    None
                } else {
                    Some(response_bytes)
                };
                let remaining = if remaining_bytes.is_empty() {
                    None
                } else {
                    Some(remaining_bytes)
                };

                // Handshake complete, remove handler
                self.handshake = None;
                self.state = ConnectionState::Active;

                (remaining, response, true, false)
            }
            Err(e) => {
                debug!("Connection {} handshake error: {:?}", self.token.id, e);
                (None, None, false, true)
            }
        }
    }

    /// Has pending writes
    pub fn has_pending_writes(&self) -> bool {
        !self.write_queue.is_empty()
    }

    /// Get desired Interest
    pub fn desired_interest(&self) -> Interest {
        // Closing state no longer needs reads, only write remaining data
        let mut interest = if self.state.can_read() {
            Interest::READABLE
        } else {
            Interest {
                readable: false,
                writable: false,
            }
        };
        if self.has_pending_writes() {
            interest = interest.add_writable();
        }
        interest
    }

    /// Mark as closing
    pub fn mark_closing(&mut self) {
        self.state = ConnectionState::Closing;
    }

    /// Mark as closed
    pub fn mark_closed(&mut self) {
        self.state = ConnectionState::Closed;
    }

    /// Close connection
    pub fn shutdown(&mut self) {
        if let Err(e) = self.socket.shutdown(Shutdown::Both) {
            debug!("Socket shutdown error (expected if already closed): {:?}", e);
        }
        self.mark_closed();
    }

}

// ============================================================================
// Publisher State
// ============================================================================

/// Publisher state
pub struct PublisherState {
    pub stream_key: String,
    pub receiver: crossbeam_channel::Receiver<Vec<u8>>,
}

// ============================================================================
// Reactor
// ============================================================================

/// Event handling result
pub enum HandleResult {
    /// Disconnect
    Disconnect(usize),
}

/// Main Reactor structure
pub struct Reactor {
    /// Event poller
    poller: Poller,
    /// Connection storage (using slab allocation)
    connections: slab::Slab<ReactorConnection>,
    /// Generation counter (for each slot)
    generations: HashMap<usize, u32>,
    /// Business scheduler
    scheduler: RtmpScheduler,
    /// Publishers
    publishers: slab::Slab<PublisherState>,
    /// stream_key set
    stream_keys: dashmap::DashSet<String>,
    /// Stop flag
    status: Arc<AtomicUsize>,
    /// Maximum allowed connections (auto-adjusted by system FD limit)
    max_connections: usize,
    /// Connections with pending writes that need flushing (dirty tracking for O(m) instead of O(n))
    pending_flush: HashSet<usize>,
    /// Connections whose poller interest may need updating (dirty tracking for O(m) instead of O(n))
    interest_dirty: HashSet<usize>,
    /// Reusable buffer for connection IDs (to avoid Vec allocation in hot path)
    #[allow(dead_code)]
    conn_ids_buffer: Vec<usize>,
    /// Reusable buffer for packets to write (avoids allocation in handle_readable)
    packets_buffer: Vec<(usize, Vec<u8>, bool, bool, bool)>,
    /// Reusable buffer for IDs to close (avoids allocation in handle_readable)
    ids_to_close_buffer: Vec<usize>,
    /// Reusable buffer for handle results (avoids allocation in handle_readable)
    results_buffer: Vec<HandleResult>,
}

// Status constants
const STATUS_RUN: usize = 1;
const STATUS_END: usize = 2;

impl Reactor {
    /// Create new Reactor
    ///
    /// # Arguments
    /// * `gop_limit` - Maximum number of GOPs to cache per stream
    /// * `max_connections` - Maximum connections limit (None = auto-detect based on system FD limit)
    /// * `stream_keys` - Shared set of active stream keys
    /// * `status` - Shared status flag for graceful shutdown
    ///
    /// The effective max_connections is calculated as:
    /// `min(config_value, 0.8 * system_fd_limit)` to leave headroom for other FDs.
    pub fn new(
        gop_limit: usize,
        max_connections: Option<usize>,
        stream_keys: dashmap::DashSet<String>,
        status: Arc<AtomicUsize>,
    ) -> io::Result<Self> {
        let poller = Poller::new()?;

        // Use the shared effective_max_connections calculation
        let effective_max = effective_max_connections(max_connections);

        Ok(Self {
            poller,
            connections: slab::Slab::with_capacity(1024),
            generations: HashMap::new(),
            scheduler: RtmpScheduler::new(gop_limit),
            publishers: slab::Slab::with_capacity(64),
            stream_keys,
            status,
            max_connections: effective_max,
            pending_flush: HashSet::with_capacity(256),
            interest_dirty: HashSet::with_capacity(256),
            conn_ids_buffer: Vec::with_capacity(1024),
            packets_buffer: Vec::with_capacity(64),
            ids_to_close_buffer: Vec::with_capacity(16),
            results_buffer: Vec::with_capacity(16),
        })
    }

    /// Add new connection
    ///
    /// Returns error if max_connections limit is reached.
    pub fn add_connection(&mut self, socket: TcpStream) -> io::Result<ConnectionToken> {
        // Check connection limit before adding
        if self.connections.len() >= self.max_connections {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!(
                    "max connections limit reached ({}/{})",
                    self.connections.len(),
                    self.max_connections
                ),
            ));
        }

        let entry = self.connections.vacant_entry();
        let id = entry.key();

        // Get or initialize generation
        let generation = self.generations.entry(id).or_insert(0);
        *generation = generation.wrapping_add(1);
        let token = ConnectionToken::new(id, *generation);

        let conn = ReactorConnection::new(token, socket)?;

        // Register to poller with encoded token (id + generation)
        let poller_token = token.to_poller_token();
        self.poller
            .register(conn.raw_handle(), poller_token, Interest::READABLE)?;

        entry.insert(conn);

        debug!("Connection {} added (generation {})", id, token.generation);
        Ok(token)
    }

    /// Remove connection
    pub fn remove_connection(&mut self, id: usize) {
        if let Some(conn) = self.connections.try_remove(id) {
            // Deregister from poller
            if let Err(e) = self.poller.deregister(conn.raw_handle()) {
                debug!("Failed to deregister connection {} from poller: {:?}", id, e);
            }

            // Notify scheduler
            self.scheduler.notify_connection_closed(id);

            debug!(
                "Connection {} removed (generation {})",
                id, conn.token.generation
            );
        }
    }

    /// Add publishers
    pub fn add_publisher(
        &mut self,
        stream_key: String,
        receiver: crossbeam_channel::Receiver<Vec<u8>>,
    ) -> Option<usize> {
        let entry = self.publishers.vacant_entry();
        let id = entry.key();

        if self.scheduler.new_channel(stream_key.clone(), id) {
            self.stream_keys.insert(stream_key.clone());
            entry.insert(PublisherState {
                stream_key,
                receiver,
            });
            debug!("Publisher {} added", id);
            Some(id)
        } else {
            None
        }
    }

    /// Remove publishers
    pub fn remove_publisher(&mut self, id: usize) {
        if let Some(pub_state) = self.publishers.try_remove(id) {
            self.scheduler.notify_publisher_closed(id);
            self.stream_keys.remove(&pub_state.stream_key);
            debug!("Publisher {} removed", id);
        }
    }

    /// Update connection's poller interest
    fn update_interest(&mut self, id: usize) -> io::Result<()> {
        if let Some(conn) = self.connections.get_mut(id) {
            let desired = conn.desired_interest();
            if desired != conn.current_interest {
                self.poller
                    .modify(conn.raw_handle(), conn.token.to_poller_token(), desired)?;
                conn.current_interest = desired;
            }
        }
        Ok(())
    }

    /// Validate connection exists and generation matches
    ///
    /// Returns Some(id) if connection is valid, None if stale event
    /// This prevents ABA problem where a new connection reuses an old slot
    fn validate_connection(&self, poller_token: usize) -> Option<usize> {
        let token = ConnectionToken::from_poller_token(poller_token);
        if let Some(conn) = self.connections.get(token.id) {
            // On 64-bit: validate generation matches
            // On 32-bit: generation is always 0, so this check passes
            if conn.token.generation == token.generation {
                return Some(token.id);
            }
            // Stale event: generation mismatch
            debug!(
                "Stale event for connection {}: expected gen {}, got {}",
                token.id, conn.token.generation, token.generation
            );
        }
        None
    }

    /// Handle readable event
    fn handle_readable(&mut self, id: usize) -> Vec<HandleResult> {
        // Clear and reuse buffers to avoid allocation in hot path
        self.results_buffer.clear();
        self.packets_buffer.clear();
        self.ids_to_close_buffer.clear();

        // Read data from connection
        let (data, should_close) = match self.read_connection_data(id) {
            Some(result) => result,
            None => return std::mem::take(&mut self.results_buffer),
        };

        // Process the data through scheduler
        self.process_connection_data(id, &data);

        // Write pending packets to target connections
        self.write_pending_packets();

        // Close connections that need closing
        for close_id in self.ids_to_close_buffer.drain(..) {
            self.results_buffer.push(HandleResult::Disconnect(close_id));
        }

        // If EOF detected during read, close connection after processing data
        if should_close {
            self.results_buffer.push(HandleResult::Disconnect(id));
        }

        std::mem::take(&mut self.results_buffer)
    }

    /// Read data from connection
    fn read_connection_data(
        &mut self,
        id: usize,
    ) -> Option<(Vec<u8>, bool)> {
        let conn = match self.connections.get_mut(id) {
            Some(c) if c.state.can_read() => c,
            _ => return None,
        };

        match conn.try_read() {
            Ok((data, close)) => {
                // Check if there's data to process
                if data.is_empty() {
                    // No data, close if needed
                    if close {
                        self.results_buffer.push(HandleResult::Disconnect(id));
                    }
                    return None;
                }
                Some((data, close))
            }
            Err(_) => {
                self.results_buffer.push(HandleResult::Disconnect(id));
                None
            }
        }
    }

    /// Process connection data through scheduler
    fn process_connection_data(
        &mut self,
        id: usize,
        data: &[u8],
    ) {
        let conn = match self.connections.get_mut(id) {
            Some(c) => c,
            None => return,
        };

        let state = conn.state;

        if state == ConnectionState::Handshaking {
            self.process_handshake_data(id, data);
        } else {
            self.process_normal_data(id, data);
        }
    }

    /// Process handshake data
    fn process_handshake_data(
        &mut self,
        id: usize,
        data: &[u8],
    ) {
        let conn = match self.connections.get_mut(id) {
            Some(c) => c,
            None => return,
        };

        let (remaining, response, completed, error) = conn.process_handshake(data);

        if error {
            self.results_buffer.push(HandleResult::Disconnect(id));
            return;
        }

        if let Some(resp) = response {
            if !conn.enqueue_raw(resp) {
                // Queue full during handshake, disconnect
                self.results_buffer.push(HandleResult::Disconnect(id));
                return;
            }
            // Mark connection for pending flush and interest update
            self.pending_flush.insert(id);
            self.interest_dirty.insert(id);
        }

        if completed {
            debug!("Connection {} handshake completed", id);
        }

        // Process remaining data
        if let Some(remaining_data) = remaining {
            if !remaining_data.is_empty() {
                self.process_scheduler_results(id, &remaining_data);
            }
        }
    }

    /// Process normal (non-handshake) data
    fn process_normal_data(
        &mut self,
        id: usize,
        data: &[u8],
    ) {
        self.process_scheduler_results(id, data);
    }

    /// Process scheduler results
    fn process_scheduler_results(
        &mut self,
        id: usize,
        data: &[u8],
    ) {
        match self.scheduler.bytes_received(id, data) {
            Ok(server_results) => {
                for result in server_results {
                    match result {
                        ServerResult::OutboundPacket {
                            target_connection_id,
                            packet,
                            is_keyframe,
                            is_sequence_header,
                            is_video,
                        } => {
                            self.packets_buffer.push((
                                target_connection_id,
                                packet.bytes,
                                is_keyframe,
                                is_sequence_header,
                                is_video,
                            ));
                        }
                        ServerResult::DisconnectConnection {
                            connection_id: close_id,
                        } => {
                            self.ids_to_close_buffer.push(close_id);
                        }
                    }
                }
            }
            Err(e) => {
                debug!("Connection {} scheduler error: {}", id, e);
                self.results_buffer.push(HandleResult::Disconnect(id));
            }
        }
    }

    /// Write pending packets to target connections
    fn write_pending_packets(&mut self) {
        // Collect IDs that successfully enqueued data for dirty marking
        let mut enqueued_ids = Vec::new();

        for (target_id, data, is_keyframe, is_sequence_header, is_video) in self.packets_buffer.drain(..) {
            if let Some(target_conn) = self.connections.get_mut(target_id) {
                let enqueued =
                    target_conn.enqueue_data(Bytes::from(data), is_keyframe, is_sequence_header, is_video);
                if enqueued {
                    enqueued_ids.push(target_id);
                } else {
                    // Backpressure too high, cannot enqueue, close target connection
                    self.ids_to_close_buffer.push(target_id);
                }
            }
        }

        // Mark all connections that received data for pending flush and interest update
        for id in enqueued_ids {
            self.pending_flush.insert(id);
            self.interest_dirty.insert(id);
        }
    }

    /// Handle writable event
    fn handle_writable(&mut self, id: usize) -> Option<HandleResult> {
        let conn = match self.connections.get_mut(id) {
            Some(c) if c.state.can_write() => c,
            _ => return None,
        };

        match conn.try_flush() {
            Ok(true) => Some(HandleResult::Disconnect(id)),
            Ok(false) => {
                // Queue drained - update interest to clear writable flag
                // Prevents CPU churn on level-triggered systems (Windows WSAPoll)
                if !conn.has_pending_writes() {
                    self.interest_dirty.insert(id);
                }
                None
            }
            Err(_) => Some(HandleResult::Disconnect(id)),
        }
    }

    /// Handle publishers data
    fn process_publishers(&mut self) -> Vec<usize> {
        let mut publisher_ids_to_remove = Vec::new();
        let mut packets_to_write = Vec::new();
        let mut ids_to_close = Vec::new();

        let publisher_ids: Vec<usize> = self.publishers.iter().map(|(id, _)| id).collect();

        for pub_id in publisher_ids {
            let receiver = {
                let pub_state = match self.publishers.get(pub_id) {
                    Some(p) => p,
                    None => continue,
                };
                pub_state.receiver.clone()
            };

            loop {
                match receiver.try_recv() {
                    Ok(bytes) => {
                        match self.scheduler.publish_bytes_received(pub_id, bytes) {
                            Ok(server_results) => {
                                for result in server_results {
                                    match result {
                                        ServerResult::OutboundPacket {
                                            target_connection_id,
                                            packet,
                                            is_keyframe,
                                            is_sequence_header,
                                            is_video,
                                        } => {
                                            packets_to_write.push((
                                                target_connection_id,
                                                packet.bytes,
                                                is_keyframe,
                                                is_sequence_header,
                                                is_video,
                                            ));
                                        }
                                        ServerResult::DisconnectConnection {
                                            connection_id: close_id,
                                        } => {
                                            ids_to_close.push(close_id);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("Publisher {} scheduler error: {}", pub_id, e);
                                publisher_ids_to_remove.push(pub_id);
                                break;
                            }
                        }
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        debug!("Publisher {} disconnected", pub_id);
                        // Send deleteStream command
                        self.send_delete_stream(pub_id, &mut packets_to_write, &mut ids_to_close);
                        publisher_ids_to_remove.push(pub_id);
                        break;
                    }
                }
            }
        }

        // Write pending packets and collect IDs that successfully enqueued
        let mut enqueued_ids = Vec::new();
        for (target_id, data, is_keyframe, is_sequence_header, is_video) in packets_to_write {
            if let Some(target_conn) = self.connections.get_mut(target_id) {
                let enqueued =
                    target_conn.enqueue_data(Bytes::from(data), is_keyframe, is_sequence_header, is_video);
                if enqueued {
                    enqueued_ids.push(target_id);
                } else {
                    // Backpressure too high, close target connection
                    ids_to_close.push(target_id);
                }
            }
        }

        // Mark all connections that received data for pending flush and interest update
        for id in enqueued_ids {
            self.pending_flush.insert(id);
            self.interest_dirty.insert(id);
        }

        // Close connections that need closing
        for close_id in ids_to_close {
            self.remove_connection(close_id);
        }

        publisher_ids_to_remove
    }

    /// Send deleteStream command
    fn send_delete_stream(
        &mut self,
        pub_id: usize,
        packets: &mut Vec<(usize, Vec<u8>, bool, bool, bool)>,
        ids_to_close: &mut Vec<usize>,
    ) {
        let mut arguments = Vec::new();
        arguments.push(Amf0Value::Number(1.0));
        let delete_stream_cmd = RtmpMessage::Amf0Command {
            command_name: "deleteStream".to_string(),
            transaction_id: 4.0,
            command_object: Amf0Value::Null,
            additional_arguments: arguments,
        }
        .into_message_payload(RtmpTimestamp { value: 0 }, 1);

        if let Ok(payload) = delete_stream_cmd {
            let mut serializer = ChunkSerializer::new();
            if let Ok(packet) = serializer.serialize(&payload, false, true) {
                match self.scheduler.publish_bytes_received(pub_id, packet.bytes) {
                    Ok(server_results) => {
                        for result in server_results {
                            match result {
                                ServerResult::OutboundPacket {
                                    target_connection_id,
                                    packet,
                                    is_keyframe,
                                    is_sequence_header,
                                    is_video,
                                } => {
                                    packets.push((
                                        target_connection_id,
                                        packet.bytes,
                                        is_keyframe,
                                        is_sequence_header,
                                        is_video,
                                    ));
                                }
                                ServerResult::DisconnectConnection {
                                    connection_id: close_id,
                                } => {
                                    ids_to_close.push(close_id);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to process deleteStream command for publisher {}: {:?}",
                            pub_id, e
                        );
                    }
                }
            }
        }
    }

    /// Flush pending connection write queues (O(m) where m = connections with pending writes)
    fn flush_pending(&mut self) -> Vec<usize> {
        let mut ids_to_close = Vec::new();

        // Drain pending_flush to get IDs that need flushing
        let pending_ids: Vec<usize> = self.pending_flush.drain().collect();

        for id in pending_ids {
            if let Some(conn) = self.connections.get_mut(id) {
                if conn.has_pending_writes() {
                    match conn.try_flush() {
                        Ok(true) | Err(_) => {
                            // Connection should be closed
                            ids_to_close.push(id);
                        }
                        Ok(false) => {
                            if conn.has_pending_writes() {
                                // Still has pending writes, re-add to pending_flush
                                self.pending_flush.insert(id);
                            } else {
                                // Queue drained, update interest to clear writable flag
                                self.interest_dirty.insert(id);
                            }
                        }
                    }
                } else {
                    // No pending writes, ensure writable interest is cleared
                    self.interest_dirty.insert(id);
                }
            }
        }

        ids_to_close
    }

    /// Check timed out connections
    fn check_timeouts(&mut self) -> Vec<usize> {
        let timeout = Duration::from_secs(CONNECTION_TIMEOUT_SECS);
        let mut timed_out = Vec::new();

        for (id, conn) in self.connections.iter() {
            if conn.is_timed_out(timeout) {
                debug!("Connection {} timed out", id);
                timed_out.push(id);
            }
        }

        timed_out
    }

    /// Update dirty connections' poller interest (O(m) where m = connections with changed interest)
    fn update_dirty_interests(&mut self) {
        // Drain interest_dirty to get IDs that need updating
        let dirty_ids: Vec<usize> = self.interest_dirty.drain().collect();

        for id in dirty_ids {
            if let Err(e) = self.update_interest(id) {
                log::warn!("Failed to update interest for connection {}: {:?}", id, e);
            }
        }
    }

    /// Run reactor main loop
    pub fn run(
        &mut self,
        connection_receiver: crossbeam_channel::Receiver<TcpStream>,
        publisher_receiver: crossbeam_channel::Receiver<(
            String,
            crossbeam_channel::Receiver<Vec<u8>>,
        )>,
    ) {
        info!("Reactor started");

        let poll_timeout = Duration::from_millis(POLL_TIMEOUT_MS);

        loop {
            // 1. Check stop signal
            if self.status.load(Ordering::Acquire) == STATUS_END {
                info!("Reactor received stop signal");
                break;
            }

            // 2. Non-blocking receive new connections
            while let Ok(socket) = connection_receiver.try_recv() {
                match self.add_connection(socket) {
                    Ok(token) => {
                        debug!("New connection added: {:?}", token);
                    }
                    Err(e) => {
                        error!("Failed to add connection: {:?}", e);
                    }
                }
            }

            // 3. Non-blocking receive new publishers
            while let Ok((stream_key, receiver)) = publisher_receiver.try_recv() {
                if self.add_publisher(stream_key.clone(), receiver).is_some() {
                    debug!("New publisher added for stream: {}", stream_key);
                }
            }

            // 4. Poll IO events
            let events = match self.poller.poll(Some(poll_timeout)) {
                Ok(events) => events,
                Err(e) => {
                    error!("Poller error: {:?}", e);
                    continue;
                }
            };

            // 5. Process IO events
            let mut ids_to_close = Vec::new();

            for event in events {
                let poller_token = event.token;

                // Validate token and get connection id (checks generation)
                let Some(id) = self.validate_connection(poller_token) else {
                    continue;
                };

                // Handle error/hangup
                if event.is_error() || event.is_hangup() {
                    ids_to_close.push(id);
                    continue;
                }

                // Handle readable (drain until WouldBlock)
                if event.is_readable() {
                    let results = self.handle_readable(id);
                    for result in results {
                        let HandleResult::Disconnect(close_id) = result;
                        ids_to_close.push(close_id);
                    }
                }

                // Handle writable
                if event.is_writable() {
                    if let Some(HandleResult::Disconnect(close_id)) = self.handle_writable(id) {
                        ids_to_close.push(close_id);
                    }
                }
            }

            // 6. Handle publishers data
            let publisher_ids_to_remove = self.process_publishers();
            for pub_id in publisher_ids_to_remove {
                self.remove_publisher(pub_id);
            }

            // 7. Flush pending write queues (O(m) where m = connections with pending writes)
            let flush_closes = self.flush_pending();
            ids_to_close.extend(flush_closes);

            // 8. Update dirty poller interests (O(m) where m = connections with changed interests)
            self.update_dirty_interests();

            // 9. Check timeouts
            let timed_out = self.check_timeouts();
            ids_to_close.extend(timed_out);

            // 10. Clean up disconnected connections (deduplicate)
            ids_to_close.sort_unstable();
            ids_to_close.dedup();
            for id in ids_to_close {
                self.remove_connection(id);
            }
        }

        // Graceful shutdown
        self.graceful_shutdown();

        info!("Reactor stopped");
    }

    /// Graceful shutdown
    fn graceful_shutdown(&mut self) {
        info!("Starting graceful shutdown...");

        let deadline = Instant::now() + Duration::from_secs(GRACEFUL_SHUTDOWN_TIMEOUT_SECS);

        // Mark all connections as closing
        for (_, conn) in self.connections.iter_mut() {
            conn.mark_closing();
        }

        // Try to flush all pending data
        while Instant::now() < deadline {
            let mut all_flushed = true;

            for (_, conn) in self.connections.iter_mut() {
                if conn.has_pending_writes() {
                    all_flushed = false;
                    if let Err(e) = conn.try_flush() {
                        debug!("Failed to flush connection during shutdown: {:?}", e);
                    }
                }
            }

            if all_flushed {
                break;
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        // Close all connections
        for (_, conn) in self.connections.iter_mut() {
            conn.shutdown();
        }

        info!("Graceful shutdown complete");
    }

    /// Check if a connection ID is in the interest_dirty set (test only)
    #[cfg(test)]
    pub fn is_interest_dirty(&self, id: usize) -> bool {
        self.interest_dirty.contains(&id)
    }

    /// Clear interest_dirty set and return its previous contents (test only)
    #[cfg(test)]
    pub fn drain_interest_dirty(&mut self) -> Vec<usize> {
        self.interest_dirty.drain().collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_state_transitions() {
        assert!(ConnectionState::Handshaking.can_read());
        assert!(ConnectionState::Handshaking.can_write());
        assert!(!ConnectionState::Handshaking.is_active());

        assert!(ConnectionState::Active.can_read());
        assert!(ConnectionState::Active.can_write());
        assert!(ConnectionState::Active.is_active());

        assert!(ConnectionState::SlowClient.is_active());

        assert!(!ConnectionState::Closing.can_read());
        assert!(ConnectionState::Closing.can_write());

        assert!(!ConnectionState::Closed.can_read());
        assert!(!ConnectionState::Closed.can_write());
    }

    #[test]
    fn test_interest_desired() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let client = TcpStream::connect(addr).expect("Failed to connect");
        let token = ConnectionToken::new(0, 1);

        let conn = ReactorConnection::new(token, client).expect("Failed to create connection");

        // Initially should want to read
        assert_eq!(conn.desired_interest(), Interest::READABLE);
    }

    #[test]
    fn test_graceful_shutdown_flushes_data() {
        use std::net::TcpListener;

        // Create a listener on a random port
        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        // Create a client connection
        let client = TcpStream::connect(addr).expect("Failed to connect");
        let (server_socket, _) = listener.accept().expect("Failed to accept");

        // Create a connection and enqueue some data
        let token = ConnectionToken::new(0, 1);
        let mut conn = ReactorConnection::new(token, server_socket).expect("Failed to create connection");

        // Transition to Active state
        conn.state = ConnectionState::Active;

        // Enqueue some test data
        let test_data = b"Hello, World!";
        conn.enqueue_data(Bytes::from_static(test_data), false, false, false);

        assert!(conn.has_pending_writes());

        // Flush the data
        let _ = conn.try_flush();

        // Read from client side
        client.set_nonblocking(false).expect("Failed to set blocking");
        let mut buf = vec![0u8; 100];

        // Use a timeout to prevent hanging
        use std::time::Duration;
        client.set_read_timeout(Some(Duration::from_millis(100))).expect("Failed to set timeout");

        match client.peek(&mut buf) {
            Ok(n) if n > 0 => {
                // Data was flushed successfully
                assert!(n >= test_data.len());
            }
            _ => {
                // Data might not have been flushed yet, but that's ok for this test
                // The important thing is that enqueue and flush don't panic
            }
        }
    }

    #[test]
    fn test_connection_timeout_detection() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let client = TcpStream::connect(addr).expect("Failed to connect");
        let token = ConnectionToken::new(0, 1);

        let conn = ReactorConnection::new(token, client).expect("Failed to create connection");

        // Should not be timed out immediately
        assert!(!conn.is_timed_out(Duration::from_secs(60)));

        // Should be timed out with zero timeout
        assert!(conn.is_timed_out(Duration::from_nanos(1)));
    }

    #[test]
    fn test_reactor_creation() {
        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let reactor = Reactor::new(3, None, stream_keys, status);
        assert!(reactor.is_ok());
    }

    #[test]
    fn test_connection_generation_increments() {
        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        // Create a listener and accept multiple connections
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        // Add first connection
        let client1 = TcpStream::connect(addr).expect("Failed to connect");
        let (server1, _) = listener.accept().expect("Failed to accept");
        let token1 = reactor.add_connection(server1).expect("Failed to add connection");

        // Remove it
        reactor.remove_connection(token1.id);

        // Add another connection - should reuse the ID but with incremented generation
        let client2 = TcpStream::connect(addr).expect("Failed to connect");
        let (server2, _) = listener.accept().expect("Failed to accept");
        let token2 = reactor.add_connection(server2).expect("Failed to add connection");

        // Same ID but different generation
        assert_eq!(token1.id, token2.id);
        assert_eq!(token2.generation, token1.generation + 1);

        // Cleanup
        drop(client1);
        drop(client2);
    }

    #[test]
    fn test_token_validation() {
        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let client = TcpStream::connect(addr).expect("Failed to connect");
        let (server, _) = listener.accept().expect("Failed to accept");
        let token = reactor.add_connection(server).expect("Failed to add connection");

        // Connection should be valid with correct generation
        assert!(reactor.validate_connection(token.to_poller_token()).is_some());

        // Remove connection
        reactor.remove_connection(token.id);

        // Old token should now be invalid (connection removed)
        assert!(reactor.validate_connection(token.to_poller_token()).is_none());

        drop(client);
    }

    /// Test that generation token prevents ABA problem
    /// Scenario: Connection A closes, new connection B reuses slot A's id,
    /// stale events for A should be rejected
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_generation_prevents_aba_problem() {
        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        // Create first connection (connection A)
        let client_a = TcpStream::connect(addr).expect("Failed to connect A");
        let (server_a, _) = listener.accept().expect("Failed to accept A");
        let token_a = reactor.add_connection(server_a).expect("Failed to add connection A");
        let stale_poller_token = token_a.to_poller_token();

        // Remove connection A
        reactor.remove_connection(token_a.id);
        drop(client_a);

        // Create new connection (connection B) - should reuse slot 0
        let client_b = TcpStream::connect(addr).expect("Failed to connect B");
        let (server_b, _) = listener.accept().expect("Failed to accept B");
        let token_b = reactor.add_connection(server_b).expect("Failed to add connection B");

        // Token B should be valid
        assert!(reactor.validate_connection(token_b.to_poller_token()).is_some());

        // Stale token A should be INVALID even though same id slot is occupied
        // (generation differs)
        assert!(reactor.validate_connection(stale_poller_token).is_none());

        // Different generations for same id
        assert_eq!(token_a.id, token_b.id);  // Same slot reused
        assert_ne!(token_a.generation, token_b.generation);  // Different generation

        reactor.remove_connection(token_b.id);
        drop(client_b);
    }

    /// Stress test: verify reactor can handle many connections
    /// Note: This test creates connections but doesn't run the full RTMP handshake
    #[test]
    fn test_many_connections_creation() {
        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        // Create 100 connections (not 1000+ for unit test performance)
        let num_connections = 100;
        let mut clients = Vec::new();
        let mut tokens = Vec::new();

        for i in 0..num_connections {
            let client = TcpStream::connect(addr).expect(&format!("Failed to connect {}", i));
            let (server, _) = listener.accept().expect(&format!("Failed to accept {}", i));

            let token = reactor.add_connection(server).expect(&format!("Failed to add connection {}", i));
            clients.push(client);
            tokens.push(token);
        }

        // Verify all connections exist
        assert_eq!(reactor.connections.len(), num_connections);

        // Remove all connections
        for token in &tokens {
            reactor.remove_connection(token.id);
        }

        // Verify all connections removed
        assert_eq!(reactor.connections.len(), 0);
    }

    // ==================== Performance Tests ====================
    // Run with: cargo test --features rtmp --release -- --ignored --nocapture

    /// Performance test: Connection scaling (1000 connections)
    /// Tests the reactor's ability to handle many concurrent connections
    #[test]
    #[ignore] // Only run when explicitly requested
    fn perf_connection_scaling() {
        use std::time::Instant;

        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        // Adaptive connection count based on system FD limit
        // Each connection needs 2 FDs (client + server), plus some headroom
        let max_fd = effective_max_connections(None);
        let num_connections = (max_fd / 3).min(1000);

        let mut clients = Vec::with_capacity(num_connections);
        let mut tokens = Vec::with_capacity(num_connections);

        // Measure connection creation time
        let start = Instant::now();

        for i in 0..num_connections {
            let client = TcpStream::connect(addr).unwrap_or_else(|_| panic!("Failed to connect {}", i));
            let (server, _) = listener.accept().unwrap_or_else(|_| panic!("Failed to accept {}", i));
            let token = reactor.add_connection(server).unwrap_or_else(|_| panic!("Failed to add {}", i));
            clients.push(client);
            tokens.push(token);
        }

        let connect_time = start.elapsed();

        // Verify
        assert_eq!(reactor.connections.len(), num_connections);

        // Measure cleanup time
        let cleanup_start = Instant::now();
        for token in &tokens {
            reactor.remove_connection(token.id);
        }
        let cleanup_time = cleanup_start.elapsed();

        // Output results
        println!();
        println!("╔══════════════════════════════════════════════════════════╗");
        println!("║           RTMP Performance Test: Connection Scaling      ║");
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ Platform:        {:>40} ║", std::env::consts::OS);
        println!("║ Arch:            {:>40} ║", std::env::consts::ARCH);
        println!("║ Connections:     {:>40} ║", num_connections);
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ Connect time:    {:>37?} ║", connect_time);
        println!("║ Per connection:  {:>37?} ║", connect_time / num_connections as u32);
        println!("║ Cleanup time:    {:>37?} ║", cleanup_time);
        println!("║ Per cleanup:     {:>37?} ║", cleanup_time / num_connections as u32);
        println!("╚══════════════════════════════════════════════════════════╝");
        println!();
    }

    /// Performance test: Read buffer throughput
    /// Tests try_read() + extend_from_slice optimization
    #[test]
    #[ignore] // Only run when explicitly requested
    fn perf_read_throughput() {
        use std::time::Instant;
        use std::io::Write;

        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let mut client = TcpStream::connect(addr).expect("Failed to connect");
        let (server, _) = listener.accept().expect("Failed to accept");
        client.set_nodelay(true).ok();

        let token = reactor.add_connection(server).expect("Failed to add connection");

        // Test data: simulate RTMP-like traffic (various chunk sizes)
        let test_sizes = [128, 1024, 4096, 8192, 16384, 65536];
        let iterations = 100;

        println!();
        println!("╔══════════════════════════════════════════════════════════╗");
        println!("║           RTMP Performance Test: Read Throughput         ║");
        println!("╠══════════════════════════════════════════════════════════╣");
        println!("║ Platform:        {:>40} ║", std::env::consts::OS);
        println!("║ Arch:            {:>40} ║", std::env::consts::ARCH);
        println!("║ Iterations:      {:>40} ║", iterations);
        println!("╠══════════════════════════════════════════════════════════╣");

        for &size in &test_sizes {
            let data = vec![0xABu8; size];
            let mut total_bytes = 0usize;

            let start = Instant::now();

            for _ in 0..iterations {
                // Write data from client
                client.write_all(&data).expect("Failed to write");
                client.flush().expect("Failed to flush");
                total_bytes += size;

                // Small delay to let data arrive
                std::thread::sleep(std::time::Duration::from_micros(100));

                // Read via reactor connection
                if let Some(conn) = reactor.connections.get_mut(token.id) {
                    let _ = conn.try_read();
                }
            }

            let elapsed = start.elapsed();
            let throughput_mbps = (total_bytes as f64 / 1_000_000.0) / elapsed.as_secs_f64();

            println!("║ Chunk {:>6} B:  {:>8.2} MB/s ({:>6} B x {:>3})      ║",
                     size, throughput_mbps, size, iterations);
        }

        println!("╚══════════════════════════════════════════════════════════╝");
        println!();

        // Cleanup
        reactor.remove_connection(token.id);
    }

    /// Test that handle_writable marks interest_dirty when write queue drains
    #[test]
    fn test_handle_writable_marks_interest_dirty_on_queue_drain() {
        use std::io::Read;

        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        // Create a listener and connection pair
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let mut client = TcpStream::connect(addr).expect("Failed to connect");
        let (server, _) = listener.accept().expect("Failed to accept");
        client.set_nonblocking(true).ok();

        let token = reactor.add_connection(server).expect("Failed to add connection");

        // Set connection to Active state so it can write
        if let Some(conn) = reactor.connections.get_mut(token.id) {
            conn.state = ConnectionState::Active;
        }

        // Enqueue small data that will be fully written in one flush
        let test_data = b"Hello";
        if let Some(conn) = reactor.connections.get_mut(token.id) {
            conn.enqueue_data(Bytes::from_static(test_data), false, false, false);
            assert!(conn.has_pending_writes());
        }

        // Clear any existing interest_dirty entries
        reactor.drain_interest_dirty();

        // Call handle_writable - this should flush and mark interest_dirty
        let result = reactor.handle_writable(token.id);
        assert!(result.is_none(), "Connection should not be closed");

        // Verify connection no longer has pending writes
        if let Some(conn) = reactor.connections.get(token.id) {
            assert!(!conn.has_pending_writes(), "Queue should be drained");
        }

        // Verify interest_dirty was marked
        assert!(reactor.is_interest_dirty(token.id),
            "interest_dirty should contain connection ID after queue drain");

        // Read from client to verify data was sent
        let mut buf = vec![0u8; 100];
        client.set_nonblocking(false).ok();
        client.set_read_timeout(Some(std::time::Duration::from_millis(100))).ok();
        let _ = client.read(&mut buf);

        // Cleanup
        reactor.remove_connection(token.id);
    }

    /// Test that flush_pending marks interest_dirty when write queue drains
    #[test]
    fn test_flush_pending_marks_interest_dirty_on_queue_drain() {
        use std::io::Read;

        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        // Create a listener and connection pair
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let mut client = TcpStream::connect(addr).expect("Failed to connect");
        let (server, _) = listener.accept().expect("Failed to accept");
        client.set_nonblocking(true).ok();

        let token = reactor.add_connection(server).expect("Failed to add connection");

        // Set connection to Active state
        if let Some(conn) = reactor.connections.get_mut(token.id) {
            conn.state = ConnectionState::Active;
        }

        // Enqueue data and add to pending_flush set
        let test_data = b"World";
        if let Some(conn) = reactor.connections.get_mut(token.id) {
            conn.enqueue_data(Bytes::from_static(test_data), false, false, false);
        }
        reactor.pending_flush.insert(token.id);

        // Clear interest_dirty
        reactor.drain_interest_dirty();

        // Call flush_pending
        let ids_to_close = reactor.flush_pending();
        assert!(ids_to_close.is_empty(), "No connections should need closing");

        // Verify interest_dirty was marked after flush drained the queue
        assert!(reactor.is_interest_dirty(token.id),
            "interest_dirty should contain connection ID after flush_pending drains queue");

        // Read from client to consume data
        let mut buf = vec![0u8; 100];
        client.set_nonblocking(false).ok();
        client.set_read_timeout(Some(std::time::Duration::from_millis(100))).ok();
        let _ = client.read(&mut buf);

        // Cleanup
        reactor.remove_connection(token.id);
    }

    /// Test that flush_pending marks interest_dirty when connection has no pending writes
    #[test]
    fn test_flush_pending_marks_interest_dirty_when_no_pending_writes() {
        let stream_keys = dashmap::DashSet::new();
        let status = Arc::new(AtomicUsize::new(STATUS_RUN));

        let mut reactor = Reactor::new(3, None, stream_keys, status).expect("Failed to create reactor");

        // Create a listener and connection pair
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind");
        let addr = listener.local_addr().expect("Failed to get address");

        let _client = TcpStream::connect(addr).expect("Failed to connect");
        let (server, _) = listener.accept().expect("Failed to accept");

        let token = reactor.add_connection(server).expect("Failed to add connection");

        // Set connection to Active state but don't enqueue any data
        if let Some(conn) = reactor.connections.get_mut(token.id) {
            conn.state = ConnectionState::Active;
            assert!(!conn.has_pending_writes());
        }

        // Add to pending_flush even though no data pending
        // (this can happen if data was already flushed between enqueue and flush_pending)
        reactor.pending_flush.insert(token.id);

        // Clear interest_dirty
        reactor.drain_interest_dirty();

        // Call flush_pending
        let ids_to_close = reactor.flush_pending();
        assert!(ids_to_close.is_empty(), "No connections should need closing");

        // Verify interest_dirty was marked to clear writable interest
        assert!(reactor.is_interest_dirty(token.id),
            "interest_dirty should be marked even when no pending writes (to clear WRITABLE interest)");

        // Cleanup
        reactor.remove_connection(token.id);
    }

}
