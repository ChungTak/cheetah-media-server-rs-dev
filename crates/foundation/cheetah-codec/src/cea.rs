//! CEA-608 / CEA-708 closed-caption extraction from H.264/H.265 access units.
//!
//! This module is pure Sans-I/O: it does not read wall-clock time, spawn tasks,
//! or depend on a runtime. It parses SEI NALUs, extracts ATSC `cc_data()` packets,
//! runs the CEA-608/708 state machines through `cc-data`, and emits normalized
//! `WebVttCue`s.
//!
//! CEA 608/708 闭字幕从 H.264/H.265 访问单元中提取。

use crate::prelude::*;
use broadcast_common::Parse;
use bytes::Bytes;
use cc_data::decode::{Cea608Channel, Cea608Decoder, Cea708Decoder};
use cc_data::CcData;

use crate::subtitle::WebVttCue;
use crate::time::Timebase;
use crate::track::CodecId;
use crate::video::AccessUnit;

/// Maximum SEI payload size accepted to prevent unbounded allocation.
const MAX_SEI_PAYLOAD_SIZE: usize = 4096;

/// Maximum `cc_data()` packet size (header + 31 triplets + marker).
const MAX_CC_DATA_SIZE: usize = 2 + 31 * 3 + 1;

/// Maximum number of diagnostic messages kept in memory.
const MAX_DIAGNOSTICS: usize = 32;

/// CEA parser configuration.
///
/// 选择默认提取的 608 通道（1–4）和 708 服务号（1–6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CeaParserConfig {
    pub cea608_channel: u8,
    pub cea708_service: u8,
}

impl Default for CeaParserConfig {
    fn default() -> Self {
        Self {
            cea608_channel: 1,
            cea708_service: 1,
        }
    }
}

impl CeaParserConfig {
    /// Creates a config with validated channel/service numbers.
    ///
    /// 校验通道与服务号范围，越界时返回 `CeaError::InvalidConfig`。
    pub fn new(cea608_channel: u8, cea708_service: u8) -> Result<Self, CeaError> {
        if cea608_channel == 0 || cea608_channel > 4 {
            return Err(CeaError::InvalidConfig(format!(
                "cea608_channel must be 1–4, got {cea608_channel}"
            )));
        }
        if cea708_service == 0 || cea708_service > 6 {
            return Err(CeaError::InvalidConfig(format!(
                "cea708_service must be 1–6, got {cea708_service}"
            )));
        }
        Ok(Self {
            cea608_channel,
            cea708_service,
        })
    }
}

/// Errors returned by the CEA parser.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CeaError {
    #[error("invalid CEA parser config: {0}")]
    InvalidConfig(String),
    #[error("malformed SEI or cc_data: {0}")]
    MalformedSei(String),
}

/// Tracks displayed text for a single caption service and emits cues when it changes.
#[derive(Debug, Clone)]
struct ServiceState {
    source: &'static str,
    last_text: String,
    last_pts_ms: Option<u64>,
    sequence: u64,
}

impl ServiceState {
    fn new(source: &'static str) -> Self {
        Self {
            source,
            last_text: String::new(),
            last_pts_ms: None,
            sequence: 0,
        }
    }

    /// Updates the displayed text and returns a cue if it changed since the last update.
    fn emit_if_changed(&mut self, text: &str, pts_ms: u64) -> Option<WebVttCue> {
        if self.last_text == text {
            self.last_pts_ms = Some(pts_ms);
            return None;
        }

        let cue = if let Some(start_ms) = self.last_pts_ms {
            if !self.last_text.is_empty() && pts_ms > start_ms {
                self.sequence += 1;
                Some(WebVttCue {
                    id: Some(format!("{}-{}", self.source, self.sequence)),
                    start_ms,
                    end_ms: pts_ms,
                    payload: self.last_text.clone(),
                    settings: None,
                })
            } else {
                None
            }
        } else {
            None
        };

        self.last_text.clear();
        self.last_text.push_str(text);
        self.last_pts_ms = Some(pts_ms);
        cue
    }

    /// Flushes any pending text as a cue ending at `end_ms`.
    fn flush(&mut self, end_ms: u64) -> Option<WebVttCue> {
        let start_ms = self.last_pts_ms?;
        if self.last_text.is_empty() {
            return None;
        }
        let end = if end_ms > start_ms {
            end_ms
        } else {
            start_ms + 1
        };
        self.sequence += 1;
        let cue = WebVttCue {
            id: Some(format!("{}-{}", self.source, self.sequence)),
            start_ms,
            end_ms: end,
            payload: self.last_text.clone(),
            settings: None,
        };
        self.last_text.clear();
        self.last_pts_ms = None;
        Some(cue)
    }
}

/// Pure Sans-I/O parser that converts H.264/H.265 closed captions to WebVTT cues.
///
/// 无 I/O 的解析器，把 H.264/H.265 闭字幕转换为 WebVTT cue。
#[derive(Debug, Clone)]
pub struct CeaParser {
    config: CeaParserConfig,
    cea608: Cea608Decoder,
    cea708: Cea708Decoder,
    state608: ServiceState,
    state708: ServiceState,
    last_duration_ms: u64,
    diagnostics: Vec<String>,
    dropped_diagnostics: u64,
}

impl CeaParser {
    /// Creates a new parser with the given config.
    pub fn new(config: CeaParserConfig) -> Self {
        Self {
            config,
            cea608: Cea608Decoder::new(),
            cea708: Cea708Decoder::new(),
            state608: ServiceState::new("cea608"),
            state708: ServiceState::new("cea708"),
            last_duration_ms: 0,
            diagnostics: Vec::new(),
            dropped_diagnostics: 0,
        }
    }

    /// Returns accumulated diagnostics and clears the internal buffer.
    pub fn take_diagnostics(&mut self) -> Vec<String> {
        self.dropped_diagnostics = 0;
        core::mem::take(&mut self.diagnostics)
    }

    /// Returns a slice of accumulated diagnostics without consuming them.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Resets decoder state and flushes pending cues, typically on seek/discontinuity.
    ///
    /// `end_ms` is used as the end timestamp for any cue still open; pass the
    /// current access-unit PTS if available.
    pub fn reset(&mut self, end_ms: Option<u64>) -> Vec<WebVttCue> {
        let mut cues = Vec::new();
        let end = end_ms.unwrap_or_else(|| {
            self.state608
                .last_pts_ms
                .or(self.state708.last_pts_ms)
                .unwrap_or(1)
                + self.last_duration_ms
        });
        if let Some(cue) = self.state608.flush(end) {
            cues.push(cue);
        }
        if let Some(cue) = self.state708.flush(end) {
            cues.push(cue);
        }
        self.cea608 = Cea608Decoder::new();
        self.cea708 = Cea708Decoder::new();
        cues
    }

    fn push_diagnostic(&mut self, message: &str) {
        if self.diagnostics.len() < MAX_DIAGNOSTICS {
            self.diagnostics.push(message.to_string());
        } else if self.diagnostics.len() == MAX_DIAGNOSTICS {
            self.dropped_diagnostics += 1;
            self.diagnostics.push(format!(
                "... {} further diagnostics dropped",
                self.dropped_diagnostics
            ));
        } else {
            self.dropped_diagnostics += 1;
        }
    }

    /// Feeds one access unit and returns any cues whose text changed.
    ///
    /// `codec` must be `CodecId::H264` or `CodecId::H265`; other codecs produce no
    /// output and are not treated as an error.
    pub fn push_access_unit(&mut self, codec: CodecId, unit: &AccessUnit) -> Vec<WebVttCue> {
        let timing = match &unit.timing {
            Some(t) => t,
            None => {
                self.push_diagnostic("access unit has no timing");
                return Vec::new();
            }
        };

        let pts_us = Timebase::to_micros(timing.timebase, timing.pts);
        let duration_us = Timebase::to_micros(timing.timebase, timing.duration);
        let pts_ms = pts_us.max(0) as u64 / 1000;
        let duration_ms = duration_us.max(0) as u64 / 1000;
        self.last_duration_ms = duration_ms.max(1);

        for nal in &unit.units {
            if let Err(e) = self.push_nal(codec, nal) {
                self.push_diagnostic(&format!("{e}"));
            }
        }

        let mut cues = Vec::new();
        if let Some(cue) = self.emit_608(pts_ms) {
            cues.push(cue);
        }
        if let Some(cue) = self.emit_708(pts_ms) {
            cues.push(cue);
        }
        cues
    }

    fn emit_608(&mut self, pts_ms: u64) -> Option<WebVttCue> {
        let channel = match self.config.cea608_channel {
            1 => Cea608Channel::Cc1,
            2 => Cea608Channel::Cc2,
            3 => Cea608Channel::Cc3,
            _ => Cea608Channel::Cc4,
        };
        let text = self.cea608.channel_text(channel);
        self.state608.emit_if_changed(&text, pts_ms)
    }

    fn emit_708(&mut self, pts_ms: u64) -> Option<WebVttCue> {
        let text = self
            .cea708
            .service_text(self.config.cea708_service as usize);
        self.state708.emit_if_changed(&text, pts_ms)
    }

    fn push_nal(&mut self, codec: CodecId, nal: &Bytes) -> Result<(), CeaError> {
        if nal.is_empty() {
            return Ok(());
        }

        let payload = match codec {
            CodecId::H264 => {
                let nal_type = nal[0] & 0x1F;
                if nal_type != 6 {
                    return Ok(());
                }
                &nal[1..]
            }
            CodecId::H265 => {
                if nal.len() < 2 {
                    return Ok(());
                }
                let nal_type = (nal[0] >> 1) & 0x3F;
                if !matches!(nal_type, 39 | 40) {
                    return Ok(());
                }
                &nal[2..]
            }
            _ => return Ok(()),
        };

        self.parse_sei_payload(payload)
    }

    /// Parses the SEI payload of a single NALU, extracting any `cc_data()` packets.
    fn parse_sei_payload(&mut self, mut data: &[u8]) -> Result<(), CeaError> {
        while !data.is_empty() {
            if is_trailing_or_padding(data) {
                return Ok(());
            }
            if data.len() < 2 {
                return Err(CeaError::MalformedSei(
                    "truncated SEI message header".to_string(),
                ));
            }
            let (payload_type, size, consumed) = read_sei_message_header(data)?;
            if consumed + size > data.len() {
                return Err(CeaError::MalformedSei(
                    "SEI payload size exceeds NALU".to_string(),
                ));
            }
            let payload = &data[consumed..consumed + size];

            if payload_type == 4 {
                if let Some(cc) = self.parse_registered_user_data(payload)? {
                    self.feed_cc_data(cc)?;
                }
            }

            data = &data[consumed + size..];
        }
        Ok(())
    }

    /// Parses `user_data_registered_itu_t_t35` looking for ATSC GA94 cc_data.
    fn parse_registered_user_data(&self, payload: &[u8]) -> Result<Option<CcData>, CeaError> {
        if payload.is_empty() {
            return Ok(None);
        }

        let mut p = payload;
        // itu_t_t35_country_code. 0xFF means an extension byte follows.
        if p[0] == 0xFF {
            if p.len() < 2 {
                return Err(CeaError::MalformedSei(
                    "truncated country code extension".to_string(),
                ));
            }
            p = &p[2..];
        } else {
            p = &p[1..];
        }

        if p.len() < 2 {
            return Ok(None);
        }
        let provider = u16::from_be_bytes([p[0], p[1]]);
        p = &p[2..];
        if provider != 0x0031 {
            // Not ATSC; ignore.
            return Ok(None);
        }

        if p.len() < 4 {
            return Ok(None);
        }
        let user_id = [p[0], p[1], p[2], p[3]];
        p = &p[4..];
        if &user_id != b"GA94" {
            return Ok(None);
        }

        if p.is_empty() {
            return Ok(None);
        }
        let user_type = p[0];
        p = &p[1..];
        if user_type != 0x03 {
            return Ok(None);
        }

        if p.len() > MAX_CC_DATA_SIZE {
            return Err(CeaError::MalformedSei(
                "cc_data packet too large".to_string(),
            ));
        }

        match CcData::parse(p) {
            Ok(cc) => Ok(Some(cc)),
            Err(e) => Err(CeaError::MalformedSei(format!("cc_data parse: {e}"))),
        }
    }

    fn feed_cc_data(&mut self, cc: CcData) -> Result<(), CeaError> {
        if !cc.process_cc_data_flag {
            return Ok(());
        }
        self.cea608.push_triplets(&cc.triplets);
        self.cea708.push_triplets(&cc.triplets);
        Ok(())
    }
}

/// Returns true if `data` is RBSP trailing bits or zero-byte padding.
///
/// A valid NALU ends with a `1` bit followed by zero bits to the next byte
/// boundary (commonly `0x80`) and possibly additional zero bytes; we tolerate
/// that without treating it as a malformed SEI message.
fn is_trailing_or_padding(data: &[u8]) -> bool {
    let mut seen_one = false;
    for &b in data {
        if b == 0 {
            continue;
        }
        if seen_one || !b.is_power_of_two() {
            return false;
        }
        seen_one = true;
    }
    true
}

/// Reads a SEI message `payload_type` and `payload_size` (both are FF-extended bytes).
///
/// Returns `(type, size, header_len)`.
fn read_sei_message_header(data: &[u8]) -> Result<(usize, usize, usize), CeaError> {
    let mut payload_type = 0usize;
    let mut pos = 0usize;

    loop {
        if pos >= data.len() {
            return Err(CeaError::MalformedSei(
                "truncated SEI message type".to_string(),
            ));
        }
        let b = data[pos];
        pos += 1;
        if b == 0xFF {
            payload_type += 255;
        } else {
            payload_type += usize::from(b);
            break;
        }
        if payload_type > MAX_SEI_PAYLOAD_SIZE {
            return Err(CeaError::MalformedSei(
                "SEI payload type too large".to_string(),
            ));
        }
    }

    let mut payload_size = 0usize;
    loop {
        if pos >= data.len() {
            return Err(CeaError::MalformedSei(
                "truncated SEI message size".to_string(),
            ));
        }
        let b = data[pos];
        pos += 1;
        if b == 0xFF {
            payload_size += 255;
        } else {
            payload_size += usize::from(b);
            break;
        }
        if payload_size > MAX_SEI_PAYLOAD_SIZE {
            return Err(CeaError::MalformedSei(
                "SEI payload size too large".to_string(),
            ));
        }
    }

    Ok((payload_type, payload_size, pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle::WebVttFrame;
    use crate::time::Timebase;
    use crate::video::{AccessUnit, AccessUnitTiming};

    fn make_access_unit(pts: i64, duration: i64, units: Vec<Bytes>) -> AccessUnit {
        AccessUnit {
            units,
            timing: Some(AccessUnitTiming {
                pts,
                dts: pts,
                duration,
                timebase: Timebase::new(1, 90_000),
            }),
            random_access: false,
            parameter_set_requirement: Default::default(),
        }
    }

    fn make_h264_sei_nalu(user_data_payload: &[u8]) -> Bytes {
        let mut out = Vec::new();
        out.push(0x06); // SEI NAL unit type
                        // SEI payload type 4 = user_data_registered_itu_t_t35
        out.push(0x04);
        let mut size = user_data_payload.len();
        while size >= 255 {
            out.push(0xFF);
            size -= 255;
        }
        out.push(size as u8);
        out.extend_from_slice(user_data_payload);
        Bytes::from(out)
    }

    fn make_cea_payload(text: &str) -> Vec<u8> {
        // Build a minimal registered user_data with GA94 / CC data type 0x03
        // followed by an empty cc_data() packet (process=0, count=0, marker=0xFF).
        let mut payload = Vec::new();
        payload.push(0xB5); // country code USA
        payload.extend_from_slice(&0x0031u16.to_be_bytes()); // ATSC provider
        payload.extend_from_slice(b"GA94");
        payload.push(0x03); // user data type code for CC data
        payload.push(0x80); // process=0, count=0
        payload.push(0xFF); // reserved
        payload.push(0xFF); // marker
        let _ = text; // text content would be encoded in cc_data triplets
        payload
    }

    #[test]
    fn ignores_non_sei_nal() {
        let mut parser = CeaParser::new(CeaParserConfig::default());
        let unit = make_access_unit(0, 3000, vec![Bytes::from_static(&[0x09, 0xF0])]);
        let cues = parser.push_access_unit(CodecId::H264, &unit);
        assert!(cues.is_empty());
    }

    #[test]
    fn malformed_sei_is_diagnosed_not_panicking() {
        let mut parser = CeaParser::new(CeaParserConfig::default());
        // NAL type 6 with an incomplete SEI payload-type extension.
        let nal = Bytes::from_static(&[0x06, 0xFF]);
        let unit = make_access_unit(0, 3000, vec![nal]);
        let cues = parser.push_access_unit(CodecId::H264, &unit);
        assert!(cues.is_empty());
        assert!(!parser.diagnostics().is_empty());
    }

    #[test]
    fn empty_registered_user_data_yields_no_cues() {
        let mut parser = CeaParser::new(CeaParserConfig::default());
        let sei_payload = make_cea_payload("");
        let nal = make_h264_sei_nalu(&sei_payload);
        let unit = make_access_unit(0, 3000, vec![nal]);
        let cues = parser.push_access_unit(CodecId::H264, &unit);
        assert!(cues.is_empty());
    }

    #[test]
    fn pop_on_608_emits_cue_on_change() {
        let mut parser = CeaParser::new(CeaParserConfig::default());
        // RCL, PAC row 15 indent 0, "HI", EOC (field 1, CC1)
        let triplets = [
            (true, 0x14, 0x20), // RCL
            (true, 0x14, 0x70), // PAC
            (true, b'H', b'I'),
            (true, 0x14, 0x2F), // EOC
        ];

        let mut cc_payload = Vec::new();
        cc_payload.push(0xB5);
        cc_payload.extend_from_slice(&0x0031u16.to_be_bytes());
        cc_payload.extend_from_slice(b"GA94");
        cc_payload.push(0x03);
        // cc_data header: process=1, count=4
        cc_payload.push(0xC4);
        cc_payload.push(0xFF);
        for (valid, d1, d2) in triplets {
            let flags = 0xF8 | (u8::from(valid) << 2);
            cc_payload.push(flags);
            cc_payload.push(d1);
            cc_payload.push(d2);
        }
        cc_payload.push(0xFF);

        let nal = make_h264_sei_nalu(&cc_payload);
        let unit = make_access_unit(0, 3000, vec![nal]);
        let cues = parser.push_access_unit(CodecId::H264, &unit);

        // The cue is emitted when the *next* access unit changes the displayed text
        // or when flush/reset is called. Without a follow-up unit, flush it.
        let mut all_cues = cues;
        all_cues.extend(parser.reset(None));

        assert_eq!(all_cues.len(), 1);
        let cue = &all_cues[0];
        assert!(cue.payload.contains('H'));
        assert!(cue.payload.contains('I'));
        assert!(cue.start_ms < cue.end_ms);
    }

    #[test]
    fn webvtt_frame_helpers_work() {
        let cue = WebVttCue {
            id: None,
            start_ms: 0,
            end_ms: 1000,
            payload: "hello".to_string(),
            settings: None,
        };
        let frame = WebVttFrame {
            cues: vec![cue],
            styles: Vec::new(),
            regions: Vec::new(),
        };
        assert_eq!(frame.len(), 1);
        assert!(!frame.is_empty());
    }

    #[test]
    fn h265_one_byte_sei_does_not_panic() {
        // A single H.265 NAL byte claiming to be a prefix SEI (type 39) with no
        // second header byte and no payload must be ignored, not panic.
        let mut parser = CeaParser::new(CeaParserConfig::default());
        let nal = Bytes::from_static(&[0x4E]); // 0b01001110 -> nal_unit_type 39
        let unit = make_access_unit(0, 3000, vec![nal]);
        let cues = parser.push_access_unit(CodecId::H265, &unit);
        assert!(cues.is_empty());
    }

    #[test]
    fn h264_sei_trailing_bits_are_tolerated() {
        let mut parser = CeaParser::new(CeaParserConfig::default());
        let payload = make_h264_sei_nalu(&make_cea_payload(""));
        let mut raw = payload.to_vec();
        raw.extend_from_slice(&[0x80, 0x00]); // RBSP trailing bits + padding
        let nal = Bytes::from(raw);
        let unit = make_access_unit(0, 3000, vec![nal]);
        let cues = parser.push_access_unit(CodecId::H264, &unit);
        assert!(cues.is_empty());
        assert!(parser.diagnostics().is_empty());
    }

    #[test]
    fn diagnostics_are_capped() {
        let mut parser = CeaParser::new(CeaParserConfig::default());
        for _ in 0..MAX_DIAGNOSTICS + 10 {
            // Each call pushes one malformed-SEI diagnostic.
            let nal = Bytes::from_static(&[0x06, 0xFF]);
            let unit = make_access_unit(0, 3000, vec![nal]);
            parser.push_access_unit(CodecId::H264, &unit);
        }
        assert!(parser.diagnostics().len() <= MAX_DIAGNOSTICS + 1);
        let taken = parser.take_diagnostics();
        assert!(!taken.is_empty());
        assert!(parser.diagnostics().is_empty());
    }
}
