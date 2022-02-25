
pub struct ReorderBuffer {
    frames: [u32; 2],
    frame_count: u32,
    next_id: u32,
}

impl ReorderBuffer {
    pub fn new(next_id: u32) -> Self {
        Self {
            frames: [0, 0],
            frame_count: 0,
            next_id,
        }
    }

    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    pub fn put<F>(&mut self, new_frame_id: u32, mut callback: F) where F: FnMut(u32, bool) {
        if self.frame_count > 0 {
            debug_assert!(self.frames[0] != self.next_id);
        }

        debug_assert!(self.frame_count <= 2);

        match self.frame_count {
            0 => {
                if new_frame_id == self.next_id {
                    callback(new_frame_id, true);
                    self.next_id = self.next_id.wrapping_add(1);
                } else {
                    self.frames[0] = new_frame_id;
                    self.frame_count = 1;
                }
            }
            1 => {
                if new_frame_id == self.next_id {
                    callback(new_frame_id, true);
                    self.next_id = self.next_id.wrapping_add(1);

                    if self.frames[0] == self.next_id {
                        callback(self.frames[0], true);
                        self.next_id = self.next_id.wrapping_add(1);
                        self.frame_count = 0;
                    }
                } else {
                    let delta_new = new_frame_id.wrapping_sub(self.next_id);
                    let delta_0 = self.frames[0].wrapping_sub(self.next_id);

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
                let mut delta_min = new_frame_id.wrapping_sub(self.next_id);

                let delta_1 = self.frames[1].wrapping_sub(self.next_id);

                debug_assert!(delta_1 != delta_min);
                if delta_1 < delta_min {
                    std::mem::swap(&mut self.frames[1], &mut min_frame_id);
                    delta_min = delta_1;
                }

                let delta_0 = self.frames[0].wrapping_sub(self.next_id);

                debug_assert!(delta_0 != delta_min);
                if delta_0 < delta_min {
                    std::mem::swap(&mut self.frames[0], &mut min_frame_id);
                }

                while self.next_id != min_frame_id {
                    callback(self.next_id, false);
                    self.next_id = self.next_id.wrapping_add(1);
                }

                callback(min_frame_id, true);
                self.next_id = self.next_id.wrapping_add(1);

                if self.frames[0] == self.next_id {
                    callback(self.frames[0], true);
                    self.next_id = self.next_id.wrapping_add(1);

                    if self.frames[1] == self.next_id {
                        callback(self.frames[1], true);
                        self.next_id = self.next_id.wrapping_add(1);
                        self.frame_count = 0;
                    } else {
                        self.frames[0] = self.frames[1];
                        self.frame_count = 1;
                    }
                }
            }
            _ => ()
        }
    }

    pub fn advance<F>(&mut self, new_base_id: u32, mut callback: F) where F: FnMut(u32, bool) {
        if self.frame_count > 0 {
            debug_assert!(self.frames[0] != self.next_id);
        }

        debug_assert!(self.frame_count <= 2);

        match self.frame_count {
            0 => {
            }
            1 => {
            }
            2 => {
            }
            _ => ()
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

    #[test]
    fn ack_1() {
        // Ack 0
        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true)]);

        // Nack 0, 1, Ack 2
        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true)]);
    }

    #[test]
    fn ack_2() {
        // Ack 0, 1
        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true)]);

        // Nack 0, 1, Ack 2, 3
        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true), (3, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 5, vec![(0, false), (1, false), (2, true), (3, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 5, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true)]);
    }

    #[test]
    fn ack_3() {
        // Ack 0, 1, 2
        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true), (2, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 1, vec![]);
        test_put_callbacks(&mut rb, 0, vec![(0, true), (1, true), (2, true)]);

        // Nack 0, 1, ack 2, 3, 4
        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 4, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 2, vec![]);
        test_put_callbacks(&mut rb, 3, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);

        let mut rb = ReorderBuffer::new(0);
        test_put_callbacks(&mut rb, 4, vec![]);
        test_put_callbacks(&mut rb, 3, vec![]);
        test_put_callbacks(&mut rb, 2, vec![(0, false), (1, false), (2, true), (3, true), (4, true)]);
    }
}

