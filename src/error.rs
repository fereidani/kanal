#![forbid(unsafe_code)]
use core::{fmt, fmt::Debug};

/// Error type for channel send operations without timeout
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

impl<T> Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SendError(..)")
    }
}
impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt("send to a closed channel", f)
    }
}

impl<T> SendError<T> {
    /// Consumes the error and returns the contained value.
    #[inline]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::error::Error for SendError<T> {}

/// Error type for channel send operations with timeout
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SendTimeoutError<T> {
    /// Indicates that the channel is closed on both sides with a call to
    /// `close()`
    Closed(T),
    /// Indicates that channel operation reached timeout and is canceled
    Timeout(T),
}

impl<T> Debug for SendTimeoutError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SendTimeoutError::Closed(..) => write!(f, "Closed(..)"),
            SendTimeoutError::Timeout(..) => write!(f, "Timeout(..)"),
        }
    }
}
impl<T> fmt::Display for SendTimeoutError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                SendTimeoutError::Closed(..) => "send to a closed channel",
                SendTimeoutError::Timeout(..) => "send timeout",
            },
            f,
        )
    }
}

impl<T> SendTimeoutError<T> {
    /// Consumes the error and returns the contained value.
    #[inline]
    pub fn into_inner(self) -> T {
        match self {
            SendTimeoutError::Closed(value) => value,
            SendTimeoutError::Timeout(value) => value,
        }
    }
}

impl<T> std::error::Error for SendTimeoutError<T> {}

/// Error type for channel receive operations without timeout
#[derive(Debug, PartialEq, Eq)]
pub struct ReceiveError();
impl core::error::Error for ReceiveError {}
impl fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt("receive from a closed channel", f)
    }
}

/// Error type for channel receive operations with timeout
#[derive(Debug, PartialEq, Eq)]
pub enum ReceiveErrorTimeout {
    /// Indicates that the channel is closed on both sides with a call to
    /// `close()`
    Closed,
    /// Indicates that channel operation reached timeout and is canceled
    Timeout,
}
impl core::error::Error for ReceiveErrorTimeout {}
impl fmt::Display for ReceiveErrorTimeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                ReceiveErrorTimeout::Closed => "receive from a closed channel",
                ReceiveErrorTimeout::Timeout => "receive timeout",
            },
            f,
        )
    }
}

/// Error type for closing a channel when channel is already closed
#[derive(Debug, PartialEq, Eq)]
pub struct CloseError();
impl std::error::Error for CloseError {}
impl fmt::Display for CloseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt("channel is already closed", f)
    }
}
