//! Reliable, bounded appearance streams. Movement is serviced first by the lobby.
//! Selective acknowledgements slide continuously; no page-sized round trips.
use crate::{lobby::Outgoing, packed};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const META: u8 = 12;
pub const DATA: u8 = 13;
pub const ACK: u8 = 14;
pub const MAX_BLOB: usize = 64 * 1024 * 1024;
const MEMORY: usize = 12 * MAX_BLOB;
/// Downloads in progress hold at most this much together, so finished looks always have room (P4).
const PARTIAL_MEMORY: usize = MEMORY / 2;
/// A download with no new chunk this long is given up (P4). A slow one that keeps coming is not (audit 10-28 #3: one
/// slower than 32 kB/s overall was, though a big model in a full lobby comes that slowly).
const STALL_MS: u64 = 30_000;
/// Audit 10-28 #4: members our look goes to at once; the next starts when one has it all. A 50-player lobby sent ~800 kB
/// in its first flight (16 chunks to each of 49), which filled the upload movement and voice share for a second or two.
const SENDING_TO: usize = 4;
const CHUNK: usize = 1024;
const WINDOW: usize = 1024;
const MASK: usize = WINDOW / 8;
/// Ledger #54 (user 10-26): chunks in flight to one peer at most, 384 KiB (~409 KB on the wire): under a Steam
/// connection's 512 KB send buffer (GNS k_ESteamNetworkingConfig_SendBufferSize) with room left for movement. On
/// Steam's 256 KB/s connections the queue back-off ([`QUEUE_MS`]) keeps it far lower.
const SEND_WINDOW: usize = 384;
/// Chunks that come back this much slower than the fastest round trip seen waited in a queue (Steam's send buffer, a
/// slow uplink) that movement shares: the window halves (to [`MIN_WINDOW`] at least) instead of growing.
const QUEUE_MS: u64 = 100;
const MIN_WINDOW: usize = 8;

/// A look's identity hash (what META announces).
pub fn digest(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub hash: [u8; 32],
    pub size: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(bytes: &[u8]) -> Vec<u8> {
        let mut b = blake3::hash(bytes).as_bytes().to_vec();
        b.extend((bytes.len() as u32).to_le_bytes());
        b
    }
    #[test]
    fn blob_bounds_hash_validation_and_obsolete_chunks() {
        let mut b = Blobs::default();
        let mut oversized = vec![0; 32];
        oversized.extend(((MAX_BLOB + 1) as u32).to_le_bytes());
        b.receive(1, 10, META, 1, &oversized, 0);
        assert!(b.cache.is_empty());
        b.receive(1, 10, META, 1, &manifest(&[1, 2]), 0);
        let mut wrong = 0u32.to_le_bytes().to_vec();
        wrong.extend([3, 4]);
        b.receive(1, 10, DATA, 1, &wrong, 0);
        assert!(
            b.ready(10).is_none(),
            "corrupt content must never be loaded"
        );
        b.receive(1, 10, META, 2, &manifest(&[5, 6]), 0);
        b.receive(1, 10, DATA, 1, &wrong, 0);
        b.receive(1, 10, META, 1, &manifest(&[1, 2]), 0);
        assert_eq!(b.actors[&10].0, 2);
        let mut valid = 0u32.to_le_bytes().to_vec();
        valid.extend([5, 6]);
        b.receive(1, 10, DATA, 2, &valid, 0);
        assert_eq!(b.ready(10).unwrap().1, &[5, 6]);
        // A later peer can consume verified cached bytes without data retransmission.
        b.receive(2, 11, META, 1, &manifest(&[5, 6]), 0);
        assert_eq!(b.ready(11).unwrap().1, &[5, 6]);
    }
    #[test]
    fn a_look_whose_wearers_left_is_freed() {
        let mut b = Blobs::default();
        b.publish(1, vec![1]);
        b.publish(2, vec![2]);
        b.publish(3, vec![2]);
        b.service(9, 1, &BTreeSet::from([1, 3]), &[], 0);
        assert_eq!(b.cache.len(), 2, "a look one remaining member still wears stays");
        b.service(9, 1, &BTreeSet::from([1]), &[], 0);
        assert_eq!(b.cache.len(), 1, "only our own look is left");
        assert_eq!(b.ready(1).unwrap().1, &[1]);
    }
    #[test]
    fn lobby_rejects_forged_blob_origins_and_unregistered_senders() {
        use crate::lobby::{Info, Session};
        let info = |id| Info {
            id,
            map: 1,
            rig: 1,
            physics: 1,
            appearance: 1,
        };
        let mut host = Session::new(88, info(10), None);
        let mut client = Session::new(88, info(11), Some(1));
        // HELLO, COOKIE, HELLO with the cookie.
        for now in [1, 2] {
            for p in client.service(now) {
                host.receive(2, &p.data, now);
            }
            for p in host.service(now) {
                client.receive(1, &p.data, now);
            }
        }
        let mut forged = packed::header(88, 12, META, 1);
        forged.extend(manifest(&[9]));
        host.stamp_from(2, &mut forged);
        host.receive(2, &forged, 2);
        assert!(host.blobs.actors.is_empty());
        let mut valid = packed::header(88, 11, META, 1);
        valid.extend(manifest(&[9]));
        let mut elsewhere = valid.clone();
        host.stamp_from(3, &mut elsewhere);
        host.receive(3, &elsewhere, 3);
        assert!(host.blobs.actors.is_empty());
        host.receive(2, &valid, 4);
        assert!(host.blobs.actors.is_empty(), "without the link's token");
        host.stamp_from(2, &mut valid);
        host.receive(2, &valid, 4);
        assert!(host.blobs.actors.contains_key(&11));
    }
    /// I5 (end review 10-12): a modded peer (actor 20) announces another player's look hash as its own and sends junk
    /// for every chunk while the real owner (actor 10) is downloading. The shared cache entry failed, and a failed
    /// entry ignores all later data, so the owner's look never arrived for the whole session.
    #[test]
    fn a_spoofed_failing_download_does_not_block_the_owners_look() {
        let look: Vec<u8> = (0..3 * CHUNK + 5).map(|i| (i * 7) as u8).collect();
        let chunk = |i: usize, bytes: &[u8]| {
            let mut d = (i as u32).to_le_bytes().to_vec();
            d.extend(&bytes[i * CHUNK..((i + 1) * CHUNK).min(bytes.len())]);
            d
        };
        let count = look.len().div_ceil(CHUNK);
        let mut b = Blobs::default();
        b.receive(1, 10, META, 1, &manifest(&look), 0);
        b.receive(2, 20, META, 1, &manifest(&look), 0);
        let junk = vec![0u8; look.len()];
        for i in 0..count {
            b.receive(2, 20, DATA, 1, &chunk(i, &junk), 0);
        }
        assert!(b.ready(20).is_none(), "junk never loads");
        for i in 0..count {
            b.receive(1, 10, DATA, 1, &chunk(i, &look), 0);
        }
        assert_eq!(b.ready(10).map(|(_, bytes)| bytes.to_vec()), Some(look));
    }
    /// P4: announcing a 64 MiB look allocates nothing; chunks past the window are refused; a download that stops
    /// frees what it held.
    #[test]
    fn downloads_grow_with_their_data_and_a_stalled_one_is_freed() {
        let mut b = Blobs::default();
        let mut big = [7u8; 32].to_vec();
        big.extend((MAX_BLOB as u32).to_le_bytes());
        for actor in 10..60 {
            b.receive(1, actor, META, 1, &big, 0);
        }
        assert_eq!(b.partial.len(), 50);
        assert!(b.partial.values().all(|p| p.bytes.is_empty()), "fifty announcements, no memory");
        let chunk = |i: u32| {
            let mut d = i.to_le_bytes().to_vec();
            d.extend([1u8; CHUNK]);
            d
        };
        b.receive(1, 10, DATA, 1, &chunk(WINDOW as u32 + 5), 0);
        assert!(b.partial[&10].bytes.is_empty(), "past the window");
        b.receive(1, 10, DATA, 1, &chunk(3), 0);
        assert_eq!(b.partial[&10].bytes.len(), 4 * CHUNK);
        let members: BTreeSet<u64> = (10..60).collect();
        b.service(1, 1, &members, &[], STALL_MS - 1);
        assert!(!b.partial[&10].failed);
        b.service(1, 1, &members, &[], STALL_MS + 1);
        assert!(b.partial[&10].failed && b.partial[&10].bytes.is_empty(), "stalled: freed");
        b.receive(1, 10, DATA, 1, &chunk(4), STALL_MS + 2);
        assert!(b.partial[&10].bytes.is_empty(), "and stays failed");
    }
    /// perf 10-18: one service call (8 ms after the last) queues at most ~256 kB of a look, ~64 kB of it to any one
    /// peer (it was 2 MB); 64 ms after, the old 2 MB.
    #[test]
    fn one_service_call_sends_a_bounded_slice_of_a_look() {
        let mut b = Blobs::default();
        assert!(b.publish(1, vec![7; 1 << 20]));
        let peers: Vec<_> = (2..10).map(|p| (p, p, 0)).collect();
        let members: BTreeSet<u64> = (1..10).collect();
        b.service(9, 1, &members, &peers, 0);
        let seq = b.actors[&1].0;
        for p in 2..10 {
            let mut ack = 1u64.to_le_bytes().to_vec();
            ack.extend(0u32.to_le_bytes());
            ack.extend([0; MASK]);
            b.receive(p, p, ACK, seq, &ack, 1);
        }
        // Every window open, as on a fast link a while in: the call's caps bind, not the windows.
        for tx in b.sending.values_mut() {
            tx.window = SEND_WINDOW;
        }
        let out = b.service(9, 1, &members, &peers, 2);
        let total: usize = out.iter().map(|o| o.data.len()).sum();
        assert!((200_000..=256_000).contains(&total), "{total}");
        for p in 2..10 {
            assert!(out.iter().filter(|o| o.peer == p).map(|o| o.data.len()).sum::<usize>() <= 64_000);
        }
    }
    /// Ledger #54 (user 10-26): however fast a peer acknowledges, no more of a look is in flight to it than fits a Steam
    /// connection's 512 KB send buffer with room left for movement (the window used to grow to 1 MiB), and on a fast
    /// link the window still opens.
    #[test]
    fn a_peer_never_has_more_than_384_kib_of_a_look_in_flight() {
        let mut b = Blobs::default();
        assert!(b.publish(1, vec![3; 8 << 20]));
        let (members, peers) = (BTreeSet::from([1, 2]), [(2, 2, 0)]);
        b.service(9, 1, &members, &peers, 0);
        let seq = b.actors[&1].0;
        let ack = |b: &mut Blobs, prefix: u32, now: u64| {
            let mut d = 1u64.to_le_bytes().to_vec();
            d.extend(prefix.to_le_bytes());
            d.extend([0; MASK]);
            b.receive(2, 2, ACK, seq, &d, now);
        };
        ack(&mut b, 0, 0);
        let (mut now, mut done, mut acked_to, mut most) = (0, 0u32, 0u32, 0);
        for _ in 0..40 {
            now += 64;
            for o in b.service(9, 1, &members, &peers, now).iter().filter(|o| o.data[24] == DATA) {
                let index = u32::from_le_bytes(o.data[packed::HEADER..packed::HEADER + 4].try_into().unwrap());
                done = done.max(index + 1);
            }
            most = most.max(b.sending[&(2, 1)].sent.len());
            // The acknowledgement comes a call later, as over a real round trip.
            ack(&mut b, acked_to, now);
            acked_to = done;
        }
        assert!((300..=384).contains(&most), "{most} chunks in flight at most");
    }
    /// One peer (2) of a 1 MiB look, accepted; `ack` acknowledges `chunks`, `sent` lists a service call's chunks.
    fn one_peer_sending(now: u64) -> (Blobs, impl Fn(&mut Blobs, std::ops::Range<usize>, u64), impl Fn(&mut Blobs, u64) -> Vec<u32>) {
        let mut b = Blobs::default();
        assert!(b.publish(1, vec![5; 1 << 20]));
        let (members, peers) = (BTreeSet::from([1, 2]), [(2, 2, 0)]);
        b.service(9, 1, &members, &peers, now);
        let seq = b.actors[&1].0;
        let ack = move |b: &mut Blobs, chunks: std::ops::Range<usize>, now: u64| {
            let mut d = 1u64.to_le_bytes().to_vec();
            d.extend(0u32.to_le_bytes());
            let mut mask = [0u8; MASK];
            for i in chunks {
                mask[i / 8] |= 1 << (i % 8);
            }
            d.extend(mask);
            b.receive(2, 2, ACK, seq, &d, now);
        };
        ack(&mut b, 0..0, now);
        let sent = move |b: &mut Blobs, now: u64| {
            let out = b.service(9, 1, &members, &peers, now);
            out.iter()
                .filter(|o| o.data[24] == DATA)
                .map(|o| u32::from_le_bytes(o.data[packed::HEADER..packed::HEADER + 4].try_into().unwrap()))
                .collect()
        };
        (b, ack, sent)
    }
    /// Ledger #54: a chunk still unacknowledged when chunks sent after it come back was lost (RACK, RFC 8985): it goes
    /// again at the next call, not the retry timeout (400 ms at least) after it was sent.
    #[test]
    fn a_chunk_lost_behind_acknowledged_ones_is_sent_again_at_once() {
        let (mut b, ack, sent) = one_peer_sending(10_000);
        assert_eq!(sent(&mut b, 10_050), (0..16).collect::<Vec<_>>());
        ack(&mut b, 1..16, 10_100);
        assert_eq!(sent(&mut b, 10_150), (16..46).collect::<Vec<_>>());
        ack(&mut b, 1..46, 10_200);
        assert_eq!(sent(&mut b, 10_250).first(), Some(&0), "chunk 0 again, 200 ms after it was sent");
    }
    /// Ledger #54 (Karn): a chunk sent twice may be acknowledged for its first copy, so it gives no round trip; one that
    /// did (10 ms here, on a 150 ms link) made every later chunk look queued and shrank the window for the rest of the
    /// look.
    #[test]
    fn a_resent_chunk_gives_no_round_trip() {
        let (mut b, ack, sent) = one_peer_sending(10_000);
        assert_eq!(sent(&mut b, 10_050).len(), 16);
        ack(&mut b, 1..16, 10_200);
        assert_eq!(sent(&mut b, 10_450).first(), Some(&0), "the retry timeout sends chunk 0 again");
        ack(&mut b, 0..16, 10_460);
        ack(&mut b, 0..46, 10_600);
        let window = b.sending[&(2, 1)].window;
        assert!(window > 31, "the window grows on a link that isn't queuing: {window}");
    }
    /// Ledger #54: a chunk marked lost but not sent again yet has no send time ("0"); when its first copy's
    /// acknowledgement turns up late after all it gives no round trip (it gave the whole session clock: halved).
    #[test]
    fn a_chunk_found_before_it_goes_again_gives_no_round_trip() {
        let (mut b, ack, sent) = one_peer_sending(10_000);
        assert_eq!(sent(&mut b, 10_050).len(), 16);
        ack(&mut b, 1..16, 10_100);
        assert_eq!(sent(&mut b, 10_150).len(), 30);
        ack(&mut b, 1..46, 10_200);
        let before = b.sending[&(2, 1)].window;
        ack(&mut b, 0..46, 10_210);
        assert_eq!(b.sending[&(2, 1)].window, before, "chunk 0 arrived late after all: nothing is queued");
    }
    /// Ledger #54 (TLP, RFC 8985): a flight lost whole leaves no later chunk to show it. Two quick round trips without
    /// an acknowledgement, the newest chunk goes again alone, and its acknowledgement sends the rest again at once (they
    /// waited for the 400 ms retry timeout: 5 idle steps at 15 FPS).
    #[test]
    fn a_whole_lost_flight_is_found_by_one_probe() {
        let (mut b, ack, sent) = one_peer_sending(10_000);
        assert_eq!(sent(&mut b, 10_050).len(), 16);
        ack(&mut b, 0..16, 10_100);
        assert_eq!(sent(&mut b, 10_150), (16..48).collect::<Vec<_>>(), "lost on the way");
        assert!(sent(&mut b, 10_200).is_empty(), "one quick round trip: still waiting");
        assert_eq!(sent(&mut b, 10_250), [47], "the probe");
        ack(&mut b, 47..48, 10_300);
        let again = sent(&mut b, 10_350);
        assert!(again.starts_with(&(16..47).collect::<Vec<_>>()), "the rest, 200 ms after they went: {again:?}");
    }
    /// Ledger #54: a lost acknowledgement is found by the same probe. The chunks it covered come back late because of
    /// the loss, not a queue (the probe's own quick acknowledgement says so), so the window grows instead of halving.
    #[test]
    fn a_lost_acknowledgement_costs_one_probe_not_the_window() {
        let (mut b, ack, sent) = one_peer_sending(10_000);
        assert_eq!(sent(&mut b, 10_050).len(), 16);
        ack(&mut b, 0..16, 10_100);
        assert_eq!(sent(&mut b, 10_150).len(), 32);
        assert_eq!(sent(&mut b, 10_300), [47], "the probe");
        ack(&mut b, 0..48, 10_350);
        let window = b.sending[&(2, 1)].window;
        assert!(window > 32, "grown by the chunks acknowledged, not halved: {window}");
    }
    /// Datagrams `from` one endpoint into `to`, as the lobby hands them over (actor and sequence from the header).
    fn deliver(to: &mut Blobs, from: u64, out: &[Outgoing], now: u64) {
        for o in out {
            let actor = u64::from_le_bytes(o.data[16..24].try_into().unwrap());
            let seq = u32::from_le_bytes(o.data[25..29].try_into().unwrap());
            to.receive(from, actor, o.data[24], seq, &o.data[packed::HEADER..], now);
        }
    }
    /// Audit 10-28 #3: a download given up (no chunk for 30 s) still acknowledged only the chunks it held, so its
    /// sender sent the rest again for the whole session. Its acknowledgement now covers the whole look: the sender stops.
    #[test]
    fn a_given_up_download_stops_its_sender() {
        let (mut tx, mut rx) = (Blobs::default(), Blobs::default());
        assert!(tx.publish(1, vec![5; 1 << 20]));
        let members = BTreeSet::from([1, 2]);
        let mut now = 10_000;
        let step = |tx: &mut Blobs, rx: &mut Blobs, now: u64, arrive: bool| {
            let out = tx.service(9, 1, &members, &[(2, 2, 0)], now);
            let data = out.iter().filter(|o| o.data[24] == DATA).count();
            if arrive {
                deliver(rx, 1, &out, now);
            }
            let back = rx.service(9, 2, &members, &[(1, 1, 0)], now);
            deliver(tx, 2, &back, now);
            data
        };
        for _ in 0..4 {
            now += 50;
            step(&mut tx, &mut rx, now, true);
        }
        assert!(rx.partial[&1].prefix > 0, "under way");
        // The link drops for 30 s: nothing arrives, the receiver gives up.
        now += STALL_MS + 100;
        step(&mut tx, &mut rx, now, false);
        assert!(rx.partial[&1].failed);
        // It comes back: one more flight arrives, and its acknowledgement says to stop.
        for _ in 0..3 {
            now += 500;
            step(&mut tx, &mut rx, now, true);
        }
        let mut more = 0;
        for _ in 0..40 {
            now += 500;
            more += step(&mut tx, &mut rx, now, true);
        }
        assert_eq!(more, 0, "the sender stopped");
        assert!(rx.ready(1).is_none(), "and the look stays the default");
    }
    /// Audit 10-28 #3: a download slower than 32 kB/s overall was given up however steadily it came (a big model in a
    /// full lobby). Only one that stops for 30 s is.
    #[test]
    fn a_slow_download_that_keeps_coming_is_not_given_up() {
        let look = vec![4u8; 200 * CHUNK];
        let mut b = Blobs::default();
        b.receive(1, 10, META, 1, &manifest(&look), 0);
        let members = BTreeSet::from([10]);
        // One chunk every 2 s: 0.5 kB/s, for 400 s.
        for i in 0..200usize {
            let now = i as u64 * 2_000;
            let mut d = (i as u32).to_le_bytes().to_vec();
            d.extend(&look[i * CHUNK..(i + 1) * CHUNK]);
            b.receive(1, 10, DATA, 1, &d, now);
            b.service(9, 1, &members, &[], now + 1_000);
            assert!(b.partial.get(&10).is_none_or(|p| !p.failed), "given up at chunk {i}");
        }
        assert_eq!(b.ready(10).map(|(_, bytes)| bytes.len()), Some(look.len()));
    }
    /// Audit 10-28 #4: a look went to every member at once (a 50-player lobby's first flight: 16 chunks to each of 49,
    /// ~800 kB), filling the upload that movement and voice share. It goes to SENDING_TO members at a time; the next
    /// starts when one has it all.
    #[test]
    fn a_look_goes_to_a_few_members_at_a_time() {
        let mut b = Blobs::default();
        assert!(b.publish(1, vec![9; 40 * CHUNK]));
        let peers: Vec<_> = (2..51).map(|p| (p, p, 0)).collect();
        let members: BTreeSet<u64> = (1..51).collect();
        b.service(9, 1, &members, &peers, 0);
        let seq = b.actors[&1].0;
        let ack = |b: &mut Blobs, p: u64, prefix: u32, now: u64| {
            let mut d = 1u64.to_le_bytes().to_vec();
            d.extend(prefix.to_le_bytes());
            d.extend([0; MASK]);
            b.receive(p, p, ACK, seq, &d, now);
        };
        for p in 2..51 {
            ack(&mut b, p, 0, 1);
        }
        let to = |out: &[Outgoing]| out.iter().filter(|o| o.data[24] == DATA).map(|o| o.peer).collect::<BTreeSet<_>>();
        let first = to(&b.service(9, 1, &members, &peers, 50));
        assert_eq!(first.len(), SENDING_TO, "{first:?}");
        let done = *first.iter().next().unwrap();
        ack(&mut b, done, 40, 100);
        let next = to(&b.service(9, 1, &members, &peers, 150));
        assert_eq!(next.len(), 1, "one more starts: {next:?}");
        assert!(!next.contains(&done) && !first.contains(next.iter().next().unwrap()));
    }
    /// End review 10-28 (C1): a member waiting its turn (SENDING_TO) gets no chunk for a while; counted from its META,
    /// that was "stalled", so its download was given up and its turn acknowledged the whole look: the default look for
    /// the session. A download not started yet is never given up (it holds no memory).
    #[test]
    fn a_download_waiting_its_turn_is_not_given_up() {
        let look = vec![6u8; 3 * CHUNK];
        let mut b = Blobs::default();
        b.receive(1, 10, META, 1, &manifest(&look), 0);
        let members = BTreeSet::from([10]);
        b.service(9, 1, &members, &[], STALL_MS * 3);
        assert!(!b.partial[&10].failed, "waiting, not stalled");
        for i in 0..3usize {
            let mut d = (i as u32).to_le_bytes().to_vec();
            d.extend(&look[i * CHUNK..(i + 1) * CHUNK]);
            b.receive(1, 10, DATA, 1, &d, STALL_MS * 3 + 1);
        }
        assert_eq!(b.ready(10).map(|(_, bytes)| bytes.len()), Some(look.len()), "its turn came: the look");
    }
    /// End review 10-28 (C1): a transfer that resumes (its link dropped and came back: the member holds part of the
    /// look) waited behind SENDING_TO others and stalled out; it goes on at once.
    #[test]
    fn a_resuming_transfer_does_not_wait_its_turn() {
        let mut b = Blobs::default();
        assert!(b.publish(1, vec![9; 40 * CHUNK]));
        let peers: Vec<_> = (2..8).map(|p| (p, p, 0)).collect();
        let members: BTreeSet<u64> = (1..8).collect();
        b.service(9, 1, &members, &peers, 0);
        let seq = b.actors[&1].0;
        let ack = |b: &mut Blobs, p: u64, prefix: u32, now: u64| {
            let mut d = 1u64.to_le_bytes().to_vec();
            d.extend(prefix.to_le_bytes());
            d.extend([0; MASK]);
            b.receive(p, p, ACK, seq, &d, now);
        };
        for p in 2..7 {
            ack(&mut b, p, 0, 1);
        }
        ack(&mut b, 7, 10, 1);
        let to: BTreeSet<u64> = b.service(9, 1, &members, &peers, 50).iter().filter(|o| o.data[24] == DATA).map(|o| o.peer).collect();
        assert!(to.contains(&7), "the one holding 10 chunks goes on: {to:?}");
        assert_eq!(to.len(), SENDING_TO + 1, "{to:?}");
    }
    /// End review 10-28 (M1): a member that acknowledges META but never a chunk (a modded client, or one whose memory
    /// stays full) held one of the SENDING_TO places for the session; four of them stopped every look. A transfer with
    /// no progress for STALL_MS no longer holds one.
    #[test]
    fn a_transfer_that_never_moves_frees_its_place() {
        let mut b = Blobs::default();
        assert!(b.publish(1, vec![9; 40 * CHUNK]));
        let peers: Vec<_> = (2..7).map(|p| (p, p, 0)).collect();
        let members: BTreeSet<u64> = (1..7).collect();
        b.service(9, 1, &members, &peers, 0);
        let seq = b.actors[&1].0;
        for p in 2..7 {
            let mut d = 1u64.to_le_bytes().to_vec();
            d.extend(0u32.to_le_bytes());
            d.extend([0; MASK]);
            b.receive(p, p, ACK, seq, &d, 1);
        }
        let to = |out: Vec<Outgoing>| out.iter().filter(|o| o.data[24] == DATA).map(|o| o.peer).collect::<BTreeSet<_>>();
        let first = to(b.service(9, 1, &members, &peers, 50));
        assert_eq!(first.len(), SENDING_TO);
        // None of the four acknowledges a chunk for STALL_MS: the fifth starts.
        let waiting = *(2..7).collect::<BTreeSet<u64>>().difference(&first).next().unwrap();
        let later = to(b.service(9, 1, &members, &peers, 50 + STALL_MS + 1));
        assert!(later.contains(&waiting), "{later:?}");
    }
    /// 3.7: no bytes panic the blob decoder.
    #[test]
    fn random_blob_datagrams_never_panic() {
        let mut b = Blobs::default();
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let members: BTreeSet<u64> = (0..4).collect();
        for i in 0..100_000u64 {
            let len = [0, 5, 36, 12 + MASK, CHUNK + 4, (next() % 1200) as usize][i as usize % 6];
            let mut d: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if len >= 36 && i % 3 == 0 {
                d[32..36].copy_from_slice(&((next() % 8192) as u32).to_le_bytes());
            }
            if len >= 4 && i % 3 == 1 {
                d[..4].copy_from_slice(&((next() % 8) as u32).to_le_bytes());
            }
            b.receive(next() % 4, next() % 4, [META, DATA, ACK][i as usize % 3], (next() % 3) as u32, &d, i);
            if i % 997 == 0 {
                b.service(1, 0, &members, &[(1, 1, 1), (2, 2, 2)], i);
            }
        }
    }
}
struct Blob {
    id: Identity,
    /// A download's bytes grow as chunks arrive (never past `prefix + WINDOW` chunks), not the announced size at META.
    bytes: Arc<Vec<u8>>,
    received: Vec<bool>,
    prefix: usize,
    complete: bool,
    /// Bad bytes or stalled: its memory is freed and it never retries (until a new META).
    failed: bool,
    /// When the download last got a new chunk.
    last: u64,
}
impl Blob {
    fn count(&self) -> usize {
        self.id.size.div_ceil(CHUNK)
    }
}
struct Sending {
    seq: u32,
    /// The actor at that endpoint: a quick rejoin from the same endpoint is a new recipient (M7, end review 10-12).
    recipient: u64,
    meta_at: Option<u64>,
    accepted: bool,
    prefix: usize,
    mask: [u8; MASK],
    sent: BTreeMap<usize, u64>,
    /// Chunks in `sent` that went (or are due to go) more than once: an acknowledgement may be the first copy's (Karn).
    resent: BTreeSet<usize>,
    window: usize,
    last_loss: u64,
    /// The quickest a chunk came back acknowledged (ms): the round trip with nothing queued ([`QUEUE_MS`]).
    fastest: u64,
    /// When an acknowledgement last moved the look on, or a probe last went (TLP in [`Blobs::service`]).
    quiet_since: u64,
    /// Its chunks have started going: it holds one of the [`SENDING_TO`] places until it has them all, while it moves.
    under_way: bool,
    /// When it started, or an acknowledgement last took chunks off it: STALL_MS without, and it holds no place (end
    /// review 10-28 M1: one that never acknowledges a chunk held it for the session).
    moved: u64,
}
impl Sending {
    fn new(seq: u32, recipient: u64) -> Self {
        Self {
            seq,
            recipient,
            under_way: false,
            moved: 0,
            meta_at: None,
            accepted: false,
            prefix: 0,
            mask: [0; MASK],
            sent: BTreeMap::new(),
            resent: BTreeSet::new(),
            // Ledger #54: 16 KiB first (was 64): the first flight alone queued ~250 ms of movement on a 256 KB/s link.
            window: 16,
            last_loss: 0,
            fastest: u64::MAX,
            quiet_since: 0,
        }
    }
    fn has(&self, i: usize) -> bool {
        i < self.prefix
            || (i - self.prefix < WINDOW
                && self.mask[(i - self.prefix) / 8] & (1 << ((i - self.prefix) % 8)) != 0)
    }
}
#[derive(Default)]
pub struct Blobs {
    actors: BTreeMap<u64, (u32, [u8; 32])>,
    /// Complete, verified looks by hash (shared by every actor that wears one).
    cache: BTreeMap<[u8; 32], Blob>,
    /// I5 (end review 10-12): downloads in progress, one per sending actor; one moves to `cache` only once its hash
    /// verifies, so a peer that sends junk under another's hash fails only its own.
    partial: BTreeMap<u64, Blob>,
    sending: BTreeMap<(u64, u64), Sending>,
    acknowledgements: BTreeSet<(u64, u64)>,
    round: usize,
    /// perf 10-18: when `service` last ran; its byte caps scale with the time since.
    last_service: u64,
}
impl Blobs {
    fn reserve(&mut self, size: usize) -> bool {
        let mut used: usize = self.cache.values().chain(self.partial.values()).map(|b| b.bytes.len()).sum();
        let unused: Vec<_> = self
            .cache
            .keys()
            .filter(|hash| !self.actors.values().any(|(_, h)| h == *hash))
            .copied()
            .collect();
        for hash in unused {
            if used + size <= MEMORY {
                break;
            }
            used -= self.cache.remove(&hash).unwrap().id.size;
        }
        used + size <= MEMORY
    }
    pub fn publish(&mut self, actor: u64, bytes: Vec<u8>) -> bool {
        let hash = digest(&bytes);
        self.publish_shared(actor, Arc::new(bytes), hash)
    }
    /// perf 10-18: [`Self::publish`] of bytes whose [`digest`] the caller kept (our own look, again each session).
    pub fn publish_shared(&mut self, actor: u64, bytes: Arc<Vec<u8>>, hash: [u8; 32]) -> bool {
        if bytes.is_empty() || bytes.len() > MAX_BLOB {
            return false;
        }
        if self.actors.get(&actor).is_some_and(|(_, h)| *h == hash) {
            return true;
        }
        if self.cache.get(&hash).is_none_or(|b| !b.complete) {
            // P4: our own look is never refused for others' downloads; this only evicts unused looks to make room.
            // ponytail: so the cache can hold one look (ours) past MEMORY.
            if !self.cache.contains_key(&hash) {
                self.reserve(bytes.len());
            }
            let count = bytes.len().div_ceil(CHUNK);
            self.cache.insert(
                hash,
                Blob {
                    id: Identity {
                        hash,
                        size: bytes.len(),
                    },
                    bytes,
                    received: vec![],
                    prefix: count,
                    complete: true,
                    failed: false,
                    last: 0,
                },
            );
        }
        let seq = self
            .actors
            .get(&actor)
            .map_or(1, |(s, _)| s.saturating_add(1));
        self.actors.insert(actor, (seq, hash));
        true
    }
    /// Audit 10-24: `peer` (None: every peer) dropped our actor and its look while we counted it delivered (it timed us
    /// out, or we it): offer the look again. One META each when it still has it.
    pub(crate) fn forget(&mut self, peer: Option<u64>) {
        self.sending.retain(|(p, _), _| peer.is_some_and(|peer| *p != peer));
    }
    pub fn ready(&self, actor: u64) -> Option<(&Identity, &[u8])> {
        let (_, hash) = self.actors.get(&actor)?;
        let b = self.cache.get(hash)?;
        b.complete.then_some((&b.id, b.bytes.as_slice()))
    }
    pub fn progress(&self, local: u64) -> (usize, usize) {
        self.partial
            .iter()
            .filter(|(id, _)| **id != local)
            .map(|(_, b)| b)
            .fold((0, 0), |(done, total), b| {
                (
                    done + b.received.iter().filter(|v| **v).count(),
                    total + b.count(),
                )
            })
    }
    // The lobby authenticates origin/peer and bounds membership before calling this.
    pub(crate) fn receive(&mut self, peer: u64, actor: u64, kind: u8, seq: u32, data: &[u8], now: u64) {
        if kind == META {
            if data.len() != 36 || seq == 0 {
                return;
            }
            let hash: [u8; 32] = data[..32].try_into().unwrap();
            let size = u32::from_le_bytes(data[32..].try_into().unwrap()) as usize;
            if size == 0 || size > MAX_BLOB {
                return;
            }
            if self
                .actors
                .get(&actor)
                .is_some_and(|(old, h)| seq < *old || (seq == *old && hash != *h))
            {
                return;
            }
            if let Some(b) = self.cache.get(&hash) {
                if b.id.size != size {
                    return;
                }
                self.partial.remove(&actor);
            } else if let Some(b) = self.partial.get(&actor).filter(|b| b.id.hash == hash) {
                if b.id.size != size {
                    return;
                }
            } else {
                // One download per sender (keyed by actor); nothing is allocated for it until its chunks arrive (P4).
                self.partial.insert(
                    actor,
                    Blob {
                        id: Identity { hash, size },
                        bytes: Arc::new(vec![]),
                        received: vec![false; size.div_ceil(CHUNK)],
                        prefix: 0,
                        complete: false,
                        failed: false,
                        last: now,
                    },
                );
            }
            self.actors.insert(actor, (seq, hash));
            self.acknowledgements.insert((peer, actor));
        } else if kind == DATA {
            let Some((current, hash)) = self.actors.get(&actor) else {
                return;
            };
            if seq != *current || data.len() < 5 {
                return;
            }
            let hash = *hash;
            // Verified already (ours, or another sender's): just say so.
            if self.cache.contains_key(&hash) {
                self.partial.remove(&actor);
                self.acknowledgements.insert((peer, actor));
                return;
            }
            let Some(b) = self.partial.get(&actor).filter(|b| b.id.hash == hash) else {
                return;
            };
            let index = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
            // A sender never runs more than WINDOW chunks past what we acknowledged, so memory grows with the data.
            if index >= b.count() || index >= b.prefix + WINDOW || data.len() - 4 != CHUNK.min(b.id.size - index * CHUNK) {
                return;
            }
            if b.failed || b.received[index] {
                self.acknowledgements.insert((peer, actor));
                return;
            }
            let end = index * CHUNK + data.len() - 4;
            let grow = end.saturating_sub(b.bytes.len());
            let partial: usize = self.partial.values().map(|b| b.bytes.len()).sum();
            // Out of room: dropped unacknowledged, so the sender retries once a look is freed (our wait, not its stall).
            if grow > 0 && (partial + grow > PARTIAL_MEMORY || !self.reserve(grow)) {
                if let Some(b) = self.partial.get_mut(&actor) {
                    b.last = now;
                }
                return;
            }
            self.acknowledgements.insert((peer, actor));
            let b = self.partial.get_mut(&actor).unwrap();
            let bytes = Arc::get_mut(&mut b.bytes).unwrap();
            if bytes.len() < end {
                bytes.resize(end, 0);
            }
            bytes[index * CHUNK..end].copy_from_slice(&data[4..]);
            b.received[index] = true;
            b.last = now;
            while b.prefix < b.count() && b.received[b.prefix] {
                b.prefix += 1;
            }
            if b.prefix == b.count() {
                b.complete = blake3::hash(&b.bytes).as_bytes() == &b.id.hash;
                b.failed = !b.complete;
                if b.failed {
                    b.bytes = Arc::default();
                }
                // Bad bytes never reach the asset loader or trigger an endless retry.
                if b.complete {
                    let b = self.partial.remove(&actor).unwrap();
                    self.cache.entry(hash).or_insert(b);
                }
            }
        } else if kind == ACK {
            if data.len() != 12 + MASK {
                return;
            }
            let origin = u64::from_le_bytes(data[..8].try_into().unwrap());
            let prefix = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
            let Some((current, hash)) = self.actors.get(&origin) else {
                return;
            };
            // We only send verified looks; an ACK naming anything else is noise.
            let Some(count) = self.cache.get(hash).map(Blob::count) else {
                return;
            };
            let Some(tx) = self.sending.get_mut(&(peer, origin)) else {
                return;
            };
            if seq != *current || seq != tx.seq || prefix > count || prefix < tx.prefix {
                return;
            }
            if prefix == tx.prefix {
                for (a, b) in tx.mask.iter_mut().zip(&data[12..]) {
                    *a |= *b;
                }
            } else {
                tx.mask.copy_from_slice(&data[12..]);
                tx.prefix = prefix;
            }
            tx.accepted = true;
            let acked: Vec<_> = tx.sent.iter().filter(|(i, _)| tx.has(**i)).map(|(&i, &at)| (i, at)).collect();
            // The quickest of the batch: a frame's jitter delays some chunks, a queue delays them all. A chunk sent again
            // counts only once that copy went (one marked lost below waits at "0") and if it came back no quicker than
            // the fastest round trip; sooner, it was the first copy's (Karn, as RFC 8985 applies it).
            let trip = acked
                .iter()
                .filter(|(i, at)| !tx.resent.contains(i) || (*at > 0 && now.saturating_sub(*at) >= tx.fastest))
                .map(|(_, at)| now.saturating_sub(*at))
                .min();
            for (i, _) in &acked {
                tx.sent.remove(i);
                tx.resent.remove(i);
            }
            if !acked.is_empty() {
                tx.quiet_since = now;
                tx.moved = now;
            }
            if let Some(trip) = trip {
                tx.fastest = tx.fastest.min(trip);
                let queued = trip - tx.fastest;
                // Ledger #54: these waited in a queue movement shares; halve once a round trip, hold when near it.
                if queued > QUEUE_MS {
                    if now.saturating_sub(tx.last_loss) >= trip {
                        tx.window = (tx.window / 2).max(MIN_WINDOW);
                        tx.last_loss = now;
                    }
                } else if queued < QUEUE_MS / 2 {
                    tx.window = (tx.window + acked.len()).min(SEND_WINDOW);
                }
                // Ledger #54 (RACK, RFC 8985): one still out that went well before this batch's newest was lost. Sent at
                // "0" it is overdue (the next call sends it, once the session clock is past the retry timeout).
                let newest = now - trip;
                for (i, at) in tx.sent.iter_mut() {
                    if *at + tx.fastest / 4 < newest {
                        *at = 0;
                        tx.resent.insert(*i);
                    }
                }
            }
        }
    }
    pub(crate) fn service(
        &mut self,
        session: u64,
        local: u64,
        members: &BTreeSet<u64>,
        peers: &[(u64, u64, u64)],
        now: u64,
    ) -> Vec<Outgoing> {
        self.actors.retain(|id, _| members.contains(id));
        self.partial.retain(|id, _| members.contains(id));
        // Each member sends only its own look, so a look nobody here wears is never sent on: free it now, not at MEMORY.
        self.cache.retain(|hash, _| self.actors.values().any(|(_, h)| h == hash));
        // P4: a download that stops frees its memory instead of holding it for the session.
        // ponytail: it then stays failed: that look shows as the default until its owner publishes another.
        // End review 10-28 (C1): one not started (no bytes: it waits its turn, SENDING_TO) is never given up.
        for b in self.partial.values_mut() {
            if !b.failed && !b.bytes.is_empty() && now.saturating_sub(b.last) > STALL_MS {
                b.failed = true;
                b.bytes = Arc::default();
            }
        }
        self.sending.retain(|(peer, id), _| {
            members.contains(id) && peers.iter().any(|(p, _, _)| p == peer)
        });
        let mut out = vec![];
        for (peer, origin) in std::mem::take(&mut self.acknowledgements) {
            if !peers.iter().any(|(p, _, _)| *p == peer) {
                continue;
            }
            let Some((seq, hash)) = self.actors.get(&origin) else {
                continue;
            };
            let Some(b) = self.cache.get(hash).or_else(|| self.partial.get(&origin).filter(|b| b.id.hash == *hash)) else {
                continue;
            };
            // Audit 10-28 #3: one given up says it holds everything, so its sender stops (it sent the rest again for
            // the whole session).
            let prefix = if b.failed { b.count() } else { b.prefix };
            let mut data = packed::header(session, local, ACK, *seq);
            data.extend(origin.to_le_bytes());
            data.extend((prefix as u32).to_le_bytes());
            let mut mask = [0u8; MASK];
            for i in prefix..(prefix + WINDOW).min(b.count()) {
                if b.complete || b.received[i] {
                    mask[(i - prefix) / 8] |= 1 << ((i - prefix) % 8);
                }
            }
            data.extend(mask);
            out.push(Outgoing { peer, data });
        }
        // ACKs and the loss- and delay-responsive window control throughput, rather than
        // a fixed bytes/second cap. Bound each service call so model uploads
        // cannot monopolize the game thread or overflow the local IPC queues.
        // perf 10-18: 256 kB a call, 64 kB a peer (were 2 MB / 512 kB: one call could queue 2 MB on the game thread),
        // per 8 ms since the last call, up to 8x (the old caps at 15 FPS), so a low frame rate keeps its throughput.
        let scale = (now.saturating_sub(self.last_service).clamp(8, 64) / 8) as usize;
        self.last_service = now;
        let mut global = crate::knob!("SKATE_NET_BLOB_KB", 256) as usize * 1000 * scale;
        let mut peers = peers.to_vec();
        if !peers.is_empty() {
            let n = peers.len();
            peers.rotate_left(self.round % n);
        }
        self.round = self.round.wrapping_add(1);
        // Our look's transfers under way, out of SENDING_TO (one reset below, for a new look or a rejoin, starts over).
        let mut under_way = match self.actors.get(&local).and_then(|(seq, hash)| Some((*seq, self.cache.get(hash)?.count()))) {
            Some((seq, count)) => self
                .sending
                .iter()
                .filter(|((_, origin), tx)| {
                    *origin == local && tx.seq == seq && tx.under_way && tx.prefix < count && now.saturating_sub(tx.moved) <= STALL_MS
                })
                .count(),
            None => 0,
        };
        'peers: for (peer, recipient, rtt) in peers {
            if recipient == 0 {
                continue;
            }
            let mut budget = crate::knob!("SKATE_NET_BLOB_KB", 256) as usize * 250 * scale;
            // Only our own look: every member sends its own (peer to peer).
            let mut origins: Vec<_> = self.actors.keys().copied().filter(|&id| id != recipient && id == local).collect();
            if !origins.is_empty() {
                let n = origins.len();
                origins.rotate_left(self.round % n);
            }
            for origin in &origins {
                let (seq, hash) = self.actors[origin];
                let b = &self.cache[&hash];
                let tx = self
                    .sending
                    .entry((peer, *origin))
                    .or_insert_with(|| Sending::new(seq, recipient));
                if tx.seq != seq || tx.recipient != recipient {
                    *tx = Sending::new(seq, recipient);
                }
                if !tx.accepted && tx.meta_at.is_none_or(|at| now.saturating_sub(at) >= 500) {
                    let mut data = packed::header(session, *origin, META, seq);
                    data.extend(hash);
                    data.extend((b.id.size as u32).to_le_bytes());
                    out.push(Outgoing { peer, data });
                    tx.meta_at = Some(now);
                }
            }
            // Round robin chunks across actors, with a sliding selective-ACK window.
            let mut candidates = vec![];
            for origin in origins {
                let (_, hash) = self.actors[&origin];
                let b = &self.cache[&hash];
                let tx = self.sending.get_mut(&(peer, origin)).unwrap();
                if !tx.accepted || b.failed {
                    continue;
                }
                if !tx.under_way {
                    // End review 10-28 (C1): one resuming (the member holds part of it: its link dropped and came back)
                    // goes on at once; waiting, it stalled out.
                    let resuming = tx.prefix > 0 || tx.mask.iter().any(|&m| m != 0);
                    if under_way >= SENDING_TO && !resuming {
                        continue;
                    }
                    (tx.under_way, tx.moved) = (true, now);
                    under_way += 1;
                }
                let retry = (rtt * 3 + 200).clamp(400, 2000);
                if now.saturating_sub(tx.last_loss) >= retry
                    && tx.sent.values().any(|at| now.saturating_sub(*at) >= retry)
                {
                    // Never above a window the queue delay already cut lower.
                    tx.window = (tx.window / 2).max(32).min(tx.window);
                    tx.last_loss = now;
                }
                // Ledger #54 (TLP, RFC 8985): nothing acknowledged for two quick round trips (a flight or its
                // acknowledgement lost whole): the newest chunk goes again, and the acknowledgement it brings shows what
                // is missing.
                let probe = (now.saturating_sub(tx.quiet_since) > tx.fastest.saturating_mul(2))
                    .then(|| tx.sent.iter().max_by_key(|(_, at)| **at).map(|(i, _)| *i))
                    .flatten();
                if probe.is_some() {
                    tx.quiet_since = now;
                }
                // Ledger #54: `window` caps the chunks in flight, not a range from the prefix, so one lost chunk doesn't
                // stall the rest until it is sent again; new ones run on to the receiver's WINDOW.
                let mut room = tx.window.saturating_sub(tx.sent.len());
                let chunks: Vec<_> = (tx.prefix..(tx.prefix + WINDOW).min(b.count()))
                    .filter(|&i| {
                        !tx.has(i)
                            && (b.complete || b.received[i])
                            && match tx.sent.get(&i) {
                                Some(at) => now.saturating_sub(*at) >= retry || probe == Some(i),
                                None if room > 0 => {
                                    room -= 1;
                                    true
                                }
                                None => false,
                            }
                    })
                    .collect();
                candidates.push((origin, chunks.into_iter()));
            }
            loop {
                let mut sent = false;
                for (origin, indices) in &mut candidates {
                    let Some(i) = indices.next() else {
                        continue;
                    };
                    let (seq, hash) = self.actors[origin];
                    let b = &self.cache[&hash];
                    let start = i * CHUNK;
                    let end = (start + CHUNK).min(b.id.size);
                    let size = packed::HEADER + 4 + end - start;
                    if global < size {
                        return out;
                    }
                    if budget < size {
                        continue 'peers;
                    }
                    let mut data = packed::header(session, *origin, DATA, seq);
                    data.extend((i as u32).to_le_bytes());
                    data.extend(&b.bytes[start..end]);
                    let tx = self.sending.get_mut(&(peer, *origin)).unwrap();
                    if tx.sent.insert(i, now).is_some() {
                        tx.resent.insert(i);
                    }
                    budget -= size;
                    global -= size;
                    out.push(Outgoing { peer, data });
                    sent = true;
                }
                if !sent {
                    break;
                }
            }
        }
        out
    }
}
