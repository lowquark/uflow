
pub struct ReorderBuffer {
    frames: [u32; 2],
    frame_count: u32,
    base_id: u32,
    max_span: u32,
}

impl ReorderBuffer {
    pub fn new(base_id: u32, max_span: u32) -> Self {
        Self {
            frames: [0, 0],
            frame_count: 0,
            base_id,
            max_span,
        }
    }

    #[cfg(test)]
    pub fn base_id(&self) -> u32 {
        self.base_id
    }

    pub fn can_put(&self, new_frame_id: u32) -> bool {
        new_frame_id.wrapping_sub(self.base_id) < self.max_span
    }

    pub fn put<F>(&mut self, new_frame_id: u32, mut callback: F) where F: FnMut(u32, bool) {
        debug_assert!(self.can_put(new_frame_id));

        if self.frame_count > 0 {
            debug_assert!(self.frames[0] != self.base_id);
        }

        debug_assert!(self.frame_count <= 2);

        match self.frame_count {
            0 => {
                if new_frame_id == self.base_id {
                    callback(new_frame_id, true);
                    self.base_id = self.base_id.wrapping_add(1);
                } else {
                    self.frames[0] = new_frame_id;
                    self.frame_count = 1;
                }
            }
            1 => {
                if new_frame_id == self.base_id {
                    callback(new_frame_id, true);
                    self.base_id = self.base_id.wrapping_add(1);

                    if self.frames[0] == self.base_id {
                        callback(self.frames[0], true);
                        self.base_id = self.base_id.wrapping_add(1);
                        self.frame_count = 0;
                    }
                } else {
                    let delta_new = new_frame_id.wrapping_sub(self.base_id);
                    let delta_0 = self.frames[0].wrapping_sub(self.base_id);

                    debug_assert!(delta_new != delta_0);
                    if delta_new < delta_0 {
                        self.frames[1] = self.frames[0];
                        self.frames[0] = new_frame_id;
                    } else {
                        self.frames[1] = new_frame_id;
                    }

                    self.frame_count = 2;
                }
            }
            2 => {
                let mut min_frame_id = new_frame_id;
                let mut delta_min = new_frame_id.wrapping_sub(self.base_id);

                let delta_1 = self.frames[1].wrapping_sub(self.base_id);

                debug_assert!(delta_1 != delta_min);
                if delta_1 < delta_min {
                    std::mem::swap(&mut self.frames[1], &mut min_frame_id);
                    delta_min = delta_1;
                }

                let delta_0 = self.frames[0].wrapping_sub(self.base_id);

                debug_assert!(delta_0 != delta_min);
                if delta_0 < delta_min {
                    std::mem::swap(&mut self.frames[0], &mut min_frame_id);
                }

                while self.base_id != min_frame_id {
                    callback(self.base_id, false);
                    self.base_id = self.base_id.wrapping_add(1);
                }

                callback(min_frame_id, true);
                self.base_id = self.base_id.wrapping_add(1);

                if self.frames[0] == self.base_id {
                    callback(self.frames[0], true);
                    self.base_id = self.base_id.wrapping_add(1);
                    self.frame_count -= 1;

                    if self.frames[1] == self.base_id {
                        callback(self.frames[1], true);
                        self.base_id = self.base_id.wrapping_add(1);
                        self.frame_count -= 1;
                    } else {
                        self.frames[0] = self.frames[1];
                    }
                }
            }
            _ => ()
        }
    }

    pub fn can_advance(&self, new_base_id: u32) -> bool {
        let delta = new_base_id.wrapping_sub(self.base_id);
        delta >= 1 && delta <= self.max_span
    }

    pub fn advance<F>(&mut self, new_base_id: u32, mut callback: F) where F: FnMut(u32, bool) {
        debug_assert!(self.can_advance(new_base_id));

        if self.frame_count > 0 {
            debug_assert!(self.frames[0] != self.base_id);
        }

        debug_assert!(self.frame_count <= 2);

        while self.frame_count > 0 && self.frames[0].wrapping_sub(self.base_id) < new_base_id.wrapping_sub(self.base_id) {
            while self.base_id != self.frames[0] {
                callback(self.base_id, false);
                self.base_id = self.base_id.wrapping_add(1);
            }

            callback(self.frames[0], true);
            self.base_id = self.base_id.wrapping_add(1);

            if self.frame_count == 2 {
                self.frames[0] = self.frames[1];
            }

            self.frame_count -= 1;
        }

        while self.base_id != new_base_id {
            callback(self.base_id, false);
            self.base_id = self.base_id.wrapping_add(1);
        }

        match self.frame_count {
            0 => (),
            1 => {
                if self.frames[0] == self.base_id {
                    callback(self.frames[0], true);
                    self.base_id = self.base_id.wrapping_add(1);
                    self.frame_count = 0;
                }
            }
            2 => {
                if self.frames[0] == self.base_id {
                    callback(self.frames[0], true);
                    self.base_id = self.base_id.wrapping_add(1);
                    self.frame_count -= 1;

                    if self.frames[1] == self.base_id {
                        callback(self.frames[1], true);
                        self.base_id = self.base_id.wrapping_add(1);
                        self.frame_count -= 1;
                    } else {
                        self.frames[0] = self.frames[1];
                    }
                }
            }
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_put_callbacks(rb: &mut ReorderBuffer, new_frame_id: u32, expected_callbacks: Vec<(u32, bool)>) {
        let mut callbacks = Vec::new();
        let cb = |frame_id: u32, acked: bool| {
            callbacks.push((frame_id, acked));
        };
        rb.put(new_frame_id, cb);
        assert_eq!(callbacks, expected_callbacks);
    }

    fn test_advance_callbacks(rb: &mut ReorderBuffer, new_frame_id: u32, expected_callbacks: Vec<(u32, bool)>) {
        let mut callbacks = Vec::new();
        let cb = |frame_id: u32, acked: bool| {
            callbacks.push((frame_id, acked));
        };
        rb.advance(new_frame_id, cb);
        assert_eq!(callbacks, expected_callbacks);
    }

    #[test]
    fn ack_1_ack() {
        // Ack 0
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);
        assert_eq!(rb.frame_count, 2);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);
        assert_eq!(rb.frame_count, 2);

        // Nack 0, 1, Ack 2
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(rb.frame_count, 2);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(rb.frame_count, 2);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(rb.frame_count, 2);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(rb.frame_count, 2);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(rb.frame_count, 2);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(rb.frame_count, 2);
    }

    #[test]
    fn ack_2_acks() {
        // Ack 0, 1
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true)]);
        assert_eq!(rb.frame_count, 1);

        // Nack 0, 1, Ack 2, 3
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true), (3, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true), (3, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true)]);
        assert_eq!(rb.frame_count, 1);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true)]);
        assert_eq!(rb.frame_count, 1);
    }

    #[test]
    fn ack_3_acks() {
        // Ack 0, 1, 2
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true), (2, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true), (2, true)]);
        assert_eq!(rb.frame_count, 0);

        // Nack 0, 1, ack 2, 3, 4
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
        assert_eq!(rb.frame_count, 0);

        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
        assert_eq!(rb.frame_count, 0);
    }

    #[test]
    fn advance_0_acks() {
        // 0x beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, false)]);
        assert_eq!(rb.frame_count, 0);

        // 1x beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 6, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, false)]);
        assert_eq!(rb.frame_count, 1);

        // 2x beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 6, vec![]);
        test_put_callbacks(&mut rb, 7, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, false)]);
        assert_eq!(rb.frame_count, 2);
    }

    #[test]
    fn advance_1_ack() {
        // ~space, ~beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_advance_callbacks(&mut rb, 2, vec![(0, false), (1, true)]);
        assert_eq!(rb.frame_count, 0);

        // ~space, beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 6, vec![]);
        test_advance_callbacks(&mut rb, 2, vec![(0, false), (1, true)]);
        assert_eq!(rb.frame_count, 1);

        // space, ~beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, true), (2, false), (3, false), (4, false)]);
        assert_eq!(rb.frame_count, 0);

        // space, beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 6, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, true), (2, false), (3, false), (4, false)]);
        assert_eq!(rb.frame_count, 1);

        // past-end, ~beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, false), (5, true)]);
        assert_eq!(rb.frame_count, 0);

        // past-end, beyond
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 7, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, false), (5, true)]);
        assert_eq!(rb.frame_count, 1);
    }

    #[test]
    fn advance_2_acks() {
        // ~space, ~space
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_advance_callbacks(&mut rb, 3, vec![(0, false), (1, true), (2, true)]);
        assert_eq!(rb.frame_count, 0);

        // ~space, space
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, true), (2, true), (3, false), (4, false)]);
        assert_eq!(rb.frame_count, 0);

        // space, ~space
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, true), (2, false), (3, false), (4, true)]);
        assert_eq!(rb.frame_count, 0);

        // space, space
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, true), (2, false), (3, true), (4, false)]);
        assert_eq!(rb.frame_count, 0);

        // ~space, past-end
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, true), (5, true)]);
        assert_eq!(rb.frame_count, 0);

        // space, past-end
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, true), (2, false), (3, false), (4, false), (5, true)]);
        assert_eq!(rb.frame_count, 0);

        // 2x past-end
        let mut rb = ReorderBuffer::new(0, 100);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 6, vec![]);
        test_advance_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, false), (3, false), (4, false), (5, true), (6, true)]);
        assert_eq!(rb.frame_count, 0);
    }

    #[test]
    fn max_span() {
        let rb = ReorderBuffer::new(1, 100);
        assert_eq!(rb.can_put(0), false);
        assert_eq!(rb.can_put(1), true);

        assert_eq!(rb.can_put(100), true);
        assert_eq!(rb.can_put(101), false);

        assert_eq!(rb.can_advance(0), false);
        assert_eq!(rb.can_advance(1), false);
        assert_eq!(rb.can_advance(2), true);

        assert_eq!(rb.can_advance(100), true);
        assert_eq!(rb.can_advance(101), true);
        assert_eq!(rb.can_advance(102), false);
    }
}

