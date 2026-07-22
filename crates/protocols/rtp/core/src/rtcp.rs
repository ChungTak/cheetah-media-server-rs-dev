//! RTCP compound packet parser/encoder (RFC 3550).
//!
//! Supports Sender Report (SR), Receiver Report (RR), Source Description (SDES)
//! and BYE packets. This is a Sans-I/O data-plane helper: it does not manage
//! timers or session state.
//!
//! RTCP 复合包解析/编码（RFC 3550）。
//!
//! 支持发送者报告（SR）、接收者报告（RR）、源描述（SDES）和 BYE 包。
//! 这是一个 Sans-I/O 数据面辅助模块，不管理定时器或会话状态。

mod bye_app;
mod compound;
mod error;
mod report;
mod sdes;

pub use bye_app::{RtcpAppPacket, RtcpBye};
pub use compound::{RtcpCompoundPacket, RtcpPacket};
pub use error::{RtcpEncodeError, RtcpPacketType, RtcpParseError};
pub use report::{RtcpReceiverReport, RtcpReportBlock, RtcpSenderReport};
pub use sdes::{RtcpSdesChunk, RtcpSdesItem, RtcpSdesItemType, RtcpSourceDescription};

fn sign_extend_24(value: u32) -> i32 {
    let v = value & 0x00ff_ffff;
    if (v & 0x0080_0000) != 0 {
        (v | 0xff00_0000) as i32
    } else {
        v as i32
    }
}

fn padding_to_4(len: usize) -> usize {
    let rem = len % 4;
    if rem == 0 {
        0
    } else {
        4 - rem
    }
}

#[cfg(test)]
mod tests;
