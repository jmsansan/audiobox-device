use std::fmt;

/// Stable error categories. Match on the code rather than the message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// The system has no output device, or none that can play.
    NoDevice,
    /// No output device has the requested name.
    DeviceNotFound,
    /// The device was disconnected or is no longer available.
    DeviceUnavailable,
    /// The device is in use by another application, perhaps only for a moment.
    DeviceBusy,
    /// The operating system denied access to the device.
    PermissionDenied,
    /// The audio system itself, such as a sound server, is not running.
    HostUnavailable,
    /// The device supports no configuration that the mixer can be played through.
    UnsupportedConfig,
    /// A setting of `DeviceConfig` is out of range.
    InvalidArgument,
    /// The stream failed while it played.
    StreamFailed,
}

/// An error with a stable category and human-readable context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<audiobox::Error> for Error {
    fn from(value: audiobox::Error) -> Self {
        Self::new(ErrorCode::InvalidArgument, value.message)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
