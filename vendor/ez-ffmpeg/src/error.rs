use ffmpeg_next::ffi::AVERROR;
use ffmpeg_sys_next::*;
use std::ffi::NulError;
use std::{io, result};

/// Result type of all ez-ffmpeg library calls.
pub type Result<T, E = Error> = result::Result<T, E>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Scheduler is not started")]
    NotStarted,

    #[error("URL error: {0}")]
    Url(#[from] UrlError),

    #[error("Open input stream error: {0}")]
    OpenInputStream(#[from] OpenInputError),

    #[error("Find stream info error: {0}")]
    FindStream(#[from] FindStreamError),

    #[error("Decoder error: {0}")]
    Decoder(#[from] DecoderError),

    #[error("Filter graph parse error: {0}")]
    FilterGraphParse(#[from] FilterGraphParseError),

    #[error("Ffilter description converted to utf8 string error")]
    FilterDescUtf8,

    #[error("Filter name converted to utf8 string error")]
    FilterNameUtf8,

    #[error("A filtergraph has zero outputs, this is not supported")]
    FilterZeroOutputs,

    #[error("Input is not a valid number")]
    ParseInteger,

    #[error("Alloc output context error: {0}")]
    AllocOutputContext(#[from] AllocOutputContextError),

    #[error("Open output error: {0}")]
    OpenOutput(#[from] OpenOutputError),

    #[error("Output file '{0}' is the same as an input file")]
    FileSameAsInput(String),

    #[error("Find devices error: {0}")]
    FindDevices(#[from] FindDevicesError),

    #[error("Alloc frame error: {0}")]
    AllocFrame(#[from] AllocFrameError),

    #[error("Alloc packet error: {0}")]
    AllocPacket(#[from] AllocPacketError),

    // ---- Muxing ----
    #[error("Muxing operation failed {0}")]
    Muxing(#[from] MuxingOperationError),

    // ---- Open Encoder ----
    #[error("Open encoder operation failed {0}")]
    OpenEncoder(#[from] OpenEncoderOperationError),

    // ---- Encoding ----
    #[error("Encoding operation failed {0}")]
    Encoding(#[from] EncodingOperationError),

    // ---- FilterGraph ----
    #[error("Filter graph operation failed {0}")]
    FilterGraph(#[from] FilterGraphOperationError),

    // ---- Open Decoder ----
    #[error("Open decoder operation failed {0}")]
    OpenDecoder(#[from] OpenDecoderOperationError),

    // ---- Decoding ----
    #[error("Decoding operation failed {0}")]
    Decoding(#[from] DecodingOperationError),

    // ---- Demuxing ----
    #[error("Demuxing operation failed {0}")]
    Demuxing(#[from] DemuxingOperationError),

    // ---- Frame Filter ----
    #[error("Frame filter init failed: {0}")]
    FrameFilterInit(String),

    #[error("Frame filter process failed: {0}")]
    FrameFilterProcess(String),

    #[error("Frame filter request failed: {0}")]
    FrameFilterRequest(String),

    #[error("No {0} stream of the type:{1} were found while build frame pipeline")]
    FrameFilterTypeNoMatched(String, String),

    #[error("{0} stream:{1} of the type:{2} were mismatched while build frame pipeline")]
    FrameFilterStreamTypeNoMatched(String, usize, String),

    #[error("Frame filter pipeline destination already finished")]
    FrameFilterDstFinished,

    #[error("Frame filter pipeline send frame failed, out of memory")]
    FrameFilterSendOOM,

    #[error("Frame filter pipeline thread exited")]
    FrameFilterThreadExited,

    #[cfg(feature = "rtmp")]
    #[error("Rtmp stream already exists with key: {0}")]
    RtmpStreamAlreadyExists(String),

    #[cfg(feature = "rtmp")]
    #[error("Rtmp create stream failed. Check whether the server is stopped.")]
    RtmpCreateStream,

    #[cfg(feature = "rtmp")]
    #[error("Rtmp server session error: {0}")]
    RtmpServerSession(#[from] rml_rtmp::sessions::ServerSessionError),

    #[cfg(feature = "rtmp")]
    #[error("Rtmp server thread exited")]
    RtmpThreadExited,

    #[error("IO error:{0}")]
    IO(#[from] io::Error),

    #[error("EOF")]
    EOF,
    #[error("Exit")]
    Exit,
    #[error("Bug")]
    Bug,
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Error::EOF, Error::EOF) => true,
            (Error::Exit, Error::Exit) => true,
            (Error::Bug, Error::Bug) => true,
            _ => false,
        }
    }
}

impl Eq for Error {}

#[derive(thiserror::Error, Debug)]
pub enum DemuxingOperationError {
    #[error("while reading frame: {0}")]
    ReadFrameError(DemuxingError),

    #[error("while referencing packet: {0}")]
    PacketRefError(DemuxingError),

    #[error("while seeking file: {0}")]
    SeekFileError(DemuxingError),

    #[error("Thread exited")]
    ThreadExited,
}

#[derive(thiserror::Error, Debug)]
pub enum DecodingOperationError {
    #[error("during frame reference creation: {0}")]
    FrameRefError(DecodingError),

    #[error("during frame properties copy: {0}")]
    FrameCopyPropsError(DecodingError),

    #[error("during subtitle decoding: {0}")]
    DecodeSubtitleError(DecodingError),

    #[error("during subtitle copy: {0}")]
    CopySubtitleError(DecodingError),

    #[error("during packet submission to decoder: {0}")]
    SendPacketError(DecodingError),

    #[error("during frame reception from decoder: {0}")]
    ReceiveFrameError(DecodingError),

    #[error("during frame allocation: {0}")]
    FrameAllocationError(DecodingError),

    #[error("during packet allocation: {0}")]
    PacketAllocationError(DecodingError),

    #[error("during AVSubtitle allocation: {0}")]
    SubtitleAllocationError(DecodingError),

    #[error("corrupt decoded frame")]
    CorruptFrame,

    #[error("during retrieve data on hw: {0}")]
    HWRetrieveDataError(DecodingError),

    #[error("during cropping: {0}")]
    CroppingError(DecodingError),
}

#[derive(thiserror::Error, Debug)]
pub enum OpenDecoderOperationError {
    #[error("during context allocation: {0}")]
    ContextAllocationError(OpenDecoderError),

    #[error("while applying parameters to context: {0}")]
    ParameterApplicationError(OpenDecoderError),

    #[error("while opening decoder: {0}")]
    DecoderOpenError(OpenDecoderError),

    #[error("while copying channel layout: {0}")]
    ChannelLayoutCopyError(OpenDecoderError),

    #[error("while Hw setup: {0}")]
    HwSetupError(OpenDecoderError),

    #[error("Invalid decoder name")]
    InvalidName,

    #[error("Thread exited")]
    ThreadExited,
}

#[derive(thiserror::Error, Debug)]
pub enum FilterGraphOperationError {
    #[error("during requesting oldest frame: {0}")]
    RequestOldestError(FilterGraphError),

    #[error("during process frames: {0}")]
    ProcessFramesError(FilterGraphError),

    #[error("during send frames: {0}")]
    SendFramesError(FilterGraphError),

    #[error("during copying channel layout: {0}")]
    ChannelLayoutCopyError(FilterGraphError),

    #[error("during buffer source add frame: {0}")]
    BufferSourceAddFrameError(FilterGraphError),

    #[error("during closing buffer source: {0}")]
    BufferSourceCloseError(FilterGraphError),

    #[error("during replace buffer: {0}")]
    BufferReplaceoseError(FilterGraphError),

    #[error("during parse: {0}")]
    ParseError(FilterGraphParseError),

    #[error("The data in the frame is invalid or corrupted")]
    InvalidData,

    #[error("Thread exited")]
    ThreadExited,
}

#[derive(thiserror::Error, Debug)]
pub enum EncodingOperationError {
    #[error("during frame submission: {0}")]
    SendFrameError(EncodingError),

    #[error("during packet retrieval: {0}")]
    ReceivePacketError(EncodingError),

    #[error("during audio frame receive: {0}")]
    ReceiveAudioError(EncodingError),

    #[error(": Subtitle packets must have a pts")]
    SubtitleNotPts,

    #[error(": Muxer already finished")]
    MuxerFinished,

    #[error("Encode subtitle error: {0}")]
    EncodeSubtitle(#[from] EncodeSubtitleError),

    #[error(": {0}")]
    AllocPacket(AllocPacketError),
}

#[derive(thiserror::Error, Debug)]
pub enum MuxingOperationError {
    #[error("during write header: {0}")]
    WriteHeader(WriteHeaderError),

    #[error("during interleaved write: {0}")]
    InterleavedWriteError(MuxingError),

    #[error("during trailer write: {0}")]
    TrailerWriteError(MuxingError),

    #[error("during closing IO: {0}")]
    IOCloseError(MuxingError),

    #[error("Thread exited")]
    ThreadExited,
}

#[derive(thiserror::Error, Debug)]
pub enum OpenEncoderOperationError {
    #[error("during frame side data cloning: {0}")]
    FrameSideDataCloneError(OpenEncoderError),

    #[error("during channel layout copying: {0}")]
    ChannelLayoutCopyError(OpenEncoderError),

    #[error("during codec opening: {0}")]
    CodecOpenError(OpenEncoderError),

    #[error("while setting codec parameters: {0}")]
    CodecParametersError(OpenEncoderError),

    #[error(": unknown format of the frame")]
    UnknownFrameFormat,

    #[error("while setting subtitle: {0}")]
    SettingSubtitleError(OpenEncoderError),

    #[error("while Hw setup: {0}")]
    HwSetupError(OpenEncoderError),

    #[error("during context allocation: {0}")]
    ContextAllocationError(OpenEncoderError),

    #[error("Thread exited")]
    ThreadExited,
}

#[derive(thiserror::Error, Debug)]
pub enum UrlError {
    #[error("Null byte found in string at position {0}")]
    NullByteError(usize),
}

impl From<NulError> for Error {
    fn from(err: NulError) -> Self {
        Error::Url(UrlError::NullByteError(err.nul_position()))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum OpenInputError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("File or stream not found")]
    NotFound,

    #[error("I/O error occurred while opening the file or stream")]
    IOError,

    #[error("Pipe error, possibly the stream or data connection was broken")]
    PipeError,

    #[error("Invalid file descriptor")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported input format")]
    NotImplemented,

    #[error("Operation not permitted to access the file or stream")]
    OperationNotPermitted,

    #[error("The data in the file or stream is invalid or corrupted")]
    InvalidData,

    #[error("The connection timed out while trying to open the stream")]
    Timeout,

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),

    #[error("Invalid source provided")]
    InvalidSource,

    #[error("Invalid source format:{0}")]
    InvalidFormat(String),

    #[error("No seek callback is provided")]
    SeekFunctionMissing,
}

impl From<i32> for OpenInputError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => OpenInputError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => OpenInputError::InvalidArgument,
            AVERROR_NOT_FOUND => OpenInputError::NotFound,
            AVERROR_IO_ERROR => OpenInputError::IOError,
            AVERROR_PIPE_ERROR => OpenInputError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => OpenInputError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => OpenInputError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => OpenInputError::OperationNotPermitted,
            AVERROR_INVALIDDATA => OpenInputError::InvalidData,
            AVERROR_TIMEOUT => OpenInputError::Timeout,
            _ => OpenInputError::UnknownError(err_code),
        }
    }
}

const AVERROR_OUT_OF_MEMORY: i32 = AVERROR(ENOMEM);
const AVERROR_INVALID_ARGUMENT: i32 = AVERROR(EINVAL);
const AVERROR_NOT_FOUND: i32 = AVERROR(ENOENT);
const AVERROR_IO_ERROR: i32 = AVERROR(EIO);
const AVERROR_PIPE_ERROR: i32 = AVERROR(EPIPE);
const AVERROR_BAD_FILE_DESCRIPTOR: i32 = AVERROR(EBADF);
const AVERROR_NOT_IMPLEMENTED: i32 = AVERROR(ENOSYS);
const AVERROR_OPERATION_NOT_PERMITTED: i32 = AVERROR(EPERM);
const AVERROR_PERMISSION_DENIED: i32 = AVERROR(EACCES);
const AVERROR_TIMEOUT: i32 = AVERROR(ETIMEDOUT);
const AVERROR_NOT_SOCKET: i32 = AVERROR(ENOTSOCK);
const AVERROR_AGAIN: i32 = AVERROR(EAGAIN);

#[derive(thiserror::Error, Debug)]
pub enum FindStreamError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("Reached end of file while looking for stream info")]
    EndOfFile,

    #[error("Timeout occurred while reading stream info")]
    Timeout,

    #[error("I/O error occurred while reading stream info")]
    IOError,

    #[error("The data in the stream is invalid or corrupted")]
    InvalidData,

    #[error("Functionality not implemented or unsupported stream format")]
    NotImplemented,

    #[error("Operation not permitted to access the file or stream")]
    OperationNotPermitted,

    #[error("No Stream found")]
    NoStreamFound,

    #[error("No codec parameters found")]
    NoCodecparFound,

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for FindStreamError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => FindStreamError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => FindStreamError::InvalidArgument,
            AVERROR_EOF => FindStreamError::EndOfFile,
            AVERROR_TIMEOUT => FindStreamError::Timeout,
            AVERROR_IO_ERROR => FindStreamError::IOError,
            AVERROR_INVALIDDATA => FindStreamError::InvalidData,
            AVERROR_NOT_IMPLEMENTED => FindStreamError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => FindStreamError::OperationNotPermitted,
            _ => FindStreamError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum FilterGraphParseError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("End of file reached during parsing")]
    EndOfFile,

    #[error("I/O error occurred during parsing")]
    IOError,

    #[error("Invalid data encountered during parsing")]
    InvalidData,

    #[error("Functionality not implemented or unsupported filter format")]
    NotImplemented,

    #[error("Permission denied during filter graph parsing")]
    PermissionDenied,

    #[error("Socket operation on non-socket during filter graph parsing")]
    NotSocket,

    #[error("Option not found during filter graph configuration")]
    OptionNotFound,

    #[error("Invalid file index {0} in filtergraph description {1}")]
    InvalidFileIndexInFg(usize, String),

    #[error("Invalid file index {0} in output url: {1}")]
    InvalidFileIndexInOutput(usize, String),

    #[error("Invalid filter specifier {0}")]
    InvalidFilterSpecifier(String),

    #[error("Filter '{0}' has output {1} ({2}) unconnected")]
    OutputUnconnected(String, usize, String),

    #[error("An unknown error occurred. ret: {0}")]
    UnknownError(i32),
}

impl From<i32> for FilterGraphParseError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => FilterGraphParseError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => FilterGraphParseError::InvalidArgument,
            AVERROR_EOF => FilterGraphParseError::EndOfFile,
            AVERROR_IO_ERROR => FilterGraphParseError::IOError,
            AVERROR_INVALIDDATA => FilterGraphParseError::InvalidData,
            AVERROR_NOT_IMPLEMENTED => FilterGraphParseError::NotImplemented,
            AVERROR_OPTION_NOT_FOUND => FilterGraphParseError::OptionNotFound,
            _ => FilterGraphParseError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum AllocOutputContextError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("File or stream not found")]
    NotFound,

    #[error("I/O error occurred while allocating the output context")]
    IOError,

    #[error("Pipe error, possibly the stream or data connection was broken")]
    PipeError,

    #[error("Invalid file descriptor")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported output format")]
    NotImplemented,

    #[error("Operation not permitted to allocate the output context")]
    OperationNotPermitted,

    #[error("Permission denied while allocating the output context")]
    PermissionDenied,

    #[error("The connection timed out while trying to allocate the output context")]
    Timeout,

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for AllocOutputContextError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => AllocOutputContextError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => AllocOutputContextError::InvalidArgument,
            AVERROR_NOT_FOUND => AllocOutputContextError::NotFound,
            AVERROR_IO_ERROR => AllocOutputContextError::IOError,
            AVERROR_PIPE_ERROR => AllocOutputContextError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => AllocOutputContextError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => AllocOutputContextError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => AllocOutputContextError::OperationNotPermitted,
            AVERROR_PERMISSION_DENIED => AllocOutputContextError::PermissionDenied,
            AVERROR_TIMEOUT => AllocOutputContextError::Timeout,
            _ => AllocOutputContextError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum OpenOutputError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("File or stream not found")]
    NotFound,

    #[error("I/O error occurred while opening the file or stream")]
    IOError,

    #[error("Pipe error, possibly the stream or data connection was broken")]
    PipeError,

    #[error("Invalid file descriptor")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported output format")]
    NotImplemented,

    #[error("Operation not permitted to open the file or stream")]
    OperationNotPermitted,

    #[error("Permission denied while opening the file or stream")]
    PermissionDenied,

    #[error("The connection timed out while trying to open the file or stream")]
    Timeout,

    #[error("encoder not found")]
    EncoderNotFound,

    #[error("Stream map '{0}' matches no streams;")]
    MatchesNoStreams(String),

    #[error("Invalid label {0}")]
    InvalidLabel(String),

    #[error("not contain any stream")]
    NotContainStream,

    #[error("unknown format of the frame")]
    UnknownFrameFormat,

    #[error("Invalid file index {0} in input url: {1}")]
    InvalidFileIndexInIntput(usize, String),

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),

    #[error("Invalid sink provided")]
    InvalidSink,

    #[error("No seek callback is provided")]
    SeekFunctionMissing,

    #[error("Format '{0}' is unsupported")]
    FormatUnsupported(String),
}

impl From<i32> for OpenOutputError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => OpenOutputError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => OpenOutputError::InvalidArgument,
            AVERROR_NOT_FOUND => OpenOutputError::NotFound,
            AVERROR_IO_ERROR => OpenOutputError::IOError,
            AVERROR_PIPE_ERROR => OpenOutputError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => OpenOutputError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => OpenOutputError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => OpenOutputError::OperationNotPermitted,
            AVERROR_PERMISSION_DENIED => OpenOutputError::PermissionDenied,
            AVERROR_TIMEOUT => OpenOutputError::Timeout,
            AVERROR_ENCODER_NOT_FOUND => OpenOutputError::EncoderNotFound,
            _ => OpenOutputError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum FindDevicesError {
    #[error("AVCaptureDevice class not found in macOS")]
    AVCaptureDeviceNotFound,

    #[error("current media_type({0}) is not supported")]
    MediaTypeSupported(i32),
    #[error("current OS is not supported")]
    OsNotSupported,
    #[error("device_description can not to string")]
    UTF8Error,

    #[error("Memory allocation error")]
    OutOfMemory,
    #[error("Invalid argument provided")]
    InvalidArgument,
    #[error("Device or stream not found")]
    NotFound,
    #[error("I/O error occurred while accessing the device or stream")]
    IOError,
    #[error("Operation not permitted for this device or stream")]
    OperationNotPermitted,
    #[error("Permission denied while accessing the device or stream")]
    PermissionDenied,
    #[error("This functionality is not implemented")]
    NotImplemented,
    #[error("Bad file descriptor")]
    BadFileDescriptor,
    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for FindDevicesError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => FindDevicesError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => FindDevicesError::InvalidArgument,
            AVERROR_NOT_FOUND => FindDevicesError::NotFound,
            AVERROR_IO_ERROR => FindDevicesError::IOError,
            AVERROR_OPERATION_NOT_PERMITTED => FindDevicesError::OperationNotPermitted,
            AVERROR_PERMISSION_DENIED => FindDevicesError::PermissionDenied,
            AVERROR_NOT_IMPLEMENTED => FindDevicesError::NotImplemented,
            AVERROR_BAD_FILE_DESCRIPTOR => FindDevicesError::BadFileDescriptor,
            _ => FindDevicesError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WriteHeaderError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("File or stream not found")]
    NotFound,

    #[error("I/O error occurred while writing the header")]
    IOError,

    #[error("Pipe error, possibly the stream or data connection was broken")]
    PipeError,

    #[error("Invalid file descriptor")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported output format")]
    NotImplemented,

    #[error("Operation not permitted to write the header")]
    OperationNotPermitted,

    #[error("Permission denied while writing the header")]
    PermissionDenied,

    #[error("The connection timed out while trying to write the header")]
    Timeout,

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for WriteHeaderError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => WriteHeaderError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => WriteHeaderError::InvalidArgument,
            AVERROR_NOT_FOUND => WriteHeaderError::NotFound,
            AVERROR_IO_ERROR => WriteHeaderError::IOError,
            AVERROR_PIPE_ERROR => WriteHeaderError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => WriteHeaderError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => WriteHeaderError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => WriteHeaderError::OperationNotPermitted,
            AVERROR_PERMISSION_DENIED => WriteHeaderError::PermissionDenied,
            AVERROR_TIMEOUT => WriteHeaderError::Timeout,
            _ => WriteHeaderError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WriteFrameError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("Reached end of file while writing data")]
    EndOfFile,

    #[error("Timeout occurred while writing data")]
    Timeout,

    #[error("I/O error occurred while writing data")]
    IOError,

    #[error("Bad file descriptor")]
    BadFileDescriptor,

    #[error("Pipe error occurred")]
    PipeError,

    #[error("Functionality not implemented or unsupported operation")]
    NotImplemented,

    #[error("Operation not permitted")]
    OperationNotPermitted,

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Not a valid socket")]
    NotSocket,

    #[error("An unknown error occurred. ret: {0}")]
    UnknownError(i32),
}

impl From<i32> for WriteFrameError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => WriteFrameError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => WriteFrameError::InvalidArgument,
            AVERROR_EOF => WriteFrameError::EndOfFile,
            AVERROR_TIMEOUT => WriteFrameError::Timeout,
            AVERROR_IO_ERROR => WriteFrameError::IOError,
            AVERROR_BAD_FILE_DESCRIPTOR => WriteFrameError::BadFileDescriptor,
            AVERROR_PIPE_ERROR => WriteFrameError::PipeError,
            AVERROR_NOT_IMPLEMENTED => WriteFrameError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => WriteFrameError::OperationNotPermitted,
            AVERROR_PERMISSION_DENIED => WriteFrameError::PermissionDenied,
            AVERROR_NOT_SOCKET => WriteFrameError::NotSocket,
            _ => WriteFrameError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum EncodeSubtitleError {
    #[error("Memory allocation error while encoding subtitle")]
    OutOfMemory,

    #[error("Invalid argument provided for subtitle encoding")]
    InvalidArgument,

    #[error("Operation not permitted while encoding subtitle")]
    OperationNotPermitted,

    #[error("The encoding functionality is not implemented or unsupported")]
    NotImplemented,

    #[error("Encoder temporarily unable to process, please retry")]
    TryAgain,

    #[error("Subtitle encoding failed with unknown error. ret: {0}")]
    UnknownError(i32),
}

impl From<i32> for EncodeSubtitleError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => EncodeSubtitleError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => EncodeSubtitleError::InvalidArgument,
            AVERROR_OPERATION_NOT_PERMITTED => EncodeSubtitleError::OperationNotPermitted,
            AVERROR_NOT_IMPLEMENTED => EncodeSubtitleError::NotImplemented,
            AVERROR_AGAIN => EncodeSubtitleError::TryAgain,
            _ => EncodeSubtitleError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum AllocPacketError {
    #[error("Memory allocation error while alloc packet")]
    OutOfMemory,
}

#[derive(thiserror::Error, Debug)]
pub enum AllocFrameError {
    #[error("Memory allocation error while alloc frame")]
    OutOfMemory,
}

#[derive(thiserror::Error, Debug)]
pub enum MuxingError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("I/O error occurred during muxing")]
    IOError,

    #[error("Broken pipe during muxing")]
    PipeError,

    #[error("Bad file descriptor encountered")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported")]
    NotImplemented,

    #[error("Operation not permitted")]
    OperationNotPermitted,

    #[error("Resource temporarily unavailable")]
    TryAgain,

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for MuxingError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => MuxingError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => MuxingError::InvalidArgument,
            AVERROR_IO_ERROR => MuxingError::IOError,
            AVERROR_PIPE_ERROR => MuxingError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => MuxingError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => MuxingError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => MuxingError::OperationNotPermitted,
            AVERROR_AGAIN => MuxingError::TryAgain,
            _ => MuxingError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum OpenEncoderError {
    #[error("Memory allocation error occurred during encoder initialization")]
    OutOfMemory,

    #[error("Invalid argument provided to encoder")]
    InvalidArgument,

    #[error("I/O error occurred while opening encoder")]
    IOError,

    #[error("Broken pipe encountered during encoder initialization")]
    PipeError,

    #[error("Bad file descriptor used in encoder")]
    BadFileDescriptor,

    #[error("Encoder functionality not implemented or unsupported")]
    NotImplemented,

    #[error("Operation not permitted while configuring encoder")]
    OperationNotPermitted,

    #[error("Resource temporarily unavailable during encoder setup")]
    TryAgain,

    #[error("An unknown error occurred in encoder setup. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for OpenEncoderError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => OpenEncoderError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => OpenEncoderError::InvalidArgument,
            AVERROR_IO_ERROR => OpenEncoderError::IOError,
            AVERROR_PIPE_ERROR => OpenEncoderError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => OpenEncoderError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => OpenEncoderError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => OpenEncoderError::OperationNotPermitted,
            AVERROR_AGAIN => OpenEncoderError::TryAgain,
            _ => OpenEncoderError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum EncodingError {
    #[error("Memory allocation error during encoding")]
    OutOfMemory,

    #[error("Invalid argument provided to encoder")]
    InvalidArgument,

    #[error("I/O error occurred during encoding")]
    IOError,

    #[error("Broken pipe encountered during encoding")]
    PipeError,

    #[error("Bad file descriptor encountered during encoding")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported encoding feature")]
    NotImplemented,

    #[error("Operation not permitted for encoder")]
    OperationNotPermitted,

    #[error("Resource temporarily unavailable, try again later")]
    TryAgain,

    #[error("End of stream reached or no more frames to encode")]
    EndOfStream,

    #[error("An unknown error occurred during encoding. ret: {0}")]
    UnknownError(i32),
}

impl From<i32> for EncodingError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => EncodingError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => EncodingError::InvalidArgument,
            AVERROR_IO_ERROR => EncodingError::IOError,
            AVERROR_PIPE_ERROR => EncodingError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => EncodingError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => EncodingError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => EncodingError::OperationNotPermitted,
            AVERROR_AGAIN => EncodingError::TryAgain,
            AVERROR_EOF => EncodingError::EndOfStream,
            _ => EncodingError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum FilterGraphError {
    #[error("Memory allocation error during filter graph processing")]
    OutOfMemory,

    #[error("Invalid argument provided to filter graph processing")]
    InvalidArgument,

    #[error("I/O error occurred during filter graph processing")]
    IOError,

    #[error("Broken pipe during filter graph processing")]
    PipeError,

    #[error("Bad file descriptor encountered during filter graph processing")]
    BadFileDescriptor,

    #[error("Functionality not implemented or unsupported during filter graph processing")]
    NotImplemented,

    #[error("Operation not permitted during filter graph processing")]
    OperationNotPermitted,

    #[error("Resource temporarily unavailable during filter graph processing")]
    TryAgain,

    #[error("EOF")]
    EOF,

    #[error("An unknown error occurred during filter graph processing. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for FilterGraphError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => FilterGraphError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => FilterGraphError::InvalidArgument,
            AVERROR_IO_ERROR => FilterGraphError::IOError,
            AVERROR_PIPE_ERROR => FilterGraphError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => FilterGraphError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => FilterGraphError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => FilterGraphError::OperationNotPermitted,
            AVERROR_AGAIN => FilterGraphError::TryAgain,
            AVERROR_EOF => FilterGraphError::EOF,
            _ => FilterGraphError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum OpenDecoderError {
    #[error("Memory allocation error during decoder initialization")]
    OutOfMemory,

    #[error("Invalid argument provided during decoder initialization")]
    InvalidArgument,

    #[error("Functionality not implemented or unsupported during decoder initialization")]
    NotImplemented,

    #[error("Resource temporarily unavailable during decoder initialization")]
    TryAgain,

    #[error("I/O error occurred during decoder initialization")]
    IOError,

    #[error("An unknown error occurred during decoder initialization: {0}")]
    UnknownError(i32),
}

impl From<i32> for OpenDecoderError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => OpenDecoderError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => OpenDecoderError::InvalidArgument,
            AVERROR_NOT_IMPLEMENTED => OpenDecoderError::NotImplemented,
            AVERROR_AGAIN => OpenDecoderError::TryAgain,
            AVERROR_IO_ERROR => OpenDecoderError::IOError,
            _ => OpenDecoderError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DecodingError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("I/O error occurred during decoding")]
    IOError,

    #[error("Timeout occurred during decoding")]
    Timeout,

    #[error("Broken pipe encountered during decoding")]
    PipeError,

    #[error("Bad file descriptor encountered during decoding")]
    BadFileDescriptor,

    #[error("Unsupported functionality or format encountered")]
    NotImplemented,

    #[error("Operation not permitted")]
    OperationNotPermitted,

    #[error("Resource temporarily unavailable")]
    TryAgain,

    #[error("An unknown decoding error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for DecodingError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => DecodingError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => DecodingError::InvalidArgument,
            AVERROR_IO_ERROR => DecodingError::IOError,
            AVERROR_TIMEOUT => DecodingError::Timeout,
            AVERROR_PIPE_ERROR => DecodingError::PipeError,
            AVERROR_BAD_FILE_DESCRIPTOR => DecodingError::BadFileDescriptor,
            AVERROR_NOT_IMPLEMENTED => DecodingError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => DecodingError::OperationNotPermitted,
            AVERROR_AGAIN => DecodingError::TryAgain,
            _ => DecodingError::UnknownError(err_code),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DecoderError {
    #[error("not found.")]
    NotFound,
}

#[derive(thiserror::Error, Debug)]
pub enum DemuxingError {
    #[error("Memory allocation error")]
    OutOfMemory,

    #[error("Invalid argument provided")]
    InvalidArgument,

    #[error("I/O error occurred during demuxing")]
    IOError,

    #[error("End of file reached during demuxing")]
    EndOfFile,

    #[error("Resource temporarily unavailable")]
    TryAgain,

    #[error("Functionality not implemented or unsupported")]
    NotImplemented,

    #[error("Operation not permitted")]
    OperationNotPermitted,

    #[error("Bad file descriptor encountered")]
    BadFileDescriptor,

    #[error("Invalid data found when processing input")]
    InvalidData,

    #[error("An unknown error occurred. ret:{0}")]
    UnknownError(i32),
}

impl From<i32> for DemuxingError {
    fn from(err_code: i32) -> Self {
        match err_code {
            AVERROR_OUT_OF_MEMORY => DemuxingError::OutOfMemory,
            AVERROR_INVALID_ARGUMENT => DemuxingError::InvalidArgument,
            AVERROR_IO_ERROR => DemuxingError::IOError,
            AVERROR_EOF => DemuxingError::EndOfFile,
            AVERROR_AGAIN => DemuxingError::TryAgain,
            AVERROR_NOT_IMPLEMENTED => DemuxingError::NotImplemented,
            AVERROR_OPERATION_NOT_PERMITTED => DemuxingError::OperationNotPermitted,
            AVERROR_BAD_FILE_DESCRIPTOR => DemuxingError::BadFileDescriptor,
            AVERROR_INVALIDDATA => DemuxingError::InvalidData,
            _ => DemuxingError::UnknownError(err_code),
        }
    }
}
