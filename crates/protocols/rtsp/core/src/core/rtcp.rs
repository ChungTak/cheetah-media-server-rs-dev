/// `RTCP_PT_SR` constant.
/// `RTCP_PT_SR` 常量。
pub const RTCP_PT_SR: u8 = 200;
/// `RTCP_PT_RR` constant.
/// `RTCP_PT_RR` 常量。
pub const RTCP_PT_RR: u8 = 201;
/// `RTCP_PT_SDES` constant.
/// `RTCP_PT_SDES` 常量。
pub const RTCP_PT_SDES: u8 = 202;
/// `RTCP_PT_BYE` constant.
/// `RTCP_PT_BYE` 常量。
pub const RTCP_PT_BYE: u8 = 203;
/// `RTCP_PT_APP` constant.
/// `RTCP_PT_APP` 常量。
pub const RTCP_PT_APP: u8 = 204;

/// `SDES_CNAME` constant.
/// `SDES_CNAME` 常量。
pub const SDES_CNAME: u8 = 1;
/// `SDES_NAME` constant.
/// `SDES_NAME` 常量。
pub const SDES_NAME: u8 = 2;
/// `SDES_EMAIL` constant.
/// `SDES_EMAIL` 常量。
pub const SDES_EMAIL: u8 = 3;
/// `SDES_PHONE` constant.
/// `SDES_PHONE` 常量。
pub const SDES_PHONE: u8 = 4;
/// `SDES_LOC` constant.
/// `SDES_LOC` 常量。
pub const SDES_LOC: u8 = 5;
/// `SDES_TOOL` constant.
/// `SDES_TOOL` 常量。
pub const SDES_TOOL: u8 = 6;
/// `SDES_NOTE` constant.
/// `SDES_NOTE` 常量。
pub const SDES_NOTE: u8 = 7;
/// `SDES_PRIV` constant.
/// `SDES_PRIV` 常量。
pub const SDES_PRIV: u8 = 8;

const RTCP_COMMON_HEADER_SIZE: usize = 4;
const RTCP_SR_BASE_PAYLOAD_SIZE: usize = 24;
const RTCP_RR_BASE_PAYLOAD_SIZE: usize = 4;
const RTCP_REPORT_BLOCK_SIZE: usize = 24;
const RTCP_MAX_REPORT_COUNT: usize = 31;
const RTCP_CUMULATIVE_LOST_MAX: u32 = 0x00FF_FFFF;

/// Error returned by `RTCP` operations.
/// `RTCP` 操作返回的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtcpError {
    #[error("unsupported rtcp version: {actual}")]
    UnsupportedVersion { actual: u8 },
    #[error("insufficient data for {context}: need at least {needed} bytes, got {actual}")]
    InsufficientData {
        context: &'static str,
        needed: usize,
        actual: usize,
    },
    #[error(
        "invalid rtcp padding size: {padding_size}, available payload+padding bytes: {available}"
    )]
    InvalidPadding { padding_size: u8, available: usize },
    #[error("rtcp sender report count exceeds 5-bit field: {count}")]
    TooManyReportBlocks { count: usize },
    #[error("rtcp sdes chunk count exceeds 5-bit field: {count}")]
    TooManySdesChunks { count: usize },
    #[error("rtcp bye source count exceeds 5-bit field: {count}")]
    TooManyByeSources { count: usize },
    #[error("rtcp app subtype exceeds 5-bit field: {subtype}")]
    InvalidAppSubtype { subtype: u8 },
    #[error("rtcp cumulative lost exceeds 24-bit field: {value}")]
    CumulativeLostOutOfRange { value: u32 },
    #[error("rtcp sdes item too large for 8-bit length field: type={item_type}, len={len}")]
    SdesItemDataTooLong { item_type: u8, len: usize },
    #[error("invalid rtcp sdes PRIV item: len={len}, prefix_len={prefix_len}")]
    InvalidSdesPrivItem { len: usize, prefix_len: usize },
    #[error("rtcp bye reason too large for 8-bit length field: len={len}")]
    ByeReasonTooLong { len: usize },
    #[error("rtcp sdes chunk missing end marker: chunk_index={chunk_index}")]
    SdesChunkMissingTerminator { chunk_index: usize },
    #[error("rtcp packet too large to encode length field: {words} words")]
    PacketTooLarge { words: usize },
    #[error("rtcp packet payload must align to 32-bit words, got {bytes} bytes")]
    PayloadNotWordAligned { bytes: usize },
}

/// RTCP Sender Report 中的报告块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpReportBlock {
    pub ssrc: u32,
    pub fraction_lost: u8,
    pub cumulative_lost: u32,
    pub highest_seq: u32,
    pub jitter: u32,
    pub last_sr: u32,
    pub delay_since_sr: u32,
}

/// RTCP Sender Report（PT=200，RFC 3550）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSenderReport {
    pub ssrc: u32,
    pub ntp_timestamp: u64,
    pub rtp_timestamp: u32,
    pub packet_count: u32,
    pub octet_count: u32,
    pub reports: Vec<RtcpReportBlock>,
}

/// RTCP Receiver Report（PT=201，RFC 3550）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpReceiverReport {
    pub ssrc: u32,
    pub reports: Vec<RtcpReportBlock>,
}

/// RTCP Source Description（PT=202，RFC 3550）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSdes {
    pub chunks: Vec<RtcpSdesChunk>,
}

/// RTCP BYE（PT=203，RFC 3550）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpBye {
    pub ssrcs: Vec<u32>,
    pub reason: Option<String>,
}

/// RTCP APP（PT=204，RFC 3550）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpApp {
    pub subtype: u8,
    pub ssrc: u32,
    pub name: [u8; 4],
    pub data: Vec<u8>,
}

/// RTCP SDES Chunk。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtcpSdesChunk {
    pub ssrc: u32,
    pub items: Vec<RtcpSdesItem>,
}

/// RTCP SDES Item。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcpSdesItem {
    Cname(String),
    Name(String),
    Email(String),
    Phone(String),
    Loc(String),
    Tool(String),
    Note(String),
    Priv { prefix: String, value: Vec<u8> },
    Unknown { item_type: u8, data: Vec<u8> },
}

impl RtcpSdesItem {
    /// `item_type` function of `RtcpSdesItem`.
    /// `RtcpSdesItem` 的 `item_type` 函数。
    pub fn item_type(&self) -> u8 {
        match self {
            RtcpSdesItem::Cname(_) => SDES_CNAME,
            RtcpSdesItem::Name(_) => SDES_NAME,
            RtcpSdesItem::Email(_) => SDES_EMAIL,
            RtcpSdesItem::Phone(_) => SDES_PHONE,
            RtcpSdesItem::Loc(_) => SDES_LOC,
            RtcpSdesItem::Tool(_) => SDES_TOOL,
            RtcpSdesItem::Note(_) => SDES_NOTE,
            RtcpSdesItem::Priv { .. } => SDES_PRIV,
            RtcpSdesItem::Unknown { item_type, .. } => *item_type,
        }
    }
}

/// RTCP 包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtcpPacket {
    SenderReport(RtcpSenderReport),
    ReceiverReport(RtcpReceiverReport),
    SourceDescription(RtcpSdes),
    Bye(RtcpBye),
    App(RtcpApp),
    Unknown {
        payload_type: u8,
        count: u8,
        payload: Vec<u8>,
    },
}

impl RtcpPacket {
    /// 解析 compound RTCP 包。
    pub fn parse(data: &[u8]) -> Result<Vec<Self>, RtcpError> {
        let mut packets = Vec::new();
        let mut offset = 0;

        while data.len().saturating_sub(offset) >= RTCP_COMMON_HEADER_SIZE {
            let (packet, consumed) = Self::parse_one(&data[offset..])?;
            packets.push(packet);
            offset += consumed;
        }

        Ok(packets)
    }

    /// 构建 compound RTCP 包。
    pub fn build(packets: &[Self]) -> Result<Vec<u8>, RtcpError> {
        let mut out = Vec::new();
        for packet in packets {
            packet.build_one(&mut out)?;
        }
        Ok(out)
    }

    fn parse_one(data: &[u8]) -> Result<(Self, usize), RtcpError> {
        debug_assert!(data.len() >= RTCP_COMMON_HEADER_SIZE);

        let first = data[0];
        let version = first >> 6;
        if version != 2 {
            return Err(RtcpError::UnsupportedVersion { actual: version });
        }
        let has_padding = first & 0b0010_0000 != 0;
        let count = first & 0b0001_1111;
        let payload_type = data[1];
        let length_words = u16::from_be_bytes([data[2], data[3]]) as usize;
        let payload_size = length_words * 4;
        let packet_size = RTCP_COMMON_HEADER_SIZE + payload_size;
        if data.len() < packet_size {
            return Err(RtcpError::InsufficientData {
                context: "rtcp packet payload",
                needed: packet_size,
                actual: data.len(),
            });
        }

        let mut payload = &data[RTCP_COMMON_HEADER_SIZE..packet_size];
        if has_padding {
            if payload.is_empty() {
                return Err(RtcpError::InvalidPadding {
                    padding_size: 0,
                    available: 0,
                });
            }
            let padding_size = *payload.last().unwrap();
            if padding_size == 0 || padding_size as usize > payload.len() {
                return Err(RtcpError::InvalidPadding {
                    padding_size,
                    available: payload.len(),
                });
            }
            payload = &payload[..payload.len() - padding_size as usize];
        }

        let packet = match payload_type {
            RTCP_PT_SR => RtcpPacket::SenderReport(parse_sender_report(payload, count)?),
            RTCP_PT_RR => RtcpPacket::ReceiverReport(parse_receiver_report(payload, count)?),
            RTCP_PT_SDES => {
                RtcpPacket::SourceDescription(parse_source_description(payload, count)?)
            }
            RTCP_PT_BYE => RtcpPacket::Bye(parse_bye(payload, count)?),
            RTCP_PT_APP => RtcpPacket::App(parse_app(payload, count)?),
            _ => RtcpPacket::Unknown {
                payload_type,
                count,
                payload: payload.to_vec(),
            },
        };

        Ok((packet, packet_size))
    }

    fn build_one(&self, out: &mut Vec<u8>) -> Result<(), RtcpError> {
        match self {
            RtcpPacket::SenderReport(sr) => build_sender_report(out, sr),
            RtcpPacket::ReceiverReport(rr) => build_receiver_report(out, rr),
            RtcpPacket::SourceDescription(sdes) => build_source_description(out, sdes),
            RtcpPacket::Bye(bye) => build_bye(out, bye),
            RtcpPacket::App(app) => build_app(out, app),
            RtcpPacket::Unknown {
                payload_type,
                count,
                payload,
            } => {
                write_packet_header(out, *payload_type, *count, payload.len())?;
                out.extend_from_slice(payload);
                Ok(())
            }
        }
    }
}

fn parse_sender_report(payload: &[u8], report_count: u8) -> Result<RtcpSenderReport, RtcpError> {
    let report_count = report_count as usize;
    let reports = parse_report_blocks(payload, RTCP_SR_BASE_PAYLOAD_SIZE, report_count)?;

    let ssrc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let ntp_timestamp = u64::from_be_bytes([
        payload[4],
        payload[5],
        payload[6],
        payload[7],
        payload[8],
        payload[9],
        payload[10],
        payload[11],
    ]);
    let rtp_timestamp = u32::from_be_bytes([payload[12], payload[13], payload[14], payload[15]]);
    let packet_count = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
    let octet_count = u32::from_be_bytes([payload[20], payload[21], payload[22], payload[23]]);

    Ok(RtcpSenderReport {
        ssrc,
        ntp_timestamp,
        rtp_timestamp,
        packet_count,
        octet_count,
        reports,
    })
}

fn parse_receiver_report(
    payload: &[u8],
    report_count: u8,
) -> Result<RtcpReceiverReport, RtcpError> {
    let report_count = report_count as usize;
    let reports = parse_report_blocks(payload, RTCP_RR_BASE_PAYLOAD_SIZE, report_count)?;
    let ssrc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    Ok(RtcpReceiverReport { ssrc, reports })
}

fn parse_source_description(payload: &[u8], chunk_count: u8) -> Result<RtcpSdes, RtcpError> {
    let chunk_count = chunk_count as usize;
    let mut chunks = Vec::with_capacity(chunk_count);
    let mut offset = 0;

    for chunk_index in 0..chunk_count {
        if payload.len().saturating_sub(offset) < 4 {
            return Err(RtcpError::InsufficientData {
                context: "rtcp sdes chunk ssrc",
                needed: offset + 4,
                actual: payload.len(),
            });
        }
        let chunk_start = offset;
        let ssrc = u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;

        let mut items = Vec::new();
        let mut found_terminator = false;
        while offset < payload.len() {
            let item_type = payload[offset];
            offset += 1;
            if item_type == 0 {
                found_terminator = true;
                let bytes_read = offset - chunk_start;
                let padding = (4 - (bytes_read % 4)) % 4;
                if payload.len().saturating_sub(offset) < padding {
                    return Err(RtcpError::InsufficientData {
                        context: "rtcp sdes chunk padding",
                        needed: offset + padding,
                        actual: payload.len(),
                    });
                }
                offset += padding;
                break;
            }

            if offset >= payload.len() {
                return Err(RtcpError::InsufficientData {
                    context: "rtcp sdes item length",
                    needed: offset + 1,
                    actual: payload.len(),
                });
            }

            let item_len = payload[offset] as usize;
            offset += 1;
            if payload.len().saturating_sub(offset) < item_len {
                return Err(RtcpError::InsufficientData {
                    context: "rtcp sdes item data",
                    needed: offset + item_len,
                    actual: payload.len(),
                });
            }

            let item_data = &payload[offset..offset + item_len];
            offset += item_len;
            items.push(parse_sdes_item(item_type, item_data)?);
        }

        if !found_terminator {
            return Err(RtcpError::SdesChunkMissingTerminator { chunk_index });
        }

        chunks.push(RtcpSdesChunk { ssrc, items });
    }

    Ok(RtcpSdes { chunks })
}

fn parse_bye(payload: &[u8], source_count: u8) -> Result<RtcpBye, RtcpError> {
    let source_count = source_count as usize;
    let required_ssrc_bytes = source_count * 4;
    if payload.len() < required_ssrc_bytes {
        return Err(RtcpError::InsufficientData {
            context: "rtcp bye ssrc list",
            needed: required_ssrc_bytes,
            actual: payload.len(),
        });
    }

    let mut ssrcs = Vec::with_capacity(source_count);
    let mut offset = 0;
    for _ in 0..source_count {
        ssrcs.push(u32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]));
        offset += 4;
    }

    let remaining = &payload[offset..];
    let reason = if remaining.is_empty() {
        None
    } else {
        let reason_len = remaining[0] as usize;
        if remaining.len() < reason_len + 1 {
            return Err(RtcpError::InsufficientData {
                context: "rtcp bye reason",
                needed: required_ssrc_bytes + reason_len + 1,
                actual: payload.len(),
            });
        }
        Some(String::from_utf8_lossy(&remaining[1..1 + reason_len]).into_owned())
    };

    Ok(RtcpBye { ssrcs, reason })
}

fn parse_app(payload: &[u8], subtype: u8) -> Result<RtcpApp, RtcpError> {
    if payload.len() < 8 {
        return Err(RtcpError::InsufficientData {
            context: "rtcp app packet",
            needed: 8,
            actual: payload.len(),
        });
    }

    let ssrc = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let name = [payload[4], payload[5], payload[6], payload[7]];
    let data = payload[8..].to_vec();

    Ok(RtcpApp {
        subtype,
        ssrc,
        name,
        data,
    })
}

fn parse_sdes_item(item_type: u8, item_data: &[u8]) -> Result<RtcpSdesItem, RtcpError> {
    let item = match item_type {
        SDES_CNAME => RtcpSdesItem::Cname(String::from_utf8_lossy(item_data).into_owned()),
        SDES_NAME => RtcpSdesItem::Name(String::from_utf8_lossy(item_data).into_owned()),
        SDES_EMAIL => RtcpSdesItem::Email(String::from_utf8_lossy(item_data).into_owned()),
        SDES_PHONE => RtcpSdesItem::Phone(String::from_utf8_lossy(item_data).into_owned()),
        SDES_LOC => RtcpSdesItem::Loc(String::from_utf8_lossy(item_data).into_owned()),
        SDES_TOOL => RtcpSdesItem::Tool(String::from_utf8_lossy(item_data).into_owned()),
        SDES_NOTE => RtcpSdesItem::Note(String::from_utf8_lossy(item_data).into_owned()),
        SDES_PRIV => {
            if item_data.is_empty() {
                return Err(RtcpError::InvalidSdesPrivItem {
                    len: 0,
                    prefix_len: 0,
                });
            }
            let prefix_len = item_data[0] as usize;
            if item_data.len() < 1 + prefix_len {
                return Err(RtcpError::InvalidSdesPrivItem {
                    len: item_data.len(),
                    prefix_len,
                });
            }
            let prefix = String::from_utf8_lossy(&item_data[1..1 + prefix_len]).into_owned();
            let value = item_data[1 + prefix_len..].to_vec();
            RtcpSdesItem::Priv { prefix, value }
        }
        _ => RtcpSdesItem::Unknown {
            item_type,
            data: item_data.to_vec(),
        },
    };
    Ok(item)
}

fn parse_report_blocks(
    payload: &[u8],
    report_start_offset: usize,
    report_count: usize,
) -> Result<Vec<RtcpReportBlock>, RtcpError> {
    let expected = report_start_offset + report_count * RTCP_REPORT_BLOCK_SIZE;
    if payload.len() < expected {
        return Err(RtcpError::InsufficientData {
            context: "rtcp report blocks",
            needed: expected,
            actual: payload.len(),
        });
    }

    let mut reports = Vec::with_capacity(report_count);
    let mut offset = report_start_offset;
    for _ in 0..report_count {
        reports.push(parse_report_block(
            &payload[offset..offset + RTCP_REPORT_BLOCK_SIZE],
        ));
        offset += RTCP_REPORT_BLOCK_SIZE;
    }
    Ok(reports)
}

fn parse_report_block(data: &[u8]) -> RtcpReportBlock {
    let ssrc = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let fraction_and_lost = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let highest_seq = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let jitter = u32::from_be_bytes([data[12], data[13], data[14], data[15]]);
    let last_sr = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let delay_since_sr = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    RtcpReportBlock {
        ssrc,
        fraction_lost: (fraction_and_lost >> 24) as u8,
        cumulative_lost: fraction_and_lost & RTCP_CUMULATIVE_LOST_MAX,
        highest_seq,
        jitter,
        last_sr,
        delay_since_sr,
    }
}

fn build_sender_report(out: &mut Vec<u8>, sr: &RtcpSenderReport) -> Result<(), RtcpError> {
    let report_count = sr.reports.len();
    if report_count > RTCP_MAX_REPORT_COUNT {
        return Err(RtcpError::TooManyReportBlocks {
            count: report_count,
        });
    }

    let length_words = 6 + report_count * 6;
    if length_words > u16::MAX as usize {
        return Err(RtcpError::PacketTooLarge {
            words: length_words,
        });
    }

    write_packet_header(out, RTCP_PT_SR, report_count as u8, length_words * 4)?;

    out.extend_from_slice(&sr.ssrc.to_be_bytes());
    out.extend_from_slice(&sr.ntp_timestamp.to_be_bytes());
    out.extend_from_slice(&sr.rtp_timestamp.to_be_bytes());
    out.extend_from_slice(&sr.packet_count.to_be_bytes());
    out.extend_from_slice(&sr.octet_count.to_be_bytes());
    write_report_blocks(out, &sr.reports)
}

fn build_receiver_report(out: &mut Vec<u8>, rr: &RtcpReceiverReport) -> Result<(), RtcpError> {
    let report_count = rr.reports.len();
    if report_count > RTCP_MAX_REPORT_COUNT {
        return Err(RtcpError::TooManyReportBlocks {
            count: report_count,
        });
    }

    let length_words = 1 + report_count * 6;
    if length_words > u16::MAX as usize {
        return Err(RtcpError::PacketTooLarge {
            words: length_words,
        });
    }

    write_packet_header(out, RTCP_PT_RR, report_count as u8, length_words * 4)?;
    out.extend_from_slice(&rr.ssrc.to_be_bytes());
    write_report_blocks(out, &rr.reports)
}

fn build_source_description(out: &mut Vec<u8>, sdes: &RtcpSdes) -> Result<(), RtcpError> {
    let chunk_count = sdes.chunks.len();
    if chunk_count > RTCP_MAX_REPORT_COUNT {
        return Err(RtcpError::TooManySdesChunks { count: chunk_count });
    }

    let mut payload = Vec::new();
    for chunk in &sdes.chunks {
        payload.extend_from_slice(&chunk.ssrc.to_be_bytes());
        for item in &chunk.items {
            let item_type = item.item_type();
            let item_data = build_sdes_item_data(item)?;
            if item_data.len() > u8::MAX as usize {
                return Err(RtcpError::SdesItemDataTooLong {
                    item_type,
                    len: item_data.len(),
                });
            }

            payload.push(item_type);
            payload.push(item_data.len() as u8);
            payload.extend_from_slice(&item_data);
        }

        payload.push(0);
        while payload.len() % 4 != 0 {
            payload.push(0);
        }
    }

    write_packet_header(out, RTCP_PT_SDES, chunk_count as u8, payload.len())?;
    out.extend_from_slice(&payload);
    Ok(())
}

fn build_bye(out: &mut Vec<u8>, bye: &RtcpBye) -> Result<(), RtcpError> {
    let source_count = bye.ssrcs.len();
    if source_count > RTCP_MAX_REPORT_COUNT {
        return Err(RtcpError::TooManyByeSources {
            count: source_count,
        });
    }

    let mut payload = Vec::new();
    for ssrc in &bye.ssrcs {
        payload.extend_from_slice(&ssrc.to_be_bytes());
    }

    if let Some(reason) = &bye.reason {
        let reason_bytes = reason.as_bytes();
        if reason_bytes.len() > u8::MAX as usize {
            return Err(RtcpError::ByeReasonTooLong {
                len: reason_bytes.len(),
            });
        }
        payload.push(reason_bytes.len() as u8);
        payload.extend_from_slice(reason_bytes);
        while !payload.len().is_multiple_of(4) {
            payload.push(0);
        }
    }

    write_packet_header(out, RTCP_PT_BYE, source_count as u8, payload.len())?;
    out.extend_from_slice(&payload);
    Ok(())
}

fn build_app(out: &mut Vec<u8>, app: &RtcpApp) -> Result<(), RtcpError> {
    if app.subtype as usize > RTCP_MAX_REPORT_COUNT {
        return Err(RtcpError::InvalidAppSubtype {
            subtype: app.subtype,
        });
    }

    let mut payload = Vec::with_capacity(8 + app.data.len() + 3);
    payload.extend_from_slice(&app.ssrc.to_be_bytes());
    payload.extend_from_slice(&app.name);
    payload.extend_from_slice(&app.data);
    while !payload.len().is_multiple_of(4) {
        payload.push(0);
    }

    write_packet_header(out, RTCP_PT_APP, app.subtype, payload.len())?;
    out.extend_from_slice(&payload);
    Ok(())
}

fn build_sdes_item_data(item: &RtcpSdesItem) -> Result<Vec<u8>, RtcpError> {
    let data = match item {
        RtcpSdesItem::Cname(s)
        | RtcpSdesItem::Name(s)
        | RtcpSdesItem::Email(s)
        | RtcpSdesItem::Phone(s)
        | RtcpSdesItem::Loc(s)
        | RtcpSdesItem::Tool(s)
        | RtcpSdesItem::Note(s) => s.as_bytes().to_vec(),
        RtcpSdesItem::Priv { prefix, value } => {
            let prefix_bytes = prefix.as_bytes();
            let len = 1 + prefix_bytes.len() + value.len();
            if len > u8::MAX as usize {
                return Err(RtcpError::SdesItemDataTooLong {
                    item_type: SDES_PRIV,
                    len,
                });
            }
            let mut data = Vec::with_capacity(len);
            data.push(prefix_bytes.len() as u8);
            data.extend_from_slice(prefix_bytes);
            data.extend_from_slice(value);
            data
        }
        RtcpSdesItem::Unknown { data, .. } => data.clone(),
    };
    Ok(data)
}

fn write_report_blocks(out: &mut Vec<u8>, reports: &[RtcpReportBlock]) -> Result<(), RtcpError> {
    for report in reports {
        if report.cumulative_lost > RTCP_CUMULATIVE_LOST_MAX {
            return Err(RtcpError::CumulativeLostOutOfRange {
                value: report.cumulative_lost,
            });
        }
        write_report_block(out, report);
    }
    Ok(())
}

fn write_report_block(out: &mut Vec<u8>, report: &RtcpReportBlock) {
    out.extend_from_slice(&report.ssrc.to_be_bytes());
    let fraction_and_lost =
        ((report.fraction_lost as u32) << 24) | (report.cumulative_lost & 0x00FF_FFFF);
    out.extend_from_slice(&fraction_and_lost.to_be_bytes());
    out.extend_from_slice(&report.highest_seq.to_be_bytes());
    out.extend_from_slice(&report.jitter.to_be_bytes());
    out.extend_from_slice(&report.last_sr.to_be_bytes());
    out.extend_from_slice(&report.delay_since_sr.to_be_bytes());
}

fn write_packet_header(
    out: &mut Vec<u8>,
    payload_type: u8,
    count: u8,
    payload_bytes: usize,
) -> Result<(), RtcpError> {
    if !payload_bytes.is_multiple_of(4) {
        return Err(RtcpError::PayloadNotWordAligned {
            bytes: payload_bytes,
        });
    }
    let length_words = payload_bytes / 4;
    if length_words > u16::MAX as usize {
        return Err(RtcpError::PacketTooLarge {
            words: length_words,
        });
    }

    let first = (2 << 6) | (count & 0b0001_1111);
    out.push(first);
    out.push(payload_type);
    out.extend_from_slice(&(length_words as u16).to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        RtcpApp, RtcpBye, RtcpError, RtcpPacket, RtcpReceiverReport, RtcpReportBlock, RtcpSdes,
        RtcpSdesChunk, RtcpSdesItem, RtcpSenderReport, RTCP_COMMON_HEADER_SIZE, RTCP_PT_APP,
        RTCP_PT_BYE, RTCP_PT_RR, RTCP_PT_SDES, RTCP_PT_SR,
    };

    #[test]
    fn rtcp_sender_report_roundtrip_basic() {
        let sr = RtcpSenderReport {
            ssrc: 0x1122_3344,
            ntp_timestamp: 0x0102_0304_0506_0708,
            rtp_timestamp: 90_000,
            packet_count: 512,
            octet_count: 4096,
            reports: vec![RtcpReportBlock {
                ssrc: 0x5566_7788,
                fraction_lost: 10,
                cumulative_lost: 0x00AB_CDEF,
                highest_seq: 0x0102_0304,
                jitter: 0x1112_1314,
                last_sr: 0x2122_2324,
                delay_since_sr: 0x3132_3334,
            }],
        };

        let encoded = RtcpPacket::build(&[RtcpPacket::SenderReport(sr.clone())])
            .expect("build sender report");
        let decoded = RtcpPacket::parse(&encoded).expect("parse sender report");

        assert_eq!(decoded, vec![RtcpPacket::SenderReport(sr)]);
    }

    #[test]
    fn rtcp_receiver_report_roundtrip_basic() {
        let rr = RtcpReceiverReport {
            ssrc: 0x1122_3344,
            reports: vec![RtcpReportBlock {
                ssrc: 0x5566_7788,
                fraction_lost: 10,
                cumulative_lost: 0x00AB_CDEF,
                highest_seq: 0x0102_0304,
                jitter: 0x1112_1314,
                last_sr: 0x2122_2324,
                delay_since_sr: 0x3132_3334,
            }],
        };

        let encoded = RtcpPacket::build(&[RtcpPacket::ReceiverReport(rr.clone())])
            .expect("build receiver report");
        let decoded = RtcpPacket::parse(&encoded).expect("parse receiver report");

        assert_eq!(decoded, vec![RtcpPacket::ReceiverReport(rr)]);
    }

    #[test]
    fn rtcp_source_description_roundtrip_basic() {
        let sdes = RtcpSdes {
            chunks: vec![RtcpSdesChunk {
                ssrc: 0xDEAD_BEEF,
                items: vec![
                    RtcpSdesItem::Cname("test@example.com".to_string()),
                    RtcpSdesItem::Tool("cheetah".to_string()),
                    RtcpSdesItem::Priv {
                        prefix: "meta".to_string(),
                        value: b"\x01\x02\x03".to_vec(),
                    },
                ],
            }],
        };

        let encoded = RtcpPacket::build(&[RtcpPacket::SourceDescription(sdes.clone())])
            .expect("build source description");
        let decoded = RtcpPacket::parse(&encoded).expect("parse source description");

        assert_eq!(decoded, vec![RtcpPacket::SourceDescription(sdes)]);
    }

    #[test]
    fn rtcp_bye_roundtrip_basic() {
        let bye = RtcpBye {
            ssrcs: vec![0x1122_3344, 0x5566_7788],
            reason: Some("teardown".to_string()),
        };

        let encoded = RtcpPacket::build(&[RtcpPacket::Bye(bye.clone())]).expect("build bye");
        let decoded = RtcpPacket::parse(&encoded).expect("parse bye");

        assert_eq!(decoded, vec![RtcpPacket::Bye(bye)]);
    }

    #[test]
    fn rtcp_app_roundtrip_basic() {
        let app = RtcpApp {
            subtype: 17,
            ssrc: 0x1122_3344,
            name: *b"CHEE",
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };

        let encoded = RtcpPacket::build(&[RtcpPacket::App(app.clone())]).expect("build app");
        let decoded = RtcpPacket::parse(&encoded).expect("parse app");

        assert_eq!(decoded, vec![RtcpPacket::App(app)]);
    }

    #[test]
    fn rtcp_parse_rejects_invalid_version() {
        let raw = [0b0100_0000, RTCP_PT_SR, 0, 0];
        let err = RtcpPacket::parse(&raw).expect_err("unsupported version must fail");
        assert!(matches!(err, RtcpError::UnsupportedVersion { actual: 1 }));
    }

    #[test]
    fn rtcp_parse_rejects_truncated_sender_report() {
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + 24];
        raw[0] = 0b1000_0001; // version=2, report count=1
        raw[1] = RTCP_PT_SR;
        raw[2..4].copy_from_slice(&6_u16.to_be_bytes()); // payload bytes=24

        let err = RtcpPacket::parse(&raw).expect_err("truncated report block must fail");
        assert!(matches!(
            err,
            RtcpError::InsufficientData {
                context: "rtcp report blocks",
                needed: 48,
                actual: 24
            }
        ));
    }

    #[test]
    fn rtcp_parse_rejects_truncated_receiver_report() {
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + 4];
        raw[0] = 0b1000_0001; // version=2, report count=1
        raw[1] = RTCP_PT_RR;
        raw[2..4].copy_from_slice(&1_u16.to_be_bytes()); // payload bytes=4

        let err = RtcpPacket::parse(&raw).expect_err("truncated receiver report must fail");
        assert!(matches!(
            err,
            RtcpError::InsufficientData {
                context: "rtcp report blocks",
                needed: 28,
                actual: 4
            }
        ));
    }

    #[test]
    fn rtcp_parse_rejects_sdes_chunk_without_terminator() {
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + 4];
        raw[0] = 0b1000_0001; // version=2, chunk count=1
        raw[1] = RTCP_PT_SDES;
        raw[2..4].copy_from_slice(&1_u16.to_be_bytes()); // payload bytes=4
        raw[4..8].copy_from_slice(&0x1122_3344_u32.to_be_bytes());

        let err = RtcpPacket::parse(&raw).expect_err("missing SDES terminator must fail");
        assert!(matches!(
            err,
            RtcpError::SdesChunkMissingTerminator { chunk_index: 0 }
        ));
    }

    #[test]
    fn rtcp_parse_rejects_invalid_sdes_priv_item() {
        let payload = [
            0x11, 0x22, 0x33, 0x44, // ssrc
            0x08, 0x01, 0x02, 0x00, // PRIV item(len=1,prefix_len=2) + terminator
        ];
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + payload.len()];
        raw[0] = 0b1000_0001; // version=2, chunk count=1
        raw[1] = RTCP_PT_SDES;
        raw[2..4].copy_from_slice(&(payload.len() as u16 / 4).to_be_bytes());
        raw[4..].copy_from_slice(&payload);

        let err = RtcpPacket::parse(&raw).expect_err("invalid SDES PRIV item must fail");
        assert!(matches!(
            err,
            RtcpError::InvalidSdesPrivItem {
                len: 1,
                prefix_len: 2
            }
        ));
    }

    #[test]
    fn rtcp_parse_rejects_truncated_bye_ssrc_list() {
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + 4];
        raw[0] = 0b1000_0010; // version=2, source count=2
        raw[1] = RTCP_PT_BYE;
        raw[2..4].copy_from_slice(&1_u16.to_be_bytes()); // payload bytes=4
        raw[4..8].copy_from_slice(&0x1122_3344_u32.to_be_bytes());

        let err = RtcpPacket::parse(&raw).expect_err("truncated BYE ssrc list must fail");
        assert!(matches!(
            err,
            RtcpError::InsufficientData {
                context: "rtcp bye ssrc list",
                needed: 8,
                actual: 4
            }
        ));
    }

    #[test]
    fn rtcp_parse_rejects_truncated_bye_reason() {
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + 8];
        raw[0] = 0b1000_0001; // version=2, source count=1
        raw[1] = RTCP_PT_BYE;
        raw[2..4].copy_from_slice(&2_u16.to_be_bytes()); // payload bytes=8
        raw[4..8].copy_from_slice(&0x1122_3344_u32.to_be_bytes());
        raw[8] = 5; // reason length
        raw[9..12].copy_from_slice(b"abc");

        let err = RtcpPacket::parse(&raw).expect_err("truncated BYE reason must fail");
        assert!(matches!(
            err,
            RtcpError::InsufficientData {
                context: "rtcp bye reason",
                needed: 10,
                actual: 8
            }
        ));
    }

    #[test]
    fn rtcp_parse_rejects_truncated_app_packet() {
        let mut raw = vec![0_u8; RTCP_COMMON_HEADER_SIZE + 4];
        raw[0] = 0b1001_1111; // version=2, subtype=31
        raw[1] = RTCP_PT_APP;
        raw[2..4].copy_from_slice(&1_u16.to_be_bytes()); // payload bytes=4
        raw[4..8].copy_from_slice(&0x1122_3344_u32.to_be_bytes()); // only ssrc, missing name

        let err = RtcpPacket::parse(&raw).expect_err("truncated APP packet must fail");
        assert!(matches!(
            err,
            RtcpError::InsufficientData {
                context: "rtcp app packet",
                needed: 8,
                actual: 4
            }
        ));
    }

    #[test]
    fn rtcp_build_rejects_too_many_report_blocks() {
        let report = RtcpReportBlock {
            ssrc: 0,
            fraction_lost: 0,
            cumulative_lost: 0,
            highest_seq: 0,
            jitter: 0,
            last_sr: 0,
            delay_since_sr: 0,
        };
        let sr = RtcpSenderReport {
            ssrc: 1,
            ntp_timestamp: 2,
            rtp_timestamp: 3,
            packet_count: 4,
            octet_count: 5,
            reports: vec![report; 32],
        };

        let err = RtcpPacket::build(&[RtcpPacket::SenderReport(sr)])
            .expect_err("report count overflow must fail");
        assert!(matches!(err, RtcpError::TooManyReportBlocks { count: 32 }));
    }

    #[test]
    fn rtcp_build_rejects_cumulative_lost_overflow() {
        let sr = RtcpSenderReport {
            ssrc: 1,
            ntp_timestamp: 2,
            rtp_timestamp: 3,
            packet_count: 4,
            octet_count: 5,
            reports: vec![RtcpReportBlock {
                ssrc: 6,
                fraction_lost: 7,
                cumulative_lost: 0x01_00_00_00,
                highest_seq: 8,
                jitter: 9,
                last_sr: 10,
                delay_since_sr: 11,
            }],
        };

        let err = RtcpPacket::build(&[RtcpPacket::SenderReport(sr)])
            .expect_err("cumulative_lost overflow must fail");
        assert!(matches!(
            err,
            RtcpError::CumulativeLostOutOfRange {
                value: 0x01_00_00_00
            }
        ));
    }

    #[test]
    fn rtcp_build_rejects_too_many_sdes_chunks() {
        let chunk = RtcpSdesChunk {
            ssrc: 0x1122_3344,
            items: vec![RtcpSdesItem::Cname("cheetah".to_string())],
        };
        let sdes = RtcpSdes {
            chunks: vec![chunk; 32],
        };

        let err = RtcpPacket::build(&[RtcpPacket::SourceDescription(sdes)])
            .expect_err("sdes chunk count overflow must fail");
        assert!(matches!(err, RtcpError::TooManySdesChunks { count: 32 }));
    }

    #[test]
    fn rtcp_build_rejects_sdes_item_length_overflow() {
        let sdes = RtcpSdes {
            chunks: vec![RtcpSdesChunk {
                ssrc: 0x1122_3344,
                items: vec![RtcpSdesItem::Cname("x".repeat(256))],
            }],
        };

        let err = RtcpPacket::build(&[RtcpPacket::SourceDescription(sdes)])
            .expect_err("sdes item overflow must fail");
        assert!(matches!(
            err,
            RtcpError::SdesItemDataTooLong {
                item_type: 1,
                len: 256
            }
        ));
    }

    #[test]
    fn rtcp_build_rejects_too_many_bye_sources() {
        let bye = RtcpBye {
            ssrcs: vec![0x1122_3344; 32],
            reason: None,
        };

        let err = RtcpPacket::build(&[RtcpPacket::Bye(bye)])
            .expect_err("bye source count overflow must fail");
        assert!(matches!(err, RtcpError::TooManyByeSources { count: 32 }));
    }

    #[test]
    fn rtcp_build_rejects_bye_reason_length_overflow() {
        let bye = RtcpBye {
            ssrcs: vec![0x1122_3344],
            reason: Some("x".repeat(256)),
        };

        let err = RtcpPacket::build(&[RtcpPacket::Bye(bye)])
            .expect_err("bye reason length overflow must fail");
        assert!(matches!(err, RtcpError::ByeReasonTooLong { len: 256 }));
    }

    #[test]
    fn rtcp_build_rejects_app_subtype_overflow() {
        let app = RtcpApp {
            subtype: 32,
            ssrc: 0x1122_3344,
            name: *b"TEST",
            data: vec![1, 2, 3, 4],
        };

        let err =
            RtcpPacket::build(&[RtcpPacket::App(app)]).expect_err("app subtype overflow must fail");
        assert!(matches!(err, RtcpError::InvalidAppSubtype { subtype: 32 }));
    }
}
