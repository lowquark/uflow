
use std::time;

struct Ping {
    sequence_id: u16,
    instant: time::Instant,
}

pub struct PingRtt {
    ping_history: Vec<Ping>,
    next_sequence_id: u16,
    srtt_ms: f64,
    sdrtt_ms: f64,
    ping_received: bool,
}

impl PingRtt {
    const DEFAULT_RTT_MS: f64 = 100.0;
    const PING_TIMEOUT_MS: u64 = 3000;
    const RTT_SMOOTH_ALPHA: f64 = 0.875;
    const DRTT_SMOOTH_ALPHA: f64 = 0.875;
    const RTO_SDRTT_U: f64 = 4.0;

    pub fn new() -> Self {
        Self {
            ping_history: Vec::new(),
            next_sequence_id: 0,
            srtt_ms: Self::DEFAULT_RTT_MS,
            sdrtt_ms: 0.0,
            ping_received: false,
        }
    }

    fn update_rtt(&mut self, rtt_ms: f64) {
        if !self.ping_received {
            self.ping_received = true;
            self.srtt_ms = rtt_ms;
            self.sdrtt_ms = 0.0;
        } else {
            self.srtt_ms = Self::RTT_SMOOTH_ALPHA*self.srtt_ms + (1.0 - Self::RTT_SMOOTH_ALPHA)*rtt_ms;
            self.sdrtt_ms = Self::DRTT_SMOOTH_ALPHA*self.sdrtt_ms + (1.0 - Self::DRTT_SMOOTH_ALPHA)*(rtt_ms - self.srtt_ms).abs();
        }
    }

    pub fn new_ping(&mut self, now: time::Instant) -> u16 {
        let sequence_id = self.next_sequence_id;
        self.ping_history.push(Ping { sequence_id: sequence_id, instant: now } );
        self.next_sequence_id = self.next_sequence_id.wrapping_add(1);
        sequence_id
    }

    pub fn handle_ack(&mut self, now: time::Instant, sequence_id: u16) {
        for (idx, ping) in self.ping_history.iter().enumerate() {
            if sequence_id == ping.sequence_id {
                let rtt_ms = (now - ping.instant).as_secs_f64()*1_000.0;
                self.ping_history.remove(idx);
                self.update_rtt(rtt_ms);
                break;
            }
        }

        self.ping_history.retain(|ping| (now - ping.instant).as_millis() < Self::PING_TIMEOUT_MS as u128);
    }

    pub fn rtt_ms(&self) -> f64 {
        self.srtt_ms
    }

    pub fn rto_ms(&self) -> f64 {
        self.srtt_ms + Self::RTO_SDRTT_U*self.sdrtt_ms
    }
}
