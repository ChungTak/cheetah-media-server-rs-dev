//! Minimal STUN parser used by the multi-shard front-end.
//!
//! We only need to extract the *local* ICE ufrag from an incoming
//! STUN binding request so the front-end can dispatch the packet to
//! the shard that owns that ufrag. We do **not** validate the message
//! integrity — that is `str0m`'s job inside the owner shard. The
//! parser intentionally avoids pulling a full STUN crate to keep the
//! driver dependency surface small.
//!
//! ICE STUN binding requests carry a `USERNAME` attribute whose value
//! is `<local-ufrag>:<remote-ufrag>` (RFC 8445 §7.2.2). The front-end
//! cares about `local-ufrag` (the first part, before the colon)
//! because that is what the owner shard registered with the
//! [`crate::directory::RouteDirectory`].
//!
//! 多 shard 前端使用的最小 STUN 解析器。
//!
//! 我们只需要从传入的 STUN 绑定请求中提取 *local* ICE ufrag ，以便前端可以将数据包分派到拥有该 ufrag 的 shard 。
//! 我们**不**验证消息完整性——这是所有者 shard 内部 `str0m` 的工作。
//! 解析器有意避免拉出完整的 STUN crate，以保持 driver 依赖面较小。
//!
//! ICE STUN 绑定请求携带一个 `USERNAME` 属性，其值为 `<local-ufrag>:<remote-ufrag>` (RFC 8445 §7.2.2)。
//! 前端关心 `local-ufrag` （第一部分，冒号之前），因为这是所有者 shard 在 [`crate::directory::RouteDirectory`] 中注册的内容。

const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const STUN_HEADER_LEN: usize = 20;
const STUN_BINDING_METHOD_REQUEST: u16 = 0x0001;
const STUN_ATTR_USERNAME: u16 = 0x0006;

/// Try to extract the local ICE ufrag from a STUN binding request.
///
/// Returns `None` if the payload is not a STUN binding request, the
/// USERNAME attribute is missing, or the username does not contain a
/// `:` separator. Bound to the worst-case STUN message length so a
/// pathological packet cannot pin the front-end on parsing.
///
/// RFC 8445 §7.2.2: the USERNAME attribute in a binding request is
/// `<local-ufrag>:<remote-ufrag>` where "local" is the receiver's
/// ICE ufrag (the one the shard registered with the directory) and
/// "remote" is the sender's ICE ufrag.
///
/// 尝试从 STUN 绑定请求中提取本地 ICE ufrag。
///
/// 如果有效负载不是 STUN 绑定请求、缺少 USERNAME 属性或用户名不包含 `:` 分隔符，则返回 `None`。
/// 绑定到最坏情况的 STUN 消息长度，因此病态数据包无法在解析时固定前端。
///
/// RFC 8445 §7.2.2：绑定请求中的 USERNAME 属性是 `<local-ufrag>:<remote-ufrag>`，其中“本地”是接收方的 ICE ufrag（shard 在目录中注册的）
/// ，“远程”是发送方的 ICE ufrag。
pub(crate) fn extract_local_ufrag(bytes: &[u8]) -> Option<String> {
    if bytes.len() < STUN_HEADER_LEN {
        return None;
    }
    // First two bits must be zero on a STUN message; first 4 bits
    // being 0x00 distinguishes STUN from DTLS (0x14-0x17) / RTP
    // (0x80-0xBF) / RTCP (0xC0-0xCF) at the demux layer.
    if bytes[0] & 0xC0 != 0 {
        return None;
    }
    let msg_type = u16::from_be_bytes([bytes[0], bytes[1]]);
    // Method = lower 12 bits with the four class bits split across
    // bits 4 (M3) / 8 (C0) / 5..7 (M0..M2) / 9..15 (M4..M11). For our
    // narrow purpose (binding request only) it is enough to check
    // `method == 0x0001` and `class == 0` (request).
    let method = (msg_type & 0x000F) | ((msg_type & 0x00E0) >> 1) | ((msg_type & 0x3E00) >> 2);
    let class = ((msg_type & 0x0010) >> 4) | ((msg_type & 0x0100) >> 7);
    if method != STUN_BINDING_METHOD_REQUEST || class != 0 {
        return None;
    }
    let length = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
    let cookie = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if cookie != STUN_MAGIC_COOKIE {
        return None;
    }
    let body_end = STUN_HEADER_LEN.checked_add(length)?;
    if body_end > bytes.len() {
        return None;
    }

    let mut cursor = STUN_HEADER_LEN;
    while cursor + 4 <= body_end {
        let attr_type = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]);
        let attr_len = u16::from_be_bytes([bytes[cursor + 2], bytes[cursor + 3]]) as usize;
        let value_start = cursor + 4;
        let value_end = value_start.checked_add(attr_len)?;
        if value_end > body_end {
            return None;
        }
        if attr_type == STUN_ATTR_USERNAME {
            let username = std::str::from_utf8(&bytes[value_start..value_end]).ok()?;
            // RFC 8445 §7.2.2: USERNAME = <local-ufrag>:<remote-ufrag>
            // where "local" is the receiver's ICE ufrag (the one the
            // shard registered with the directory) and "remote" is the
            // sender's ICE ufrag.
            let (local, _remote) = username.split_once(':')?;
            if local.is_empty() {
                return None;
            }
            return Some(local.to_string());
        }
        // Attributes are 4-byte aligned; advance with padding.
        let padded = (attr_len + 3) & !3;
        cursor = value_start.checked_add(padded)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal STUN binding request with the given USERNAME
    /// attribute. We intentionally do NOT compute MESSAGE-INTEGRITY
    /// because the front-end only needs the username.
    fn make_binding_request(username: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        // Type: Binding Request = 0x0001
        buf.extend_from_slice(&0x0001u16.to_be_bytes());
        // Length placeholder
        buf.extend_from_slice(&0u16.to_be_bytes());
        // Magic cookie
        buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        // Transaction ID (12 bytes of arbitrary data)
        buf.extend_from_slice(&[0u8; 12]);

        let attr_value = username.as_bytes();
        buf.extend_from_slice(&STUN_ATTR_USERNAME.to_be_bytes());
        buf.extend_from_slice(&(attr_value.len() as u16).to_be_bytes());
        buf.extend_from_slice(attr_value);
        let pad = (4 - (attr_value.len() & 3)) & 3;
        buf.extend(std::iter::repeat_n(0u8, pad));

        // Backfill the message length: bytes after the 20-byte header.
        let body_len = (buf.len() - 20) as u16;
        buf[2..4].copy_from_slice(&body_len.to_be_bytes());
        buf
    }

    #[test]
    fn extract_local_ufrag_from_canonical_username() {
        // RFC 8445 §7.2.2: USERNAME = <local-ufrag>:<remote-ufrag>
        // where "local" is the receiver's ICE ufrag.
        let pkt = make_binding_request("LOCAL123:REMOTE");
        assert_eq!(extract_local_ufrag(&pkt).as_deref(), Some("LOCAL123"));
    }

    #[test]
    fn returns_none_for_short_packet() {
        assert!(extract_local_ufrag(&[0u8; 10]).is_none());
    }

    #[test]
    fn returns_none_for_dtls_record() {
        // DTLS records start with the content type byte (0x14..=0x17).
        let mut buf = vec![0x16u8; 32]; // handshake
        buf[0] = 0x16;
        assert!(extract_local_ufrag(&buf).is_none());
    }

    #[test]
    fn returns_none_for_rtp_packet() {
        // RTP version 2 starts with 0x80.
        let mut buf = vec![0u8; 32];
        buf[0] = 0x80;
        assert!(extract_local_ufrag(&buf).is_none());
    }

    #[test]
    fn returns_none_when_username_missing() {
        // Build an empty STUN binding request (no attributes).
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x0001u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&[0u8; 12]);
        assert!(extract_local_ufrag(&buf).is_none());
    }

    #[test]
    fn returns_none_when_username_lacks_separator() {
        let pkt = make_binding_request("nosep");
        assert!(extract_local_ufrag(&pkt).is_none());
    }

    #[test]
    fn returns_none_for_wrong_magic_cookie() {
        let mut pkt = make_binding_request("LOCAL:REMOTE");
        pkt[4] = 0xAA; // mangle the magic cookie
        assert!(extract_local_ufrag(&pkt).is_none());
    }

    #[test]
    fn returns_none_for_binding_response() {
        // Binding Response (success) = 0x0101.
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x0101u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&[0u8; 12]);
        assert!(extract_local_ufrag(&buf).is_none());
    }
}
