
use crate::frame::Message;

use std::collections::VecDeque;

#[derive(Debug)]
pub struct Entry {
    pub message: Message,
    pub resend: bool,
}

impl Entry {
    pub fn new(message: Message, resend: bool) -> Self {
        Self {
            message,
            resend,
        }
    }
}

pub type MessageQueue = VecDeque<Entry>;

