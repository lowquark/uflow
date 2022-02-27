
use crate::frame;

use super::TransferWindow;
use super::send_rate;
use super::frame_log_2::FrameLog;
use super::feedback;
use super::feedback::FeedbackBuilder;

fn handle_ack_group(ack: frame::FrameAck, frame_log: &mut FrameLog, feedback_builder: &mut FeedbackBuilder) {
    let mut true_nonce = false;

    let mut last_send_time_ms = 0;
    let mut total_ack_size = 0;
    let mut rate_limited = false;

    let mut bitfield_size = 0;
    for i in (0 .. 32).rev() {
        if ack.bitfield & (1 << i) != 0 {
            bitfield_size = i + 1;
            break;
        }
    }

    if bitfield_size == 0 {
        // Dud
        return;
    }

    for i in 0 .. bitfield_size {
        let frame_id = ack.base_id.wrapping_add(i);

        if let Some(ref sent_frame) = frame_log.get_frame(frame_id) {
            if ack.bitfield & (1 << i) != 0 {
                // Receiver claims to have received this packet
                true_nonce ^= sent_frame.nonce;
            }
        } else {
            // Packet forgotten or ack group exceeds span of transfer queue
            return;
        }
    }

    if ack.nonce != true_nonce {
        // Penalize bad nonce
        return;
    }

    for i in 0 .. bitfield_size {
        let frame_id = ack.base_id.wrapping_add(i);

        let ref mut sent_frame = frame_log.get_frame_mut(frame_id).unwrap();

        rate_limited |= sent_frame.rate_limited;

        if ack.bitfield & (1 << i) != 0 {
            // Receiver has received this packet
            if sent_frame.acked == false {
                sent_frame.acked = true;

                // Mark each fragment acknowledged and clear the list
                let fragment_refs = std::mem::take(&mut sent_frame.fragment_refs);

                for fragment_ref in fragment_refs.into_iter() {
                    // TODO: This could be a method?
                    if let Some(packet_rc) = fragment_ref.packet.upgrade() {
                        let mut packet_ref = packet_rc.borrow_mut();
                        packet_ref.acknowledge_fragment(fragment_ref.fragment_id);
                    }
                }

                // Mark send time of latest included packet
                last_send_time_ms = last_send_time_ms.max(sent_frame.send_time_ms);

                // Add to total ack size
                total_ack_size += sent_frame.size as usize;

                // Detect nacks
                feedback_builder.mark_acked(frame_id, frame_log);
            }
        }
    }

    // Add to pending feedback data
    feedback_builder.put_ack_data(feedback::AckData { last_send_time_ms, total_ack_size, rate_limited });
}

fn remove_expired_entries(thresh_ms: u64, frame_log: &mut FrameLog, feedback_builder: &mut FeedbackBuilder) {
    let max_base_id = frame_log.find_expiration_cutoff(thresh_ms);

    if max_base_id != frame_log.base_id() {
        feedback_builder.advance_base_id(max_base_id, frame_log);

        frame_log.drain(max_base_id);
        debug_assert!(frame_log.base_id() == max_base_id);
    }
}

fn remove_old_entries_beyond_window(transfer_window: &mut TransferWindow, frame_log: &mut FrameLog, feedback_builder: &mut FeedbackBuilder) {
    let max_base_id = transfer_window.base_id.wrapping_sub(transfer_window.size);

    let delta = max_base_id.wrapping_sub(frame_log.base_id());

    if delta > 0 && delta <= frame_log.len() {
        // The frame log has entries beyond the transfer window on the base end
        feedback_builder.advance_base_id(max_base_id, frame_log);

        frame_log.drain(max_base_id);
        debug_assert!(frame_log.base_id() == max_base_id);
    }
}

fn advance_transfer_window(new_base_id: u32, transfer_window: &mut TransferWindow, frame_log: &mut FrameLog, feedback_builder: &mut FeedbackBuilder) {
    let log_next_id = frame_log.next_id();
    let log_base_id = frame_log.base_id();
    let window_base_id = transfer_window.base_id;

    // Ensure transfer window never backtracks and never advances beyond frame log's next_id
    let next_delta = log_next_id.wrapping_sub(window_base_id);
    let delta = new_base_id.wrapping_sub(window_base_id);

    if delta == 0 || delta > next_delta {
        return;
    }

    // Advance transfer window
    transfer_window.base_id = new_base_id;

    // Prune expired frame log entries, purge corresponding IDs
    remove_old_entries_beyond_window(transfer_window, frame_log, feedback_builder);
}

pub fn handle_ack_frame(ack: frame::AckFrame, transfer_window: &mut TransferWindow, frame_log: &mut FrameLog, feedback_builder: &mut FeedbackBuilder) {
    for ack_group in ack.frame_acks.into_iter() {
        handle_ack_group(ack_group, frame_log, feedback_builder);
    }

    let receiver_base_id = /*ack.receiver_base_id*/0;
    advance_transfer_window(receiver_base_id, transfer_window, frame_log, feedback_builder);
}

pub fn step(now_ms: u64, feedback_builder: &mut FeedbackBuilder, sr_comp: &mut send_rate::SendRateComp) {
    sr_comp.step(now_ms, feedback_builder.get_feedback(now_ms),
        |new_loss_rate: f64, end_time_ms: u64| {
            feedback_builder.reset_loss_rate(new_loss_rate, end_time_ms);
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::pending_packet::PendingPacket;

    use std::rc::Rc;
    use std::cell::RefCell;

    /*
    #[test]
    fn window_advancement() {
        let mut tq = FrameLog::new(4, 3, 0);

        let packet_rc = Rc::new(RefCell::new(
            PendingPacket::new(vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(), 0, 0, 0, 0)
        ));

        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());

        assert_eq!(tq.can_put_frame(), false);

        tq.advance_receiver_base_id(4);
        assert_eq!(tq.receiver_base_id, 4);

        assert_eq!(tq.can_put_frame(), true);

        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.put_frame(555, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());

        assert_eq!(tq.can_put_frame(), false);

        assert_eq!(tq.frames.len(), 7);
        assert_eq!(tq.base_id, 1);
        assert_eq!(tq.next_id, 8);

        tq.advance_receiver_base_id(8);
        assert_eq!(tq.receiver_base_id, 8);

        assert_eq!(tq.frames.len(), 3);
        assert_eq!(tq.base_id, 5);
        assert_eq!(tq.next_id, 8);
    }
    */

    /*
    #[test]
    fn ack_data_aggregation() {
        let mut ast = AckState::new();
        let mut tq = FrameLog::new(8, 8, 0);

        let packet_rc = Rc::new(RefCell::new(
            PendingPacket::new(vec![ 0x00, 0x01, 0x02 ].into_boxed_slice(), 0, 0, 0, 0)
        ));

        let (_, n0) = tq.put_frame(  1, 0, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_,  _) = tq.put_frame(  2, 1, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_, n2) = tq.put_frame(  4, 2, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_, n3) = tq.put_frame(  8, 3, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.log_rate_limited();
        let (_, n4) = tq.put_frame( 16, 4, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        let (_, n5) = tq.put_frame( 32, 5, vec![ FragmentRef::new(&packet_rc, 0) ].into_boxed_slice());
        tq.log_rate_limited();

        let callback = |_frame_id: u32| { };

        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 0, bitfield: 0b101, nonce: n0 ^ n2 }, callback);

        assert_eq!(ast.poll(), Some(AckData { size: 5, send_time_ms: 2, rate_limited: false }));

        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 2, bitfield: 0b11, nonce: n2 ^ n3 }, callback);

        assert_eq!(ast.poll(), Some(AckData { size: 8, send_time_ms: 3, rate_limited: false }));

        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 5, bitfield: 0b1, nonce: n5 }, callback);
        acknowledge_frames(&mut tq, &mut ast, frame::FrameAck { base_id: 4, bitfield: 0b1, nonce: n4 }, callback);

        assert_eq!(ast.poll(), Some(AckData { size: 48, send_time_ms: 5, rate_limited: true }));
    }
    */
}

