
use std::collections::VecDeque;

use super::frame;

#[derive(Debug)]
pub struct SendEntry {
    pub data: frame::DataEntry,
    pub reliable: bool,
}

impl SendEntry {
    fn new(data: frame::DataEntry, reliable: bool) -> Self {
        Self {
            data: data,
            reliable: reliable,
        }
    }
}

pub struct SendQueue {
    high_priority: VecDeque<SendEntry>,
    low_priority: VecDeque<SendEntry>,
}

impl SendQueue {
    pub fn new() -> Self {
        Self {
            high_priority: VecDeque::new(),
            low_priority: VecDeque::new(),
        }
    }

    pub fn push(&mut self, data: frame::DataEntry, reliable: bool, high_priority: bool) {
        let entry = SendEntry::new(data, reliable);

        if high_priority {
            self.high_priority.push_back(entry);
        } else {
            self.low_priority.push_back(entry);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.low_priority.is_empty()
    }

    pub fn front(&self) -> Option<&SendEntry> {
        if self.high_priority.len() > 0 {
            self.high_priority.front()
        } else {
            self.low_priority.front()
        }
    }

    pub fn pop_front(&mut self) -> Option<SendEntry> {
        if self.high_priority.len() > 0 {
            self.high_priority.pop_front()
        } else {
            self.low_priority.pop_front()
        }
    }
}

