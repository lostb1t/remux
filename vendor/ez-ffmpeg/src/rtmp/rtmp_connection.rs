use log::{debug, warn};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use std::io;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::thread;

const BUFFER_SIZE: usize = 4096;

#[derive(Debug)]
pub(super) enum ReadResult {
    HandshakingInProgress,
    NoBytesReceived,
    BytesReceived {
        buffer: [u8; BUFFER_SIZE],
        byte_count: usize,
    },
}

#[derive(Debug)]
pub(super) enum ConnectionError {
    IoError(io::Error),
    SocketClosed,
}

impl From<io::Error> for ConnectionError {
    fn from(error: io::Error) -> Self {
        ConnectionError::IoError(error)
    }
}

pub(super) struct RtmpConnection {
    pub(super) connection_id: usize,
    writer: crossbeam_channel::Sender<Vec<u8>>,
    reader: crossbeam_channel::Receiver<ReadResult>,
    handshake: Handshake,
    handshake_completed: bool,
}

impl RtmpConnection {
    pub(super) fn new(connection_id: usize, socket: TcpStream) -> io::Result<Self> {

        let (byte_sender, byte_receiver) = crossbeam_channel::bounded(1024);
        let (result_sender, result_receiver) = crossbeam_channel::unbounded();

        start_byte_writer(byte_receiver, &socket)?;
        start_result_reader(result_sender, &socket)?;

        Ok(RtmpConnection {
            connection_id,
            writer: byte_sender,
            reader: result_receiver,
            handshake: Handshake::new(PeerType::Server),
            handshake_completed: false,
        })
    }

    pub(super) fn write(&self, bytes: Vec<u8>) {
        if let Err(e) = self.writer.try_send(bytes) {
            if e.is_full() {
                warn!("Connection {} client buffer full, dropping packet.", self.connection_id);
                return
            }
            if e.is_disconnected() {
                debug!("Connection {} receiver disconnected.", self.connection_id);
            }
        }
    }

    pub(super) fn read(&mut self) -> Result<ReadResult, ConnectionError> {
        match self.reader.try_recv() {
            Err(crossbeam_channel::TryRecvError::Empty) => Ok(ReadResult::NoBytesReceived),
            Err(crossbeam_channel::TryRecvError::Disconnected) => Err(ConnectionError::SocketClosed),
            Ok(result) => match self.handshake_completed {
                true => Ok(result),
                false => match result {
                    ReadResult::HandshakingInProgress => unreachable!(),
                    ReadResult::NoBytesReceived => Ok(result),
                    ReadResult::BytesReceived { buffer, byte_count } => {
                        self.handle_handshake_bytes(&buffer[..byte_count])
                    }
                },
            },
        }
    }

    fn handle_handshake_bytes(&mut self, bytes: &[u8]) -> Result<ReadResult, ConnectionError> {
        let result = match self.handshake.process_bytes(bytes) {
            Ok(result) => result,
            Err(error) => {
                debug!("Rtmp client handshake error: {:?}", error);
                return Err(ConnectionError::SocketClosed);
            }
        };

        match result {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if response_bytes.len() > 0 {
                    self.write(response_bytes);
                }

                Ok(ReadResult::HandshakingInProgress)
            }

            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                debug!("Rtmp client handshake successful!");
                if response_bytes.len() > 0 {
                    self.write(response_bytes);
                }

                let mut buffer = [0; BUFFER_SIZE];
                let buffer_size = remaining_bytes.len();
                buffer[..buffer_size].copy_from_slice(&remaining_bytes);

                self.handshake_completed = true;
                Ok(ReadResult::BytesReceived {
                    buffer,
                    byte_count: buffer_size,
                })
            }
        }
    }
}

fn start_byte_writer(byte_receiver: crossbeam_channel::Receiver<Vec<u8>>, socket: &TcpStream) -> io::Result<()>{
    let mut socket = socket.try_clone()?;
    thread::spawn(move || {
        while let Ok(bytes) = byte_receiver.recv() {
            loop {
                if let Err(e) = socket.write_all(&bytes) {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    } else {
                        debug!("Rtmp client error writing to socket: {:?}", e);
                        break;
                    }
                }
                break;
            }

        }
        socket
            .shutdown(Shutdown::Write)
            .expect("failed to shutdown socket (rtmp client write)");
    });
    Ok(())
}

fn start_result_reader(sender: crossbeam_channel::Sender<ReadResult>, socket: &TcpStream) -> io::Result<()> {
    let mut socket = socket.try_clone()?;
    thread::spawn(move || {
        loop {
            let mut buffer = [0; BUFFER_SIZE];
            match socket.read(&mut buffer) {
                Ok(0) => return, // socket closed
                Ok(read_count) => {
                    let result = ReadResult::BytesReceived {
                        buffer,
                        byte_count: read_count,
                    };

                    if let Err(e) = sender.send(result) {
                        // receiver has been dropped
                        return;
                    }
                }

                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        thread::sleep(std::time::Duration::from_millis(100));
                    } else {
                        debug!("Rtmp client error occurred reading from socket: {:?}", e);
                        return;
                    }
                }
            }
        }
    });
    Ok(())
}
