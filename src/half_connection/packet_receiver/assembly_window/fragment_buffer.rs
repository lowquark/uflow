
use super::MAX_FRAGMENT_SIZE;

pub struct FragmentBuffer {
    buffer: Box<[u8]>,
    fragment_bitfields: Box<[u64]>,
    num_fragments: usize,
    fragments_remaining: usize,
    total_size: usize,
}

impl FragmentBuffer {
    pub fn new(num_fragments: usize) -> Self {
        debug_assert!(num_fragments > 0);

        Self {
            buffer: vec![0; num_fragments * MAX_FRAGMENT_SIZE].into_boxed_slice(),
            fragment_bitfields: vec![0; (num_fragments + 63)/64].into_boxed_slice(),
            num_fragments: num_fragments,
            fragments_remaining: num_fragments,
            total_size: 0,
        }
    }

    pub fn write(&mut self, idx: usize, data: Box<[u8]>) {
        debug_assert!(idx < self.num_fragments);
        debug_assert!(data.len() == MAX_FRAGMENT_SIZE || (idx == self.num_fragments - 1 && data.len() <= MAX_FRAGMENT_SIZE));

        let bitfield_idx = idx / 64;
        let bitfield_bit = 1 << (idx % 64);

        let ref mut bitfield = self.fragment_bitfields[bitfield_idx];

        if *bitfield & bitfield_bit == 0 {
            *bitfield |= bitfield_bit;

            let begin_idx = idx * MAX_FRAGMENT_SIZE;
            let end_idx = begin_idx + data.len();

            self.buffer[begin_idx .. end_idx].copy_from_slice(&data);

            self.fragments_remaining -= 1;
            self.total_size += data.len();
        }
    }

    pub fn finalize(mut self) -> Box<[u8]> {
        debug_assert!(self.total_size <= self.buffer.len());
        let ptr = self.buffer.as_mut_ptr();
        std::mem::forget(self.buffer);
        unsafe { Box::from_raw(std::slice::from_raw_parts_mut(ptr, self.total_size)) }
    }

    pub fn is_finished(&self) -> bool {
        self.fragments_remaining == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_fragment() {
        let mut buf = FragmentBuffer::new(1);

        let fragment_data = (0 .. MAX_FRAGMENT_SIZE).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        buf.write(0, fragment_data.clone());

        assert_eq!(buf.is_finished(), true);
        assert_eq!(buf.finalize(), fragment_data);
    }

    #[test]
    fn multiple_fragments() {
        let mut buf = FragmentBuffer::new(5);

        let packet_data = (0 .. MAX_FRAGMENT_SIZE*5).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        for i in 0 .. 5 {
            assert_eq!(buf.is_finished(), false);
            buf.write(i, packet_data[i * MAX_FRAGMENT_SIZE .. (i + 1) * MAX_FRAGMENT_SIZE].into());
        }

        assert_eq!(buf.is_finished(), true);
        assert_eq!(buf.finalize(), packet_data);
    }

    #[test]
    fn multiple_fragments_nonmultiple() {
        let mut buf = FragmentBuffer::new(5);

        let packet_data = (0 .. MAX_FRAGMENT_SIZE*5 - MAX_FRAGMENT_SIZE/2).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        for i in 0 .. 4 {
            assert_eq!(buf.is_finished(), false);
            buf.write(i, packet_data[i * MAX_FRAGMENT_SIZE .. (i + 1) * MAX_FRAGMENT_SIZE].into());
        }
        assert_eq!(buf.is_finished(), false);
        buf.write(4, packet_data[4 * MAX_FRAGMENT_SIZE .. ].into());

        assert_eq!(buf.is_finished(), true);
        assert_eq!(buf.finalize(), packet_data);
    }

    #[test]
    fn max_fragments() {
        use crate::frame::serial::MAX_FRAGMENTS;

        let mut buf = FragmentBuffer::new(MAX_FRAGMENTS);

        let packet_data = (0 .. MAX_FRAGMENT_SIZE*MAX_FRAGMENTS).map(|i| i as u8).collect::<Vec<_>>().into_boxed_slice();

        let mut indices = (0 .. MAX_FRAGMENTS).map(|i| i as u16).collect::<Vec<_>>();

        for i in 0 ..= MAX_FRAGMENTS - 2 {
            let j = i + rand::random::<usize>() % (MAX_FRAGMENTS - i);
            indices.swap(i, j);
        }

        for i in 0 .. MAX_FRAGMENTS {
            let idx = indices[i] as usize;
            buf.write(idx, packet_data[idx * MAX_FRAGMENT_SIZE .. (idx + 1) * MAX_FRAGMENT_SIZE].into());
        }

        assert_eq!(buf.is_finished(), true);
        assert_eq!(buf.finalize(), packet_data);
    }
}

