
use std::time;

#[derive(Clone,Debug)]
struct RecvEntry {
    value: u32,
    timestamp: time::Instant,
    is_initial: bool,
}

pub struct RecvRateSet {
    // Queue of receive rates reported by receiver (X_recv_set)
    entries: Vec<RecvEntry>,
}

impl RecvRateSet {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn reset_initial(&mut self, now: time::Instant) {
        self.entries.clear();

        self.entries.push(RecvEntry {
            value: u32::max_value(),
            timestamp: now,
            is_initial: true,
        });
    }

    fn replace_max(&mut self, now: time::Instant, recv_rate: u32) -> u32 {
        self.entries.retain(|e| e.is_initial == false);

        let max_rate = if self.entries.is_empty() {
            recv_rate
        } else {
            self.max().max(recv_rate) // lul
        };

        self.reset(now, max_rate);

        return max_rate;
    }

    pub fn reset(&mut self, now: time::Instant, recv_rate: u32) {
        self.entries.clear();

        self.entries.push(RecvEntry {
            value: recv_rate,
            timestamp: now,
            is_initial: false,
        });
    }

    pub fn rate_limited_update(&mut self, now: time::Instant, recv_rate: u32, rtt_s: f64) -> u32 {
        self.entries.push(RecvEntry {
            value: recv_rate,
            timestamp: now,
            is_initial: false
        });

        self.entries.retain(|e| (now - e.timestamp).as_secs_f64() < 2.0 * rtt_s);

        return self.max();
    }

    pub fn loss_increase_update(&mut self, now: time::Instant, recv_rate: u32) -> u32 {
        for entry in self.entries.iter_mut() {
            entry.value /= 2;
        }

        return self.replace_max(now, (recv_rate as f64 * 0.85) as u32);
    }

    pub fn data_limited_update(&mut self, now: time::Instant, recv_rate: u32) -> u32 {
        return self.replace_max(now, recv_rate);
    }

    pub fn max(&self) -> u32 {
        let mut max_rate = self.entries.first().unwrap().value;
        for entry in self.entries.iter().skip(1) {
            if entry.value > max_rate {
                max_rate = entry.value;
            }
        }
        return max_rate;
    }
}

