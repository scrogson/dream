//! Message types for inter-process communication.

use crate::{Pid, Value};

/// Messages sent between processes
#[derive(Debug, Clone)]
pub enum Message {
    /// User-level message
    User(String),
    /// System message (e.g., crash notification from linked process)
    System(SystemMsg),
}

/// System-level messages
#[derive(Debug, Clone)]
pub enum SystemMsg {
    /// Exit signal from a linked process: {:EXIT, Pid, Reason}
    /// When trap_exit is true, this becomes a message; otherwise it kills the process
    Exit(Pid, Value),
    /// A monitored process exited: {:DOWN, Ref, :process, Pid, Reason}
    Down(u64, Pid, Value),
}
