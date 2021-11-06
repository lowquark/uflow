
use std::time;

use super::MTU;

pub struct CongestionWindow {
    cwnd: usize,
    ssthresh: usize,
    max_cwnd: usize,
    ca_count: usize,
    nack_exp: Option<time::Instant>,
    reset_exp: Option<time::Instant>,
}

impl CongestionWindow {
    const MAX_CWND: usize = 1024*1024*1024;
    const MIN_CWND: usize = MTU;
    const MIN_SSTHRESH: usize = 2*MTU;
    const RESET_RTT_SCALE: u32 = 8;

    pub fn new() -> Self {
        Self {
            cwnd: MTU,
            max_cwnd: Self::MAX_CWND,
            ssthresh: Self::MAX_CWND,
            ca_count: 0,
            nack_exp: None,
            reset_exp: None,
        }
    }

    // TODO: Why not grow by the amount of data acknowledged? Should consult RFC related to..
    // "appropriate byte counting"?
    fn grow(&mut self) {
        self.cwnd += MTU;
        if self.cwnd > self.max_cwnd {
            self.cwnd = self.max_cwnd;
        }
    }

    fn shrink(&mut self) {
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

    fn reset(&mut self) {
        self.ca_count = 0;
        self.nack_exp = None;
        self.reset_exp = None;

        self.ssthresh = self.cwnd/2;
        if self.ssthresh < Self::MIN_SSTHRESH {
            self.ssthresh = Self::MIN_SSTHRESH;
        }

        self.cwnd = MTU;
    }

    pub fn signal_ack(&mut self, size: usize) {
        if size == 0 {
            return;
        }

        self.reset_exp = None;

        if self.cwnd >= self.ssthresh {
            // Congestion avoidance
            self.ca_count += size;
            if self.ca_count >= self.cwnd {
                self.ca_count -= self.cwnd;
                self.grow();
            }
        } else {
            // Slow start
            self.grow();
        }
    }

    pub fn signal_nack(&mut self, now: time::Instant, rtt: time::Duration) {
        if let Some(reset_exp) = self.reset_exp {
            if now >= reset_exp {
                self.reset();
                return;
            }
        } else {
            self.reset_exp = Some(now + rtt * Self::RESET_RTT_SCALE);
        }

        if let Some(nack_exp) = self.nack_exp {
            if now < nack_exp {
                return;
            }
        }

        self.nack_exp = Some(now + rtt);

        self.shrink();
    }

    pub fn size(&self) -> usize {
        self.cwnd
    }

    pub fn set_max_size(&mut self, max_size: usize) {
        self.max_cwnd = max_size.min(Self::MAX_CWND).max(Self::MIN_CWND);
        if self.cwnd > self.max_cwnd {
            self.cwnd = self.max_cwnd;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CongestionWindow;
    use super::MTU;

    use std::time;

    #[test]
    fn test_multi_ack_slow_start() {
        let mut cw = CongestionWindow::new();

        assert_eq!(cw.cwnd, MTU);

        for i in 0..200 {
            cw.signal_ack(MTU);

            assert_eq!(cw.cwnd, (2 + i)*MTU);
        }
    }

    #[test]
    fn test_multi_ack_congestion_avoidance() {
        let mut cw = CongestionWindow::new();

        let initial_cwnd = CongestionWindow::MIN_SSTHRESH;
        cw.cwnd = initial_cwnd;
        cw.ssthresh = initial_cwnd;

        for i in 0..100 {
            for i in 0..cw.cwnd/MTU {
                cw.signal_ack(MTU/2);
                cw.signal_ack(MTU - MTU/2);
            }

            assert_eq!(cw.cwnd, initial_cwnd + (i + 1)*MTU);
        }
    }

    #[test]
    fn test_multi_nack() {
        let mut cw = CongestionWindow::new();

        let t0 = time::Instant::now();
        let rtt = time::Duration::from_millis(100);

        let initial_cwnd = 32768;
        cw.cwnd = initial_cwnd;
        cw.ssthresh = initial_cwnd;

        cw.signal_nack(t0, rtt);

        // Nack
        assert_eq!(cw.cwnd, initial_cwnd/2);
        assert_eq!(cw.ssthresh, initial_cwnd);

        for i in 0..20 {
            cw.signal_nack(t0 + time::Duration::from_millis(i*5), rtt);

            // No nack
            assert_eq!(cw.cwnd, initial_cwnd/2);
            assert_eq!(cw.ssthresh, initial_cwnd);
        }

        cw.signal_nack(t0 + time::Duration::from_millis(100), rtt);

        // Nack, nack
        assert_eq!(cw.cwnd, initial_cwnd/4);
        assert_eq!(cw.ssthresh, initial_cwnd/2);
    }

    #[test]
    fn test_nack_reset() {
        let mut cw = CongestionWindow::new();

        let t0 = time::Instant::now();
        let rtt = time::Duration::from_millis(100);

        let initial_cwnd = 32768;
        cw.cwnd = initial_cwnd;
        cw.ssthresh = initial_cwnd;

        cw.signal_nack(t0, rtt);
        cw.signal_nack(t0 + CongestionWindow::RESET_RTT_SCALE*rtt, rtt);

        // Nack, reset
        assert_eq!(cw.cwnd, CongestionWindow::MIN_CWND);
        assert_eq!(cw.ssthresh, initial_cwnd / 4);
    }

    #[test]
    fn test_nack_no_reset() {
        let mut cw = CongestionWindow::new();

        let t0 = time::Instant::now();
        let rtt = time::Duration::from_millis(100);

        let initial_cwnd = 32768;
        cw.cwnd = initial_cwnd;
        cw.ssthresh = initial_cwnd;

        cw.signal_nack(t0, rtt);
        cw.signal_ack(1);
        cw.signal_nack(t0 + CongestionWindow::RESET_RTT_SCALE*rtt, rtt);

        // Nack, ack, nack, no reset
        assert_eq!(cw.cwnd, (initial_cwnd/2 + MTU)/2);
        assert_eq!(cw.ssthresh, (initial_cwnd/2 + MTU));
    }
}

