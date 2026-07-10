use crate::prelude::*;

use bytes::Bytes;

use crate::amf::Pair;
use crate::amf3::Amf3Value;
use crate::bytes::{BytesReader, BytesWriter};
use crate::error::{Error, ErrorKind};

const MARKER_NUMBER: u8 = 0x00;
const MARKER_BOOLEAN: u8 = 0x01;
const MARKER_STRING: u8 = 0x02;
const MARKER_OBJECT: u8 = 0x03;
const MARKER_MOVIECLIP: u8 = 0x04;
const MARKER_NULL: u8 = 0x05;
const MARKER_UNDEFINED: u8 = 0x06;
const MARKER_REFERENCE: u8 = 0x07;
const MARKER_ECMA_ARRAY: u8 = 0x08;
const MARKER_OBJECT_END_MARKER: u8 = 0x09;
const MARKER_STRICT_ARRAY: u8 = 0x0A;
const MARKER_DATE: u8 = 0x0B;
const MARKER_LONG_STRING: u8 = 0x0C;
const MARKER_UNSUPPORTED: u8 = 0x0D;
const MARKER_RECORDSET: u8 = 0x0E;
const MARKER_XML_DOCUMENT: u8 = 0x0F;
const MARKER_TYPED_OBJECT: u8 = 0x10;
const MARKER_AVMPLUS_OBJECT: u8 = 0x11;

/// `Amf0Value` enumeration.
/// `Amf0Value` 枚举.
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum Amf0Value {
    /// `Number` variant.
    /// `Number` 变体.
    Number(f64),
    /// `Boolean` variant.
    /// `Boolean` 变体.
    Boolean(bool),
    /// `String` variant.
    /// `String` 变体.
    String(String),
    /// `Object` variant.
    /// `Object` 变体.
    Object {
        // `None` 表示匿名对象
        class_name: Option<String>,
        entries: Vec<Pair<String, Self>>,
    },
    /// `Null` variant.
    /// `Null` 变体.
    Null,
    /// `Undefined` variant.
    /// `Undefined` 变体.
    Undefined,
    /// `EcmaArray` variant.
    /// `EcmaArray` 变体.
    EcmaArray { entries: Vec<Pair<String, Self>> },
    /// `Array` variant.
    /// `Array` 变体.
    Array { entries: Vec<Self> },
    /// `Date` variant.
    /// `Date` 变体.
    Date { unix_time_ms: i64 },
    /// `XmlDocument` variant.
    /// `XmlDocument` 变体.
    XmlDocument(String),
    /// `AvmPlus` variant.
    /// `AvmPlus` 变体.
    AvmPlus(Amf3Value),
}

impl Amf0Value {
    /// `decode` function.
    /// `decode` 函数.
    pub fn decode(buf: &[u8]) -> Result<(usize, Self), Error> {
        let original_buf_len = buf.len();
        let mut decoder = Decoder {
            buf,
            complexes: Vec::new(),
            decoding: alloc::collections::BTreeSet::new(),
        };
        let value = decoder.decode_value()?;
        Ok((original_buf_len - decoder.buf.len(), value))
    }

    /// `encode` function.
    /// `encode` 函数.
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut encoder = Encoder {
            buf,
            complexes: Vec::new(),
        };
        encoder.encode_value(self);
    }

    /// `as_str` function.
    /// `as_str` 函数.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(v) => Some(v),
            _ => None,
        }
    }

    /// `as_f64` function.
    /// `as_f64` 函数.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(v) => Some(*v),
            _ => None,
        }
    }

    /// `as_object_entries` function.
    /// `as_object_entries` 函数.
    pub fn as_object_entries(&self) -> Option<&[Pair<String, Self>]> {
        match self {
            Self::Object { entries, .. } | Self::EcmaArray { entries } => Some(entries),
            _ => None,
        }
    }

    /// `object` function.
    /// `object` 函数.
    pub fn object<K, I>(entries: I) -> Self
    where
        K: Into<String>,
        I: IntoIterator<Item = (K, Self)>,
    {
        Self::Object {
            class_name: None,
            entries: entries
                .into_iter()
                .map(|(key, value)| Pair {
                    key: key.into(),
                    value,
                })
                .collect(),
        }
    }

    /// `empty_object` function.
    /// `empty_object` 函数.
    pub fn empty_object() -> Self {
        Self::Object {
            class_name: None,
            entries: Vec::new(),
        }
    }

    /// `ecma_array` function.
    /// `ecma_array` 函数.
    pub fn ecma_array<K, I>(entries: I) -> Self
    where
        K: Into<String>,
        I: IntoIterator<Item = (K, Self)>,
    {
        Self::EcmaArray {
            entries: entries
                .into_iter()
                .map(|(key, value)| Pair {
                    key: key.into(),
                    value,
                })
                .collect(),
        }
    }
}

/// `Amf0Error` enumeration.
/// `Amf0Error` 枚举.
#[derive(Debug, thiserror::Error)]
pub enum Amf0Error {
    /// `UnexpectedEof` variant.
    /// `UnexpectedEof` 变体.
    #[error("unexpected eof")]
    UnexpectedEof,
    /// `UnsupportedMarker` variant.
    /// `UnsupportedMarker` 变体.
    #[error("unsupported amf0 marker {0:#x}")]
    UnsupportedMarker(u8),
    /// `Unsupported` variant.
    /// `Unsupported` 变体.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// `InvalidUtf8` variant.
    /// `InvalidUtf8` 变体.
    #[error("invalid utf8 string")]
    InvalidUtf8,
}

/// `decode_all` function.
/// `decode_all` 函数.
pub fn decode_all(mut payload: &[u8]) -> Result<Vec<Amf0Value>, Amf0Error> {
    let mut values = Vec::new();
    while !payload.is_empty() {
        let (size, value) = Amf0Value::decode(payload).map_err(map_decode_error)?;
        values.push(value);
        payload = &payload[size..];
    }
    Ok(values)
}

/// `encode_all` function.
/// `encode_all` 函数.
pub fn encode_all(values: &[Amf0Value]) -> Bytes {
    let mut out = Vec::new();
    let mut encoder = Encoder {
        buf: &mut out,
        complexes: Vec::new(),
    };
    for value in values {
        encoder.encode_value(value);
    }
    drop(encoder);
    Bytes::from(out)
}

fn map_decode_error(error: Error) -> Amf0Error {
    match error.kind {
        ErrorKind::InsufficientBuffer => Amf0Error::UnexpectedEof,
        ErrorKind::InvalidData if error.reason.contains("UTF-8") => Amf0Error::InvalidUtf8,
        ErrorKind::Unsupported => {
            let marker = error
                .reason
                .rsplit_once(' ')
                .and_then(|(_, s)| s.parse::<u8>().ok());
            match marker {
                Some(m) => Amf0Error::UnsupportedMarker(m),
                None => Amf0Error::Unsupported(error.reason),
            }
        }
        ErrorKind::InvalidData => Amf0Error::Unsupported(error.reason),
        _ => Amf0Error::Unsupported(error.reason),
    }
}

#[derive(Debug)]
struct Decoder<'a> {
    buf: &'a [u8],
    complexes: Vec<Amf0Value>,
    decoding: alloc::collections::BTreeSet<usize>,
}

impl Decoder<'_> {
    fn decode_value(&mut self) -> Result<Amf0Value, Error> {
        let marker = self.buf.read_u8()?;
        match marker {
            MARKER_NUMBER => self.decode_number(),
            MARKER_BOOLEAN => self.decode_boolean(),
            MARKER_STRING => self.decode_string(),
            MARKER_OBJECT => self.decode_object(),
            MARKER_NULL => Ok(Amf0Value::Null),
            MARKER_UNDEFINED => Ok(Amf0Value::Undefined),
            MARKER_REFERENCE => self.decode_reference(),
            MARKER_ECMA_ARRAY => self.decode_ecma_array(),
            MARKER_STRICT_ARRAY => self.decode_strict_array(),
            MARKER_DATE => self.decode_date(),
            MARKER_LONG_STRING => self.decode_long_string(),
            MARKER_XML_DOCUMENT => self.decode_xml_document(),
            MARKER_TYPED_OBJECT => self.decode_typed_object(),
            MARKER_AVMPLUS_OBJECT => self.decode_avmplus(),
            MARKER_MOVIECLIP | MARKER_RECORDSET | MARKER_UNSUPPORTED => Err(Error::unsupported(
                format!("unsupported AMF0 marker {marker}"),
            )),
            _ => Err(Error::invalid_data(format!(
                "unexpected AMF0 marker {marker}"
            ))),
        }
    }

    fn decode_avmplus(&mut self) -> Result<Amf0Value, Error> {
        let (bytes_read, amf3_value) = Amf3Value::decode(self.buf)?;
        self.buf = &self.buf[bytes_read..];
        Ok(Amf0Value::AvmPlus(amf3_value))
    }

    fn decode_xml_document(&mut self) -> Result<Amf0Value, Error> {
        let len = self.buf.read_u32()? as usize;
        let s = self.buf.read_utf8(len)?;
        Ok(Amf0Value::XmlDocument(s))
    }

    fn decode_strict_array(&mut self) -> Result<Amf0Value, Error> {
        self.decode_complex_type(|this| {
            let count = this.buf.read_u32()? as usize;
            let entries = (0..count)
                .map(|_| this.decode_value())
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Amf0Value::Array { entries })
        })
    }

    fn decode_ecma_array(&mut self) -> Result<Amf0Value, Error> {
        self.decode_complex_type(|this| {
            let _count = this.buf.read_u32()?;
            let entries = this.decode_pairs()?;
            Ok(Amf0Value::EcmaArray { entries })
        })
    }

    fn decode_date(&mut self) -> Result<Amf0Value, Error> {
        self.decode_complex_type(|this| {
            let millis = this.buf.read_f64()?;
            let _time_zone = this.buf.read_u16()?;

            if !millis.is_finite() {
                return Err(Error::invalid_data(format!(
                    "invalid date: millis must be finite, got {millis}"
                )));
            }
            let unix_time_ms = if millis == 0.0 { 0i64 } else { millis as i64 };
            Ok(Amf0Value::Date { unix_time_ms })
        })
    }

    fn decode_typed_object(&mut self) -> Result<Amf0Value, Error> {
        self.decode_complex_type(|this| {
            let len = this.buf.read_u16()? as usize;
            let class_name = this.buf.read_utf8(len)?;
            let entries = this.decode_pairs()?;
            Ok(Amf0Value::Object {
                class_name: Some(class_name),
                entries,
            })
        })
    }

    fn decode_reference(&mut self) -> Result<Amf0Value, Error> {
        let index = self.buf.read_u16()? as usize;
        let v = self
            .complexes
            .get(index)
            .ok_or_else(|| Error::invalid_data(format!("reference index out of range: {index}")))?;

        if self.decoding.contains(&index) {
            return Err(Error::unsupported(format!(
                "circular reference at index {index}"
            )));
        }

        Ok(v.clone())
    }

    fn decode_number(&mut self) -> Result<Amf0Value, Error> {
        let n = self.buf.read_f64()?;
        Ok(Amf0Value::Number(n))
    }

    fn decode_boolean(&mut self) -> Result<Amf0Value, Error> {
        let b = self.buf.read_u8()? != 0;
        Ok(Amf0Value::Boolean(b))
    }

    fn decode_string(&mut self) -> Result<Amf0Value, Error> {
        let len = self.buf.read_u16()? as usize;
        let s = self.buf.read_utf8(len)?;
        Ok(Amf0Value::String(s))
    }

    fn decode_long_string(&mut self) -> Result<Amf0Value, Error> {
        let len = self.buf.read_u32()? as usize;
        let s = self.buf.read_utf8(len)?;
        Ok(Amf0Value::String(s))
    }

    fn decode_object(&mut self) -> Result<Amf0Value, Error> {
        self.decode_complex_type(|this| {
            let entries = this.decode_pairs()?;
            Ok(Amf0Value::Object {
                class_name: None,
                entries,
            })
        })
    }

    fn decode_pairs(&mut self) -> Result<Vec<Pair<String, Amf0Value>>, Error> {
        let mut entries = Vec::new();
        loop {
            let key_len = self.buf.read_u16()? as usize;
            let key = self.buf.read_utf8(key_len)?;

            let marker = self.buf.first().copied();
            if marker == Some(MARKER_OBJECT_END_MARKER) {
                let _ = self.buf.read_u8()?;
                break;
            }

            let value = self.decode_value()?;
            entries.push(Pair { key, value });
        }
        Ok(entries)
    }

    fn decode_complex_type<F>(&mut self, f: F) -> Result<Amf0Value, Error>
    where
        F: FnOnce(&mut Self) -> Result<Amf0Value, Error>,
    {
        let index = self.complexes.len();
        self.complexes.push(Amf0Value::Null);
        self.decoding.insert(index);
        let value = f(self)?;
        self.decoding.remove(&index);
        self.complexes[index] = value.clone();
        Ok(value)
    }
}

struct Encoder<'a> {
    buf: &'a mut Vec<u8>,
    complexes: Vec<*const Amf0Value>,
}

impl<'a> Encoder<'a> {
    fn encode_value(&mut self, v: &Amf0Value) {
        match v {
            Amf0Value::Object {
                class_name,
                entries,
            } => {
                self.encode_complex(v, |this| this.encode_object(class_name, entries));
            }
            Amf0Value::EcmaArray { entries } => {
                self.encode_complex(v, |this| this.encode_ecma_array(entries));
            }
            Amf0Value::Array { entries } => {
                self.encode_complex(v, |this| this.encode_strict_array(entries));
            }
            Amf0Value::Date { unix_time_ms } => {
                self.encode_complex(v, |this| this.encode_date(*unix_time_ms));
            }
            Amf0Value::Number(v) => self.encode_number(*v),
            Amf0Value::Boolean(v) => self.encode_boolean(*v),
            Amf0Value::String(v) => self.encode_string(v),
            Amf0Value::Null => self.buf.write_u8(MARKER_NULL),
            Amf0Value::Undefined => self.buf.write_u8(MARKER_UNDEFINED),
            Amf0Value::XmlDocument(v) => self.encode_xml_document(v),
            Amf0Value::AvmPlus(v) => self.encode_avmplus(v),
        }
    }

    fn encode_complex<F>(&mut self, v: &Amf0Value, f: F)
    where
        F: FnOnce(&mut Self),
    {
        let ptr = v as *const Amf0Value;
        if let Some(idx) = self.complexes.iter().position(|&p| p == ptr) {
            if idx <= u16::MAX as usize {
                self.buf.write_u8(MARKER_REFERENCE);
                self.buf.write_u16(idx as u16);
                return;
            }
            f(self);
            return;
        }
        if self.complexes.len() > u16::MAX as usize {
            f(self);
            return;
        }
        self.complexes.push(ptr);
        f(self);
    }

    fn encode_avmplus(&mut self, v: &Amf3Value) {
        self.buf.write_u8(MARKER_AVMPLUS_OBJECT);
        v.encode(self.buf);
    }

    fn encode_xml_document(&mut self, xml: &str) {
        self.buf.write_u8(MARKER_XML_DOCUMENT);
        self.buf.write_u32(xml.len() as u32);
        self.buf.write_bytes(xml.as_bytes());
    }

    fn encode_date(&mut self, unix_time_ms: i64) {
        self.buf.write_u8(MARKER_DATE);
        self.buf.write_f64(unix_time_ms as f64);
        self.buf.write_u16(0);
    }

    fn encode_strict_array(&mut self, entries: &[Amf0Value]) {
        self.buf.write_u8(MARKER_STRICT_ARRAY);
        self.buf.write_u32(entries.len() as u32);
        for entry in entries {
            self.encode_value(entry);
        }
    }

    fn encode_ecma_array(&mut self, entries: &[Pair<String, Amf0Value>]) {
        self.buf.write_u8(MARKER_ECMA_ARRAY);
        self.buf.write_u32(entries.len() as u32);
        self.encode_pairs(entries);
    }

    fn encode_number(&mut self, n: f64) {
        self.buf.write_u8(MARKER_NUMBER);
        self.buf.write_f64(n);
    }

    fn encode_boolean(&mut self, b: bool) {
        self.buf.write_u8(MARKER_BOOLEAN);
        self.buf.write_u8(if b { 1 } else { 0 });
    }

    fn encode_string(&mut self, s: &str) {
        if s.len() <= 0xFFFF {
            self.buf.write_u8(MARKER_STRING);
            self.buf.write_u16(s.len() as u16);
        } else {
            self.buf.write_u8(MARKER_LONG_STRING);
            self.buf.write_u32(s.len() as u32);
        }
        self.buf.write_bytes(s.as_bytes());
    }

    fn encode_object(&mut self, class_name: &Option<String>, entries: &[Pair<String, Amf0Value>]) {
        if let Some(class_name) = class_name {
            self.buf.write_u8(MARKER_TYPED_OBJECT);
            debug_assert!(
                class_name.len() <= u16::MAX as usize,
                "class name exceeds u16 length limit"
            );
            self.buf.write_u16(class_name.len() as u16);
            self.buf.write_bytes(class_name.as_bytes());
        } else {
            self.buf.write_u8(MARKER_OBJECT);
        }
        self.encode_pairs(entries);
    }

    fn encode_pairs(&mut self, entries: &[Pair<String, Amf0Value>]) {
        for pair in entries {
            debug_assert!(
                pair.key.len() <= u16::MAX as usize,
                "key exceeds u16 length limit"
            );
            self.buf.write_u16(pair.key.len() as u16);
            self.buf.write_bytes(pair.key.as_bytes());
            self.encode_value(&pair.value);
        }
        self.buf.write_u16(0); // 空键表示终止符
        self.buf.write_u8(MARKER_OBJECT_END_MARKER);
    }
}
