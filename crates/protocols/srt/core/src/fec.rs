use bytes::Bytes;

/// Recover a single missing packet from a group protected by XOR parity.
///
/// `packets` contains the group members (data and parity) in order. If exactly
/// one member is `None`, the function returns the XOR of all present members,
/// which is the byte-for-byte content of the missing packet.
///
/// - 0 missing: `None` (no recovery needed)
/// - 1 missing: `Some(recovered)`
/// - 2+ missing: `None` (XOR cannot recover more than one)
///
/// All payloads in the group are expected to be the same length in normal use;
/// shorter payloads are zero-padded for the XOR operation.
///
/// SRT 协议中的 XOR 前向纠错恢复单包丢失。
pub fn xor_recover_one(packets: &[Option<Bytes>]) -> Option<Bytes> {
    let missing_count = packets.iter().filter(|p| p.is_none()).count();
    if missing_count != 1 {
        return None;
    }

    let max_len = packets
        .iter()
        .filter_map(|p| p.as_ref())
        .map(|p| p.len())
        .max()
        .unwrap_or(0);

    if max_len == 0 {
        return Some(Bytes::new());
    }

    let mut recovered = vec![0u8; max_len];
    for packet in packets.iter().filter_map(|p| p.as_ref()) {
        for (i, byte) in packet.iter().enumerate() {
            recovered[i] ^= byte;
        }
    }

    Some(Bytes::from(recovered))
}

/// Compute the FEC matrix block group id for a packet sequence number.
///
/// The matrix has `cols` columns and `rows` rows, so each block contains
/// `cols * rows` packets. A group therefore receives one set of row/column
/// parity packets before the next block starts.
///
/// Returns `0` when `cols` or `rows` is `0` to avoid division by zero.
///
/// SRT 包序列号到 FEC 矩阵块组 id 的映射。
pub fn fec_group_id(seq: u32, cols: u32, rows: u32) -> u32 {
    if cols == 0 || rows == 0 {
        return 0;
    }
    seq / (cols * rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_recover_one_missing_data() {
        let p0 = Bytes::from_static(b"abcd");
        let p1 = Bytes::from_static(b"1234");
        let p2 = Bytes::from_static(b"wxyz");
        let parity = {
            let mut out = vec![0u8; 4];
            for p in [&p0, &p1, &p2] {
                for (i, b) in p.iter().enumerate() {
                    out[i] ^= b;
                }
            }
            Bytes::from(out)
        };

        let mut group = vec![
            Some(p0.clone()),
            Some(p1.clone()),
            Some(p2.clone()),
            Some(parity),
        ];
        group[1] = None;
        let recovered = xor_recover_one(&group).expect("one missing should be recoverable");
        assert_eq!(recovered, p1);
    }

    #[test]
    fn xor_recover_one_missing_parity() {
        let p0 = Bytes::from_static(b"abcd");
        let p1 = Bytes::from_static(b"1234");
        let p2 = Bytes::from_static(b"wxyz");

        let parity = {
            let mut out = vec![0u8; 4];
            for p in [&p0, &p1, &p2] {
                for (i, b) in p.iter().enumerate() {
                    out[i] ^= b;
                }
            }
            Bytes::from(out)
        };

        let group = vec![Some(p0), Some(p1), Some(p2), None];
        let recovered = xor_recover_one(&group).expect("parity should be recoverable");
        assert_eq!(recovered, parity);
    }

    #[test]
    fn xor_recover_one_zero_missing_returns_none() {
        let group = vec![
            Some(Bytes::from_static(b"abcd")),
            Some(Bytes::from_static(b"1234")),
        ];
        assert!(xor_recover_one(&group).is_none());
    }

    #[test]
    fn xor_recover_one_two_missing_returns_none() {
        let group = vec![Some(Bytes::from_static(b"abcd")), None, None];
        assert!(xor_recover_one(&group).is_none());
    }

    #[test]
    fn fec_group_id_maps_consecutive_blocks() {
        assert_eq!(fec_group_id(0, 10, 5), 0);
        assert_eq!(fec_group_id(49, 10, 5), 0);
        assert_eq!(fec_group_id(50, 10, 5), 1);
        assert_eq!(fec_group_id(99, 10, 5), 1);
        assert_eq!(fec_group_id(100, 10, 5), 2);
    }

    #[test]
    fn fec_group_id_zero_dimensions_is_safe() {
        assert_eq!(fec_group_id(7, 0, 5), 0);
        assert_eq!(fec_group_id(7, 10, 0), 0);
        assert_eq!(fec_group_id(7, 0, 0), 0);
    }
}
