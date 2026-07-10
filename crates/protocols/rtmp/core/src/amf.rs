use crate::amf0::Amf0Value;
use crate::amf3::Amf3Value;
use crate::error::Error;
use crate::prelude::*;

/// `AmfVersion` enumeration.
/// `AmfVersion` 枚举。
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum AmfVersion {
    Amf0,
    Amf3,
}

/// `AmfValue` enumeration.
/// `AmfValue` 枚举。
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum AmfValue {
    Amf0(Amf0Value),
    Amf3(Amf3Value),
}

impl AmfValue {
    /// Decodes the value from the input buffer.
    /// 从输入缓冲区解码值。
    pub fn decode(buf: &[u8], version: AmfVersion) -> Result<(usize, Self), Error> {
        match version {
            AmfVersion::Amf0 => Amf0Value::decode(buf).map(|(n, v)| (n, Self::Amf0(v))),
            AmfVersion::Amf3 => Amf3Value::decode(buf).map(|(n, v)| (n, Self::Amf3(v))),
        }
    }

    /// Encodes the value into the output buffer.
    /// 将值编码到输出缓冲区。
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Self::Amf0(x) => x.encode(buf),
            Self::Amf3(x) => x.encode(buf),
        }
    }

    /// `expect_object_member` function of `AmfValue`.
    /// `AmfValue` 的 `expect_object_member` 函数。
    #[track_caller]
    pub fn expect_object_member(&self, key: &str) -> Result<AmfValueRef<'_>, Error> {
        match self {
            Self::Amf0(Amf0Value::Object { entries, .. }) => entries
                .iter()
                .rfind(|pair| pair.key == key)
                .map(|pair| AmfValueRef::Amf0(&pair.value))
                .ok_or_else(|| Error::invalid_data(format!("missing required key: {key}"))),
            Self::Amf3(Amf3Value::Object { entries, .. }) => entries
                .iter()
                .rfind(|pair| pair.key == key)
                .map(|pair| AmfValueRef::Amf3(&pair.value))
                .ok_or_else(|| Error::invalid_data(format!("missing required key: {key}"))),
            _ => Err(Error::invalid_data("value is not an AMF object")),
        }
    }

    /// `expect_str` function of `AmfValue`.
    /// `AmfValue` 的 `expect_str` 函数。
    #[track_caller]
    pub fn expect_str(&self) -> Result<&str, Error> {
        self.to_ref().expect_str()
    }

    /// `expect_number` function of `AmfValue`.
    /// `AmfValue` 的 `expect_number` 函数。
    #[track_caller]
    pub fn expect_number(&self) -> Result<f64, Error> {
        self.to_ref().expect_number()
    }

    fn to_ref(&self) -> AmfValueRef<'_> {
        match self {
            Self::Amf0(v) => AmfValueRef::Amf0(v),
            Self::Amf3(v) => AmfValueRef::Amf3(v),
        }
    }

    /// `amf0_object` function of `AmfValue`.
    /// `AmfValue` 的 `amf0_object` 函数。
    pub fn amf0_object<'a, I>(entries: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, Amf0Value)>,
    {
        Self::Amf0(Amf0Value::Object {
            class_name: None,
            entries: entries
                .into_iter()
                .map(|(k, v)| Pair {
                    key: k.to_owned(),
                    value: v,
                })
                .collect(),
        })
    }
}

impl From<(AmfVersion, &str)> for AmfValue {
    fn from((version, value): (AmfVersion, &str)) -> Self {
        match version {
            AmfVersion::Amf0 => Self::Amf0(Amf0Value::String(value.to_owned())),
            AmfVersion::Amf3 => Self::Amf3(Amf3Value::String(value.to_owned())),
        }
    }
}

impl From<(AmfVersion, f64)> for AmfValue {
    fn from((version, value): (AmfVersion, f64)) -> Self {
        match version {
            AmfVersion::Amf0 => Self::Amf0(Amf0Value::Number(value)),
            AmfVersion::Amf3 => Self::Amf3(Amf3Value::Double(value)),
        }
    }
}

/// `AmfValueRef` enumeration.
/// `AmfValueRef` 枚举。
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum AmfValueRef<'a> {
    Amf0(&'a Amf0Value),
    Amf3(&'a Amf3Value),
}

impl<'a> AmfValueRef<'a> {
    /// `expect_str` function.
    /// `expect_str` 函数。
    pub fn expect_str(&self) -> Result<&'a str, Error> {
        match self {
            Self::Amf0(Amf0Value::String(s)) => Ok(s),
            Self::Amf3(Amf3Value::String(s)) => Ok(s),
            _ => Err(Error::invalid_data("value is not an AMF string")),
        }
    }

    /// `expect_number` function.
    /// `expect_number` 函数。
    pub fn expect_number(&self) -> Result<f64, Error> {
        match self {
            Self::Amf0(Amf0Value::Number(n)) => Ok(*n),
            Self::Amf3(Amf3Value::Integer(n)) => Ok(*n as f64),
            Self::Amf3(Amf3Value::Double(n)) => Ok(*n),
            _ => Err(Error::invalid_data("value is not an AMF number")),
        }
    }
}

/// `Pair` data structure.
/// `Pair` 数据结构。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pair<K, V> {
    pub key: K,
    pub value: V,
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    use crate::error::ErrorKind;
    use std::path::{Path, PathBuf};

    fn testdata_dir() -> PathBuf {
        const CANDIDATES: &[&str] = &[
            "tests/testdata",
            "../testing/property-tests/tests/testdata",
            "crates/protocols/rtmp/testing/property-tests/tests/testdata",
        ];
        for candidate in CANDIDATES {
            let path = Path::new(candidate);
            if path.is_dir() {
                return path.to_path_buf();
            }
        }
        panic!("unable to locate RTMP testdata directory");
    }

    #[test]
    fn decode_and_encode_amf0_values() {
        for entry in std::fs::read_dir(testdata_dir()).expect("read_dir() error") {
            let entry = entry.expect("read_dir() error");

            // 首先检查是否为目标测试数据
            let test_file_path = entry.path();
            let Some(test_file_name) = test_file_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !test_file_name.starts_with("amf0") || !test_file_name.ends_with(".bin") {
                continue;
            }

            // 将 AMF 二进制数据解码后再编码，
            // 确认结果二进制是否一致
            // （AMF 本身是成熟的格式，没那么复杂，
            //   因此不逐一确认各值的解码结果）
            let original_data = std::fs::read(&test_file_path).expect("read() error");
            let version = AmfVersion::Amf0;

            // 异常系测试数据做特殊处理
            if test_file_name.ends_with("-partial.bin") {
                let err = AmfValue::decode(&original_data, version)
                    .expect_err("AmfValue::decode() success");
                assert_eq!(err.kind, ErrorKind::InsufficientBuffer);
                continue;
            }
            if test_file_name.contains("-bad-") {
                let err = AmfValue::decode(&original_data, version)
                    .expect_err("AmfValue::decode() success");
                assert_eq!(err.kind, ErrorKind::InvalidData);
                continue;
            }
            if test_file_name.contains("-unsupported-") {
                let err = AmfValue::decode(&original_data, version)
                    .expect_err("AmfValue::decode() success");
                assert_eq!(err.kind, ErrorKind::Unsupported);
                continue;
            }

            let (decoded_len, amf_value) =
                AmfValue::decode(&original_data, version).expect("AmfValue::decode() error");
            assert_eq!(decoded_len, original_data.len());

            let mut encoded_data = Vec::new();
            amf_value.encode(&mut encoded_data);

            // 再解码一次，将值与 `amf_value` 进行比较
            //（对于引用类型，`encoded_data` 和 `original_data` 可能不一致）
            let (re_decoded_len, re_decoded_amf_value) = AmfValue::decode(&encoded_data, version)
                .expect("AmfValue::decode() error on re-decode");
            assert_eq!(re_decoded_len, encoded_data.len());

            if let (AmfValue::Amf0(Amf0Value::Number(n0)), AmfValue::Amf0(Amf0Value::Number(n1))) =
                (&amf_value, &re_decoded_amf_value)
            {
                if n0.is_nan() && n1.is_nan() {
                    // NaN 之间无法比较
                    continue;
                }
            }

            assert_eq!(amf_value, re_decoded_amf_value);
        }
    }

    #[test]
    fn decode_and_encode_amf3_values() {
        for entry in std::fs::read_dir(testdata_dir()).expect("read_dir() error") {
            let entry = entry.expect("read_dir() error");

            // 首先检查是否为目标测试数据
            let test_file_path = entry.path();
            let Some(test_file_name) = test_file_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !test_file_name.starts_with("amf3") || !test_file_name.ends_with(".bin") {
                continue;
            }

            // 将 AMF 二进制数据解码后再编码，
            // 确认结果二进制是否一致
            // （AMF 本身是成熟的格式，没那么复杂，
            //   因此不逐一确认各值的解码结果）
            let original_data = std::fs::read(&test_file_path).expect("read() error");
            let version = AmfVersion::Amf3;

            // 异常系测试数据做特殊处理
            if test_file_name.ends_with("-partial.bin") {
                let err = AmfValue::decode(&original_data, version)
                    .expect_err("AmfValue::decode() success");
                assert_eq!(err.kind, ErrorKind::InsufficientBuffer);
                continue;
            }
            if test_file_name.contains("-bad-") {
                let err = AmfValue::decode(&original_data, version)
                    .expect_err("AmfValue::decode() success");
                assert_eq!(err.kind, ErrorKind::InvalidData);
                continue;
            }
            if test_file_name.contains("-unsupported-") {
                let err = AmfValue::decode(&original_data, version)
                    .expect_err("AmfValue::decode() success");
                assert_eq!(err.kind, ErrorKind::Unsupported);
                continue;
            }

            let (decoded_len, amf_value) =
                AmfValue::decode(&original_data, version).expect("AmfValue::decode() error");
            assert_eq!(decoded_len, original_data.len());

            let mut encoded_data = Vec::new();
            amf_value.encode(&mut encoded_data);

            // 再解码一次，将值与 `amf_value` 进行比较
            //（对于引用类型，`encoded_data` 和 `original_data` 可能不一致）
            let (re_decoded_len, re_decoded_amf_value) = AmfValue::decode(&encoded_data, version)
                .expect("AmfValue::decode() error on re-decode");
            assert_eq!(re_decoded_len, encoded_data.len());
            assert_eq!(amf_value, re_decoded_amf_value);
        }
    }
}
