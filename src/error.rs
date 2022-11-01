#![forbid(unsafe_code)]
use std::fmt;
use std::fmt::Debug;
/// Error type for channel send operations without timeout
#[derive(Debug)]
pub enum SendError {
    /// Indicates that the channel is closed on both sides with a call to `close()`
    Closed,
    /// Indicates that all receiver instances are dropped and the channel is closed from the receive side
    ReceiveClosed,
}
impl std::error::Error for SendError {}
impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                SendError::Closed => "send to a closed channel",
                SendError::ReceiveClosed => "send to a half closed channel",
            },
            f,
        )
    }
}

/// Error type for channel send operations with timeout
#[derive(Debug)]
pub enum SendErrorTimeout {
    /// Indicates that the channel is closed on both sides with a call to `close()`
    Closed,
    /// Indicates that all receiver instances are dropped and the channel is closed from the receive side
    ReceiveClosed,
    /// Indicates that channel operation reached timeout and is canceled
    Timeout,
}
impl std::error::Error for SendErrorTimeout {}
impl fmt::Display for SendErrorTimeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                SendErrorTimeout::Closed => "send to a closed channel",
                SendErrorTimeout::ReceiveClosed => "send to a half closed channel",
                SendErrorTimeout::Timeout => "send timeout",
            },
            f,
        )
    }
}

/// Error type for channel receive operations without timeout
#[derive(Debug)]
pub enum ReceiveError {
    /// Indicates that the channel is closed on both sides with a call to `close()`
    Closed,
    /// Indicates that all sender instances are dropped and the channel is closed from the send side
    SendClosed,
}
impl std::error::Error for ReceiveError {}
impl fmt::Display for ReceiveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                ReceiveError::Closed => "receive from a closed channel",
                ReceiveError::SendClosed => "receive from a half closed channel",
            },
            f,
        )
    }
}

/// Error type for channel receive operations with timeout
#[derive(Debug)]
pub enum ReceiveErrorTimeout {
    /// Indicates that the channel is closed on both sides with a call to `close()`
    Closed,
    /// Indicates that all sender instances are dropped and the channel is closed from the send side
    SendClosed,
    /// Indicates that channel operation reached timeout and is canceled
    Timeout,
}
impl std::error::Error for ReceiveErrorTimeout {}
impl fmt::Display for ReceiveErrorTimeout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(
            match *self {
                ReceiveErrorTimeout::Closed => "receive from a closed channel",
                ReceiveErrorTimeout::SendClosed => "receive from a half closed channel",
                ReceiveErrorTimeout::Timeout => "receive timeout",
            },
            f,
        )
    }
}
