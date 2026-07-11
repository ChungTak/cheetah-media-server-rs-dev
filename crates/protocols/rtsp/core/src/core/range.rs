use std::fmt;

/// Errors that can occur while parsing an RTSP `Range` header.
///
/// The `Range` header supports `npt`, `smpte`, and `clock` formats, each with
/// its own validation rules.
///
/// RTSP `Range` 头解析错误。
///
/// `Range` 头支持 `npt`、`smpte`、`clock` 三种格式，每种格式都有各自的校验规则。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RtspRangeError {
    #[error("empty range header")]
    EmptyHeader,
    #[error("unknown range format: {0}")]
    UnknownFormat(String),
    #[error("invalid npt range: {0}")]
    InvalidNptRange(String),
    #[error("invalid npt time: {0}")]
    InvalidNptTime(String),
    #[error("invalid smpte time: {0}")]
    InvalidSmpteTime(String),
    #[error("invalid clock range: {0}")]
    InvalidClockRange(String),
}

/// RTSP `Range` header (RFC 2326 §12.29).
///
/// Dispatches to `npt`, `smpte`, or `clock` range parsing based on the prefix.
///
/// RTSP `Range` 头（RFC 2326 §12.29）。
///
/// 根据前缀分派到 `npt`、`smpte` 或 `clock` 范围解析。
#[derive(Debug, Clone, PartialEq)]
pub enum RtspRange {
    Npt(NptRange),
    Smpte(SmpteRange),
    Clock(ClockRange),
}

impl RtspRange {
    /// Parse a `Range` header value and dispatch to the correct format.
    ///
    /// 解析 `Range` 头值并分派到正确的格式。
    pub fn parse(header_value: &str) -> Result<Self, RtspRangeError> {
        let value = header_value.trim();
        if value.is_empty() {
            return Err(RtspRangeError::EmptyHeader);
        }

        if let Some(rest) = value.strip_prefix("npt=") {
            return Ok(Self::Npt(NptRange::parse(rest)?));
        }
        if let Some(rest) = value.strip_prefix("smpte=") {
            return Ok(Self::Smpte(SmpteRange::parse_with_type(
                rest,
                SmpteType::Smpte,
            )?));
        }
        if let Some(rest) = value.strip_prefix("smpte-30-drop=") {
            return Ok(Self::Smpte(SmpteRange::parse_with_type(
                rest,
                SmpteType::Smpte30Drop,
            )?));
        }
        if let Some(rest) = value.strip_prefix("smpte-25=") {
            return Ok(Self::Smpte(SmpteRange::parse_with_type(
                rest,
                SmpteType::Smpte25,
            )?));
        }
        if let Some(rest) = value.strip_prefix("clock=") {
            return Ok(Self::Clock(ClockRange::parse(rest)?));
        }

        Err(RtspRangeError::UnknownFormat(value.to_string()))
    }
}

impl fmt::Display for RtspRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtspRange::Npt(npt) => write!(f, "npt={npt}"),
            RtspRange::Smpte(smpte) => {
                let prefix = match smpte.smpte_type {
                    SmpteType::Smpte => "smpte",
                    SmpteType::Smpte30Drop => "smpte-30-drop",
                    SmpteType::Smpte25 => "smpte-25",
                };
                write!(f, "{prefix}={smpte}")
            }
            RtspRange::Clock(clock) => write!(f, "clock={clock}"),
        }
    }
}

/// Normal Play Time (NPT) value.
///
/// Can be an absolute second offset, the live-edge keyword `now`, or an
/// `hh:mm:ss` timestamp.
///
/// 正常播放时间（NPT）值。
///
/// 可以是绝对秒偏移、直播边缘关键字 `now` 或 `hh:mm:ss` 时间戳。
#[derive(Debug, Clone, PartialEq)]
pub enum NptTime {
    Now,
    Seconds(f64),
}

impl NptTime {
    /// Parse an NPT time from seconds, `now`, or `hh:mm:ss`.
    ///
    /// 从秒数、`now` 或 `hh:mm:ss` 解析 NPT 时间。
    fn parse(value: &str) -> Result<Self, RtspRangeError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(RtspRangeError::InvalidNptTime("empty time".to_string()));
        }
        if value.eq_ignore_ascii_case("now") {
            return Ok(Self::Now);
        }
        if value.contains(':') {
            return parse_npt_hhmmss(value);
        }

        let seconds = parse_non_negative_f64(value, RtspRangeError::InvalidNptTime)?;
        Ok(Self::Seconds(seconds))
    }
}

impl fmt::Display for NptTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NptTime::Now => write!(f, "now"),
            NptTime::Seconds(seconds) => write!(f, "{seconds}"),
        }
    }
}

/// Parse an `hh:mm:ss[.frac]` timestamp into a seconds value.
///
/// 将 `hh:mm:ss[.frac]` 时间戳解析为秒值。
fn parse_npt_hhmmss(value: &str) -> Result<NptTime, RtspRangeError> {
    let mut parts = value.split(':');
    let Some(hours) = parts.next() else {
        return Err(RtspRangeError::InvalidNptTime(value.to_string()));
    };
    let Some(minutes) = parts.next() else {
        return Err(RtspRangeError::InvalidNptTime(value.to_string()));
    };
    let Some(seconds) = parts.next() else {
        return Err(RtspRangeError::InvalidNptTime(value.to_string()));
    };
    if parts.next().is_some() {
        return Err(RtspRangeError::InvalidNptTime(value.to_string()));
    }

    let hours = parse_non_negative_f64(hours, RtspRangeError::InvalidNptTime)?;
    let minutes = parse_non_negative_f64(minutes, RtspRangeError::InvalidNptTime)?;
    let seconds = parse_non_negative_f64(seconds, RtspRangeError::InvalidNptTime)?;
    Ok(NptTime::Seconds(hours * 3600.0 + minutes * 60.0 + seconds))
}

/// NPT start/end range pair.
///
/// `end == None` means the range is open-ended (play to the end or live).
///
/// NPT 起止范围对。
///
/// `end == None` 表示范围开放（播放到结束或直播）。
#[derive(Debug, Clone, PartialEq)]
pub struct NptRange {
    pub start: NptTime,
    pub end: Option<NptTime>,
}

impl NptRange {
    /// Create an NPT range from explicit start and optional end values.
    ///
    /// 以显式起点和可选终点创建 NPT 范围。
    pub fn new(start: NptTime, end: Option<NptTime>) -> Self {
        Self { start, end }
    }

    /// Create an open-ended NPT range starting from `start` seconds.
    ///
    /// 创建从 `start` 秒开始的开放式 NPT 范围。
    pub fn from_start(start: f64) -> Self {
        Self {
            start: NptTime::Seconds(start),
            end: None,
        }
    }

    /// Create an open-ended NPT range starting from zero.
    ///
    /// 创建从 0 开始的开放式 NPT 范围。
    pub fn all() -> Self {
        Self {
            start: NptTime::Seconds(0.0),
            end: None,
        }
    }

    /// Create an open-ended NPT range anchored at the live edge (`now-`).
    ///
    /// 创建以直播边缘为起点的开放式 NPT 范围（`now-`）。
    pub fn from_now() -> Self {
        Self {
            start: NptTime::Now,
            end: None,
        }
    }

    /// Parse an NPT range, handling `-end`, `start-`, and `start-end` forms.
    ///
    /// 解析 NPT 范围，处理 `-end`、`start-`、`start-end` 形式。
    fn parse(value: &str) -> Result<Self, RtspRangeError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(RtspRangeError::InvalidNptRange("empty range".to_string()));
        }

        if let Some(end) = value.strip_prefix('-') {
            if end.trim().is_empty() {
                return Err(RtspRangeError::InvalidNptRange(
                    "missing end time after '-'".to_string(),
                ));
            }
            return Ok(Self {
                start: NptTime::Seconds(0.0),
                end: Some(NptTime::parse(end)?),
            });
        }

        let mut parts = value.splitn(2, '-');
        let start = parts
            .next()
            .ok_or_else(|| RtspRangeError::InvalidNptRange(value.to_string()))?;
        let end = parts.next();

        if start.trim().is_empty() {
            return Err(RtspRangeError::InvalidNptRange(value.to_string()));
        }
        let start = NptTime::parse(start)?;
        let end = match end {
            Some(part) if !part.trim().is_empty() => Some(NptTime::parse(part)?),
            _ => None,
        };

        Ok(Self { start, end })
    }
}

impl fmt::Display for NptRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            Some(end) => write!(f, "{}-{end}", self.start),
            None => write!(f, "{}-", self.start),
        }
    }
}

/// SMPTE time-code frame rate family.
///
/// SMPTE 时间码帧率族。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmpteType {
    Smpte,
    Smpte30Drop,
    Smpte25,
}

/// SMPTE time-code value.
///
/// SMPTE 时间码值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmpteTime {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub subframes: Option<u8>,
}

impl SmpteTime {
    /// Parse a `hh:mm:ss:frames[.subframes]` SMPTE value with component bounds checks.
    ///
    /// 解析 `hh:mm:ss:frames[.subframes]` SMPTE 值，并校验各分量边界。
    fn parse(value: &str) -> Result<Self, RtspRangeError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(RtspRangeError::InvalidSmpteTime("empty time".to_string()));
        }

        let mut parts = value.split(':');
        let Some(hours) = parts.next() else {
            return Err(RtspRangeError::InvalidSmpteTime(value.to_string()));
        };
        let Some(minutes) = parts.next() else {
            return Err(RtspRangeError::InvalidSmpteTime(value.to_string()));
        };
        let Some(seconds) = parts.next() else {
            return Err(RtspRangeError::InvalidSmpteTime(value.to_string()));
        };

        let hours = parse_u8(hours, RtspRangeError::InvalidSmpteTime)?;
        let minutes = parse_u8(minutes, RtspRangeError::InvalidSmpteTime)?;
        let seconds = parse_u8(seconds, RtspRangeError::InvalidSmpteTime)?;
        if minutes >= 60 || seconds >= 60 {
            return Err(RtspRangeError::InvalidSmpteTime(value.to_string()));
        }

        let mut frames = 0;
        let mut subframes = None;
        if let Some(frame_part) = parts.next() {
            if let Some((frame_value, subframe_value)) = frame_part.split_once('.') {
                frames = parse_u8(frame_value, RtspRangeError::InvalidSmpteTime)?;
                subframes = Some(parse_u8(subframe_value, RtspRangeError::InvalidSmpteTime)?);
            } else {
                frames = parse_u8(frame_part, RtspRangeError::InvalidSmpteTime)?;
            }
        }

        if parts.next().is_some() {
            return Err(RtspRangeError::InvalidSmpteTime(value.to_string()));
        }

        Ok(Self {
            hours,
            minutes,
            seconds,
            frames,
            subframes,
        })
    }
}

impl fmt::Display for SmpteTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )?;
        if let Some(subframes) = self.subframes {
            write!(f, ".{subframes:02}")?;
        }
        Ok(())
    }
}

/// SMPTE start/end range pair.
///
/// SMPTE 起止范围对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmpteRange {
    pub smpte_type: SmpteType,
    pub start: SmpteTime,
    pub end: Option<SmpteTime>,
}

impl SmpteRange {
    /// Parse an SMPTE range for the given frame-rate type.
    ///
    /// 为给定的帧率类型解析 SMPTE 范围。
    fn parse_with_type(value: &str, smpte_type: SmpteType) -> Result<Self, RtspRangeError> {
        let mut parts = value.trim().splitn(2, '-');
        let start = parts
            .next()
            .ok_or_else(|| RtspRangeError::InvalidSmpteTime(value.to_string()))?;
        if start.trim().is_empty() {
            return Err(RtspRangeError::InvalidSmpteTime(value.to_string()));
        }

        let start = SmpteTime::parse(start)?;
        let end = match parts.next() {
            Some(part) if !part.trim().is_empty() => Some(SmpteTime::parse(part)?),
            _ => None,
        };

        Ok(Self {
            smpte_type,
            start,
            end,
        })
    }
}

impl fmt::Display for SmpteRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            Some(end) => write!(f, "{}-{end}", self.start),
            None => write!(f, "{}-", self.start),
        }
    }
}

/// ISO 8601 clock-based start/end range.
///
/// ISO 8601 时钟起止范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockRange {
    pub start: String,
    pub end: Option<String>,
}

impl ClockRange {
    /// Parse a `clock` range with arbitrary start/end strings.
    ///
    /// 解析 `clock` 范围，起点/终点字符串保持原样。
    fn parse(value: &str) -> Result<Self, RtspRangeError> {
        let mut parts = value.trim().splitn(2, '-');
        let start = parts
            .next()
            .ok_or_else(|| RtspRangeError::InvalidClockRange(value.to_string()))?
            .trim()
            .to_string();
        if start.is_empty() {
            return Err(RtspRangeError::InvalidClockRange(value.to_string()));
        }

        let end = match parts.next() {
            Some(part) if !part.trim().is_empty() => Some(part.trim().to_string()),
            _ => None,
        };
        Ok(Self { start, end })
    }
}

impl fmt::Display for ClockRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            Some(end) => write!(f, "{}-{end}", self.start),
            None => write!(f, "{}-", self.start),
        }
    }
}

/// Parse a non-negative finite f64, rejecting NaN, infinity, and negatives.
///
/// 解析非负有限 f64，拒绝 NaN、无穷大和负数。
fn parse_non_negative_f64(
    value: &str,
    make_error: fn(String) -> RtspRangeError,
) -> Result<f64, RtspRangeError> {
    let parsed = value
        .trim()
        .parse::<f64>()
        .map_err(|_| make_error(value.to_string()))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(make_error(value.to_string()));
    }
    Ok(parsed)
}

fn parse_u8(value: &str, make_error: fn(String) -> RtspRangeError) -> Result<u8, RtspRangeError> {
    value
        .trim()
        .parse::<u8>()
        .map_err(|_| make_error(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{NptTime, RtspRange, RtspRangeError, SmpteType};

    #[test]
    fn parses_npt_range_variants() {
        // 秒数格式
        let range = RtspRange::parse("npt=0-").expect("parse npt seconds");
        if let RtspRange::Npt(npt) = range {
            assert!(matches!(npt.start, NptTime::Seconds(value) if value == 0.0));
            assert!(npt.end.is_none());
        } else {
            panic!("expected npt range");
        }

        // 小数秒格式
        let range = RtspRange::parse("npt=10.5-20.3").expect("parse npt decimal seconds");
        if let RtspRange::Npt(npt) = range {
            assert!(matches!(npt.start, NptTime::Seconds(value) if (value - 10.5).abs() < 0.001));
            assert!(
                matches!(npt.end, Some(NptTime::Seconds(value)) if (value - 20.3).abs() < 0.001)
            );
        } else {
            panic!("expected npt range");
        }

        // now 关键字
        let range = RtspRange::parse("npt=now-").expect("parse npt now");
        if let RtspRange::Npt(npt) = range {
            assert!(matches!(npt.start, NptTime::Now));
        } else {
            panic!("expected npt range");
        }

        // hh:mm:ss 格式
        let range = RtspRange::parse("npt=0:10:30-").expect("parse npt hh:mm:ss");
        if let RtspRange::Npt(npt) = range {
            assert!(matches!(npt.start, NptTime::Seconds(value) if (value - 630.0).abs() < 0.001));
        } else {
            panic!("expected npt range");
        }
    }

    #[test]
    fn parses_smpte_range_with_end() {
        let range =
            RtspRange::parse("smpte=0:10:20:00-0:20:30:00").expect("parse smpte range with end");
        if let RtspRange::Smpte(smpte) = range {
            assert_eq!(smpte.start.hours, 0);
            assert_eq!(smpte.start.minutes, 10);
            assert_eq!(smpte.start.seconds, 20);
            assert!(smpte.end.is_some());

            let end = smpte.end.expect("end exists");
            assert_eq!(end.hours, 0);
            assert_eq!(end.minutes, 20);
            assert_eq!(end.seconds, 30);
        } else {
            panic!("expected smpte range");
        }
    }

    #[test]
    fn rejects_invalid_smpte_time_component() {
        let err = RtspRange::parse("smpte=0:61:20:00-0:20:30:00")
            .expect_err("invalid minute in smpte must fail");
        assert!(matches!(err, RtspRangeError::InvalidSmpteTime(_)));
    }

    #[test]
    fn parses_clock_range_without_end() {
        let range = RtspRange::parse("clock=19960213T143205Z-").expect("parse clock range");
        if let RtspRange::Clock(clock) = range {
            assert_eq!(clock.start, "19960213T143205Z");
            assert!(clock.end.is_none());
        } else {
            panic!("expected clock range");
        }
    }

    #[test]
    fn test_smpte_type_preserved() {
        let smpte = RtspRange::parse("smpte=0:10:20:00-").expect("parse smpte");
        let smpte_30_drop =
            RtspRange::parse("smpte-30-drop=0:10:20:00-").expect("parse smpte-30-drop");
        let smpte_25 = RtspRange::parse("smpte-25=0:10:20:00-").expect("parse smpte-25");

        if let RtspRange::Smpte(range) = &smpte {
            assert_eq!(range.smpte_type, SmpteType::Smpte);
        } else {
            panic!("expected smpte range");
        }
        if let RtspRange::Smpte(range) = &smpte_30_drop {
            assert_eq!(range.smpte_type, SmpteType::Smpte30Drop);
        } else {
            panic!("expected smpte-30-drop range");
        }
        if let RtspRange::Smpte(range) = &smpte_25 {
            assert_eq!(range.smpte_type, SmpteType::Smpte25);
        } else {
            panic!("expected smpte-25 range");
        }

        assert_eq!(smpte.to_string(), "smpte=00:10:20:00-");
        assert_eq!(smpte_30_drop.to_string(), "smpte-30-drop=00:10:20:00-");
        assert_eq!(smpte_25.to_string(), "smpte-25=00:10:20:00-");
    }

    #[test]
    fn rejects_unknown_prefix() {
        let err = RtspRange::parse("foobar=1-2").expect_err("unknown format must fail");
        assert!(matches!(err, RtspRangeError::UnknownFormat(value) if value == "foobar=1-2"));
    }

    #[test]
    fn parses_npt_reverse_range() {
        // "-" npt-time：从头开始到指定时间
        let range = RtspRange::parse("npt=-30.5").expect("parse npt reverse range");
        if let RtspRange::Npt(npt) = range {
            assert!(matches!(npt.start, NptTime::Seconds(value) if value == 0.0));
            assert!(
                matches!(npt.end, Some(NptTime::Seconds(value)) if (value - 30.5).abs() < 0.001)
            );
        } else {
            panic!("expected npt range");
        }

        // 只有 "-" 时应报错
        let err = RtspRange::parse("npt=-").expect_err("reverse range without end must fail");
        assert!(matches!(err, RtspRangeError::InvalidNptRange(_)));
    }

    #[test]
    fn test_display() {
        let range = super::NptRange::from_start(10.5);
        assert_eq!(format!("npt={range}"), "npt=10.5-");

        let range = super::NptRange::all();
        assert_eq!(format!("npt={range}"), "npt=0-");
    }
}
