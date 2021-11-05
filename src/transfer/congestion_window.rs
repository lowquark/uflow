
use std::time;

use super::MTU;

pub struct CongestionWindow {
    cwnd: usize,
    ssthresh: usize,
    ca_count: usize,
    nack_exp: Option<time::Instant>,
}

impl CongestionWindow {
    // TODO: Slow start & congestion avoidance modes
    // TODO: Max cwnd a function of max bandwidth and RTT
    const MAX_SIZE: usize = 1024*1024*1024;
    const MIN_CWND: usize = MTU;
    const MIN_SSTHRESH: usize = 2*MTU;

    pub fn new() -> Self {
        Self {
            cwnd: MTU,
            ssthresh: Self::MAX_SIZE,
            ca_count: 0,
            nack_exp: None,
        }
    }

    pub fn reset(&mut self) {
        self.ca_count = 0;

        self.ssthresh = self.cwnd/2;
        if self.ssthresh < Self::MIN_SSTHRESH {
            self.ssthresh = Self::MIN_SSTHRESH;
        }
        self.cwnd = MTU;
    }

    pub fn signal_ack(&mut self, size: usize) {
        if self.cwnd >= self.ssthresh {
            // Congestion avoidance
            self.ca_count += size;
            if self.ca_count >= self.cwnd {
                self.ca_count -= self.cwnd;

                self.cwnd += MTU;
                if self.cwnd > Self::MAX_SIZE {
                    self.cwnd = Self::MAX_SIZE;
                }
            }
        } else {
            // Slow start
            self.cwnd += MTU;
            if self.cwnd > Self::MAX_SIZE {
                self.cwnd = Self::MAX_SIZE;
            }
        }
    }

    pub fn signal_nack(&mut self, now: time::Instant, rtt: time::Duration) {
        if let Some(nack_exp) = self.nack_exp {
            if now < nack_exp {
                return;
            } else {
                self.nack_exp = None;
            }
        }

        if self.nack_exp.is_none() {
            self.nack_exp = Some(now + rtt);

            self.ca_count = 0;

            self.ssthresh = self.cwnd;
            if self.ssthresh < Self::MIN_SSTHRESH {
                self.ssthresh = Self::MIN_SSTHRESH;
            }
            self.cwnd = self.cwnd/2;
            if self.cwnd < Self::MIN_CWND {
                self.cwnd = Self::MIN_CWND;
            }
        }
    }

    pub fn size(&self) -> usize {
        self.cwnd
    }
}

