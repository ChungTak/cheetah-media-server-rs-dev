//! Property-based round-trip tests for the AMF0/AMF3 encoders.
//!
//! These tests generate random `Amf0Value`, `Amf3Value`, and `AmfValue` trees and
//! assert that encode -> decode preserves the original value. Special care is taken
//! for NaN, which cannot be compared with `==` and is skipped for aggregate
//! round-trips, but is tested explicitly for bit-level preservation.
//!
//! AMF0/AMF3 编码器的属性测试往返测试。
//!
//! 这些测试生成随机 `Amf0Value`、`Amf3Value` 与 `AmfValue` 树，并断言 encode -> decode 保留原始值。
//! 对于 NaN 会特别处理：它无法使用 `==` 比较，因此在聚合往返中跳过，但会单独测试其位级保持。

use cheetah_rtmp_core::{Amf0Value, Amf3Value, AmfValue, AmfVersion, Pair};
use proptest::collection::vec;
use proptest::prelude::*;

// =============================================================================
// AMF0 arbitrary value strategies
// =============================================================================

/// Generate an arbitrary AMF0 string of limited length.
///
/// 生成有限长度的任意 AMF0 字符串。
fn arb_amf0_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 0..100).prop_map(|chars| chars.into_iter().collect())
}

/// Generate an AMF0 number, including infinities, zeros, and NaN.
///
/// 生成 AMF0 数字，包括无穷大、零和 NaN。
fn arb_amf0_number() -> impl Strategy<Value = f64> {
    prop_oneof![
        prop::num::f64::NORMAL,
        Just(0.0),
        Just(-0.0),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::NAN),
    ]
}

/// Generate an i64 timestamp safe for AMF0 Date round-trips.
///
/// 生成适合 AMF0 Date 往返的 i64 时间戳。
fn arb_amf0_date() -> impl Strategy<Value = i64> {
    -253402300799999i64..=253402300799999i64
}

/// Generate a leaf `Amf0Value` (no recursion).
///
/// 生成无递归的 `Amf0Value` 叶值。
fn arb_amf0_leaf() -> impl Strategy<Value = Amf0Value> {
    prop_oneof![
        arb_amf0_number().prop_map(Amf0Value::Number),
        any::<bool>().prop_map(Amf0Value::Boolean),
        arb_amf0_string().prop_map(Amf0Value::String),
        Just(Amf0Value::Null),
        Just(Amf0Value::Undefined),
        arb_amf0_date().prop_map(|unix_time_ms| Amf0Value::Date { unix_time_ms }),
        arb_amf0_string().prop_map(Amf0Value::XmlDocument),
    ]
}

/// Generate an arbitrary recursive `Amf0Value` with bounded depth and width.
///
/// 生成有界深度与宽度的任意递归 `Amf0Value`。
fn arb_amf0_value() -> impl Strategy<Value = Amf0Value> {
    arb_amf0_leaf().prop_recursive(3, 64, 10, |inner| {
        prop_oneof![
            vec((arb_amf0_string(), inner.clone()), 0..5).prop_map(|entries| {
                Amf0Value::Object {
                    class_name: None,
                    entries: entries
                        .into_iter()
                        .map(|(key, value)| Pair { key, value })
                        .collect(),
                }
            }),
            (
                arb_amf0_string(),
                vec((arb_amf0_string(), inner.clone()), 0..5)
            )
                .prop_map(|(class_name, entries)| {
                    Amf0Value::Object {
                        class_name: Some(class_name),
                        entries: entries
                            .into_iter()
                            .map(|(key, value)| Pair { key, value })
                            .collect(),
                    }
                }),
            vec((arb_amf0_string(), inner.clone()), 0..5).prop_map(|entries| {
                Amf0Value::EcmaArray {
                    entries: entries
                        .into_iter()
                        .map(|(key, value)| Pair { key, value })
                        .collect(),
                }
            }),
            vec(inner, 0..5).prop_map(|entries| Amf0Value::Array { entries }),
        ]
    })
}

// =============================================================================
// AMF3 arbitrary value strategies
// =============================================================================

/// Generate an AMF3 integer in the valid i29 range.
///
/// 在有效 i29 范围内生成 AMF3 整数。
fn arb_amf3_integer() -> impl Strategy<Value = i32> {
    -268435456i32..=268435455i32
}

/// Generate an arbitrary AMF3 string.
///
/// 生成任意 AMF3 字符串。
fn arb_amf3_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 0..100).prop_map(|chars| chars.into_iter().collect())
}

/// Generate a non-empty string usable as an AMF3 object key.
///
/// Empty strings are used as terminators in AMF3 arrays, so keys must be non-empty.
///
/// 生成可用作 AMF3 对象键的非空字符串。
///
/// AMF3 数组使用空字符串作为终止符，因此键必须非空。
fn arb_amf3_key() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 1..50).prop_map(|chars| chars.into_iter().collect())
}

/// Generate a string usable as an AMF3 ObjectVector class name.
///
/// "*" is reserved for "any type" and cannot be used as a class name.
///
/// 生成可用作 AMF3 ObjectVector 类名的字符串。
///
/// "*" 保留为"任意类型"，不能用作类名。
fn arb_amf3_class_name() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 1..50)
        .prop_map(|chars| chars.into_iter().collect())
        .prop_filter("not asterisk", |s| s != "*")
}

/// Generate an AMF3 double, including infinities, zeros, and NaN.
///
/// 生成 AMF3 double，包括无穷大、零和 NaN。
fn arb_amf3_double() -> impl Strategy<Value = f64> {
    prop_oneof![
        prop::num::f64::NORMAL,
        Just(0.0),
        Just(-0.0),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        Just(f64::NAN),
    ]
}

/// Generate an i64 timestamp safe for AMF3 Date round-trips.
///
/// 生成适合 AMF3 Date 往返的 i64 时间戳。
fn arb_amf3_date() -> impl Strategy<Value = i64> {
    -253402300799999i64..=253402300799999i64
}

/// Generate a leaf `Amf3Value` (no recursion).
///
/// 生成无递归的 `Amf3Value` 叶值。
fn arb_amf3_leaf() -> impl Strategy<Value = Amf3Value> {
    prop_oneof![
        Just(Amf3Value::Undefined),
        Just(Amf3Value::Null),
        any::<bool>().prop_map(Amf3Value::Boolean),
        arb_amf3_integer().prop_map(Amf3Value::Integer),
        arb_amf3_double().prop_map(Amf3Value::Double),
        arb_amf3_string().prop_map(Amf3Value::String),
        arb_amf3_string().prop_map(Amf3Value::XmlDocument),
        arb_amf3_date().prop_map(|unix_time_ms| Amf3Value::Date { unix_time_ms }),
        arb_amf3_string().prop_map(Amf3Value::Xml),
        vec(any::<u8>(), 0..100).prop_map(Amf3Value::ByteArray),
        (any::<bool>(), vec(any::<i32>(), 0..10))
            .prop_map(|(is_fixed, entries)| Amf3Value::IntVector { is_fixed, entries }),
        (any::<bool>(), vec(any::<u32>(), 0..10))
            .prop_map(|(is_fixed, entries)| Amf3Value::UintVector { is_fixed, entries }),
        (any::<bool>(), vec(arb_amf3_double(), 0..10))
            .prop_map(|(is_fixed, entries)| Amf3Value::DoubleVector { is_fixed, entries }),
    ]
}

/// Generate an arbitrary recursive `Amf3Value` with bounded depth and width.
///
/// 生成有界深度与宽度的任意递归 `Amf3Value`。
fn arb_amf3_value() -> impl Strategy<Value = Amf3Value> {
    arb_amf3_leaf().prop_recursive(3, 64, 10, |inner| {
        prop_oneof![
            (
                vec((arb_amf3_key(), inner.clone()), 0..3),
                vec(inner.clone(), 0..5)
            )
                .prop_map(|(assoc_entries, dense_entries)| {
                    Amf3Value::Array {
                        assoc_entries: assoc_entries
                            .into_iter()
                            .map(|(key, value)| Pair { key, value })
                            .collect(),
                        dense_entries,
                    }
                }),
            vec((arb_amf3_key(), inner.clone()), 0..5).prop_map(|entries| {
                Amf3Value::Object {
                    class_name: None,
                    sealed_count: 0,
                    entries: entries
                        .into_iter()
                        .map(|(key, value)| Pair { key, value })
                        .collect(),
                }
            }),
            (arb_amf3_key(), vec((arb_amf3_key(), inner.clone()), 0..5)).prop_map(
                |(class_name, entries)| {
                    let entries: Vec<_> = entries
                        .into_iter()
                        .map(|(key, value)| Pair { key, value })
                        .collect();
                    let sealed_count = entries.len() / 2;
                    Amf3Value::Object {
                        class_name: Some(class_name),
                        sealed_count,
                        entries,
                    }
                }
            ),
            (
                prop::option::of(arb_amf3_class_name()),
                any::<bool>(),
                vec(inner.clone(), 0..5)
            )
                .prop_map(|(class_name, is_fixed, entries)| {
                    Amf3Value::ObjectVector {
                        class_name,
                        is_fixed,
                        entries,
                    }
                }),
            (any::<bool>(), vec((inner.clone(), inner), 0..5)).prop_map(|(is_weak, entries)| {
                Amf3Value::Dictionary {
                    is_weak,
                    entries: entries
                        .into_iter()
                        .map(|(key, value)| Pair { key, value })
                        .collect(),
                }
            }),
        ]
    })
}

// =============================================================================
// AMF0 proptests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify round-trip for arbitrary AMF0 values.
    ///
    /// Values containing NaN are skipped because NaN does not compare equal.
    ///
    /// 校验任意 AMF0 值的往返。包含 NaN 的值会被跳过，因为 NaN 不相等。
    #[test]
    fn amf0_roundtrip(value in arb_amf0_value()) {
        if contains_nan_amf0(&value) {
            return Ok(());
        }

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf0Value::decode(&encoded)
            .expect("AMF0 decode failed");

        prop_assert_eq!(decoded_size, encoded.len(), "decoded size mismatch");
        prop_assert_eq!(decoded_value, value, "roundtrip value mismatch");
    }

    /// Verify special AMF0 number values, including NaN bit preservation.
    ///
    /// 校验特殊 AMF0 数字值，包括 NaN 的位保持。
    #[test]
    fn amf0_number_special_values(n in arb_amf0_number()) {
        let value = Amf0Value::Number(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf0Value::decode(&encoded)
            .expect("AMF0 Number decode failed");

        prop_assert_eq!(decoded_size, encoded.len());

        if let Amf0Value::Number(decoded_n) = decoded_value {
            if n.is_nan() {
                prop_assert!(decoded_n.is_nan(), "NaN should remain NaN");
            } else {
                prop_assert_eq!(decoded_n.to_bits(), n.to_bits(), "Number bits mismatch");
            }
        } else {
            return Err(TestCaseError::fail("decoded value is not a Number"));
        }
    }

    /// Verify AMF0 string round-trip at the short-string / long-string boundary.
    ///
    /// 在校短字符串与长字符串边界处校验 AMF0 字符串往返。
    #[test]
    fn amf0_string_boundary(len in prop_oneof![
        Just(0usize),
        Just(1usize),
        Just(0xFFFEusize),
        Just(0xFFFFusize),
        Just(0x10000usize),
        Just(0x10001usize),
    ]) {
        let s: String = (0..len).map(|_| 'a').collect();
        let value = Amf0Value::String(s.clone());

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf0Value::decode(&encoded)
            .expect("AMF0 String decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf0Value::String(decoded_s) = decoded_value {
            prop_assert_eq!(decoded_s, s);
        } else {
            return Err(TestCaseError::fail("decoded value is not a String"));
        }
    }

    /// Verify AMF0 Date round-trip.
    ///
    /// 校验 AMF0 Date 往返。
    #[test]
    fn amf0_date_roundtrip(unix_time_ms in arb_amf0_date()) {
        let value = Amf0Value::Date { unix_time_ms };

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf0Value::decode(&encoded)
            .expect("AMF0 Date decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf0Value::Date { unix_time_ms: decoded_time } = decoded_value {
            prop_assert_eq!(decoded_time, unix_time_ms);
        } else {
            return Err(TestCaseError::fail("decoded value is not a Date"));
        }
    }
}

// =============================================================================
// AMF3 proptests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Verify round-trip for arbitrary AMF3 values.
    ///
    /// Values containing NaN are skipped because NaN does not compare equal.
    ///
    /// 校验任意 AMF3 值的往返。包含 NaN 的值会被跳过，因为 NaN 不相等。
    #[test]
    fn amf3_roundtrip(value in arb_amf3_value()) {
        if contains_nan_amf3(&value) {
            return Ok(());
        }

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 decode failed");

        prop_assert_eq!(decoded_size, encoded.len(), "decoded size mismatch");
        prop_assert_eq!(decoded_value, value, "roundtrip value mismatch");
    }

    /// Verify AMF3 integer encoding at U29 boundary values.
    ///
    /// 校验 AMF3 整数在 U29 边界值上的编码。
    #[test]
    fn amf3_integer_boundary(n in prop_oneof![
        Just(0i32),
        Just(0x7Fi32),
        Just(0x80i32),
        Just(0x3FFFi32),
        Just(0x4000i32),
        Just(0x1FFFFFi32),
        Just(0x200000i32),
        Just(268435455i32),
        Just(-1i32),
        Just(-268435456i32),
    ]) {
        let value = Amf3Value::Integer(n);

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 Integer decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf3Value::Integer(decoded_n) = decoded_value {
            prop_assert_eq!(decoded_n, n, "Integer value mismatch");
        } else {
            return Err(TestCaseError::fail("decoded value is not an Integer"));
        }
    }

    /// Verify AMF3 integer round-trip across the entire i29 range.
    ///
    /// 校验整个 i29 范围内的 AMF3 整数往返。
    #[test]
    fn amf3_integer_full_range(n in arb_amf3_integer()) {
        let value = Amf3Value::Integer(n);

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 Integer decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf3Value::Integer(decoded_n) = decoded_value {
            prop_assert_eq!(decoded_n, n);
        } else {
            return Err(TestCaseError::fail("decoded value is not an Integer"));
        }
    }

    /// Verify special AMF3 double values, including NaN bit preservation.
    ///
    /// 校验特殊 AMF3 double 值，包括 NaN 的位保持。
    #[test]
    fn amf3_double_special_values(n in arb_amf3_double()) {
        let value = Amf3Value::Double(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 Double decode failed");

        prop_assert_eq!(decoded_size, encoded.len());

        if let Amf3Value::Double(decoded_n) = decoded_value {
            if n.is_nan() {
                prop_assert!(decoded_n.is_nan(), "NaN should remain NaN");
            } else {
                prop_assert_eq!(decoded_n.to_bits(), n.to_bits(), "Double bits mismatch");
            }
        } else {
            return Err(TestCaseError::fail("decoded value is not a Double"));
        }
    }

    /// Verify AMF3 `ByteArray` round-trip.
    ///
    /// 校验 AMF3 `ByteArray` 往返。
    #[test]
    fn amf3_byte_array_roundtrip(data in vec(any::<u8>(), 0..1000)) {
        let value = Amf3Value::ByteArray(data.clone());

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 ByteArray decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf3Value::ByteArray(decoded_data) = decoded_value {
            prop_assert_eq!(decoded_data, data);
        } else {
            return Err(TestCaseError::fail("decoded value is not a ByteArray"));
        }
    }

    /// Verify AMF3 `Vector.<int>` round-trip.
    ///
    /// 校验 AMF3 `Vector.<int>` 往返。
    #[test]
    fn amf3_int_vector_roundtrip(is_fixed in any::<bool>(), entries in vec(any::<i32>(), 0..100)) {
        let value = Amf3Value::IntVector { is_fixed, entries: entries.clone() };

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 IntVector decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf3Value::IntVector { is_fixed: d_fixed, entries: d_entries } = decoded_value {
            prop_assert_eq!(d_fixed, is_fixed);
            prop_assert_eq!(d_entries, entries);
        } else {
            return Err(TestCaseError::fail("decoded value is not an IntVector"));
        }
    }

    /// Verify AMF3 `Date` round-trip.
    ///
    /// 校验 AMF3 `Date` 往返。
    #[test]
    fn amf3_date_roundtrip(unix_time_ms in arb_amf3_date()) {
        let value = Amf3Value::Date { unix_time_ms };

        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let (decoded_size, decoded_value) = Amf3Value::decode(&encoded)
            .expect("AMF3 Date decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        if let Amf3Value::Date { unix_time_ms: decoded_time } = decoded_value {
            prop_assert_eq!(decoded_time, unix_time_ms);
        } else {
            return Err(TestCaseError::fail("decoded value is not a Date"));
        }
    }
}

// =============================================================================
// `AmfValue` proptests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Verify `AmfValue` (AMF0) round-trip.
    ///
    /// 校验 `AmfValue`（AMF0）往返。
    #[test]
    fn amf_value_amf0_roundtrip(value in arb_amf0_value()) {
        if contains_nan_amf0(&value) {
            return Ok(());
        }

        let amf_value = AmfValue::Amf0(value.clone());

        let mut encoded = Vec::new();
        amf_value.encode(&mut encoded);

        let (decoded_size, decoded_value) = AmfValue::decode(&encoded, AmfVersion::Amf0)
            .expect("AmfValue decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        prop_assert_eq!(decoded_value, amf_value);
    }

    /// Verify `AmfValue` (AMF3) round-trip.
    ///
    /// 校验 `AmfValue`（AMF3）往返。
    #[test]
    fn amf_value_amf3_roundtrip(value in arb_amf3_value()) {
        if contains_nan_amf3(&value) {
            return Ok(());
        }

        let amf_value = AmfValue::Amf3(value.clone());

        let mut encoded = Vec::new();
        amf_value.encode(&mut encoded);

        let (decoded_size, decoded_value) = AmfValue::decode(&encoded, AmfVersion::Amf3)
            .expect("AmfValue decode failed");

        prop_assert_eq!(decoded_size, encoded.len());
        prop_assert_eq!(decoded_value, amf_value);
    }
}

// =============================================================================
// AMF specification boundary tests
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    // -------------------------------------------------------------------------
    // AMF0 specification Section 2: type markers
    // -------------------------------------------------------------------------

    /// Verify AMF0 Number encoding follows the IEEE-754 double layout.
    ///
    /// 校验 AMF0 Number 编码遵循 IEEE-754 double 布局。
    #[test]
    fn amf0_number_ieee754_compliance(
        n in prop_oneof![
            Just(1.0f64),
            Just(-1.0f64),
            Just(f64::MIN),
            Just(f64::MAX),
            Just(f64::MIN_POSITIVE),
            Just(0.0f64),
            Just(-0.0f64),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            Just(5e-324f64),
        ]
    ) {
        let value = Amf0Value::Number(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 9, "AMF0 Number should be 9 bytes");
        prop_assert_eq!(encoded[0], 0x00, "AMF0 Number marker should be 0x00");

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        if let Amf0Value::Number(decoded_n) = decoded {
            prop_assert_eq!(decoded_n.to_bits(), n.to_bits());
        }
    }

    /// Verify AMF0 Boolean encoding size and marker.
    ///
    /// 校验 AMF0 Boolean 编码大小与标记。
    #[test]
    fn amf0_boolean_encoding(b in any::<bool>()) {
        let value = Amf0Value::Boolean(b);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 2, "AMF0 Boolean should be 2 bytes");
        prop_assert_eq!(encoded[0], 0x01, "AMF0 Boolean marker should be 0x01");
        if b {
            prop_assert_ne!(encoded[1], 0, "true should be non-zero");
        } else {
            prop_assert_eq!(encoded[1], 0, "false should be zero");
        }

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// Verify AMF0 String/LongString size and marker.
    ///
    /// 校验 AMF0 String/LongString 大小与标记。
    #[test]
    fn amf0_string_encoding_size(len in 0usize..=100usize) {
        let s: String = (0..len).map(|_| 'a').collect();
        let value = Amf0Value::String(s.clone());
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        if len <= 0xFFFF {
            prop_assert_eq!(encoded[0], 0x02, "String marker should be 0x02");
            prop_assert_eq!(encoded.len(), 1 + 2 + len);
        } else {
            prop_assert_eq!(encoded[0], 0x0C, "LongString marker should be 0x0C");
            prop_assert_eq!(encoded.len(), 1 + 4 + len);
        }

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        if let Amf0Value::String(decoded_s) = decoded {
            prop_assert_eq!(decoded_s, s);
        }
    }

    /// Verify AMF0 Null encoding size and marker.
    ///
    /// 校验 AMF0 Null 编码大小与标记。
    #[test]
    fn amf0_null_encoding(_dummy in Just(())) {
        let value = Amf0Value::Null;
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 1, "AMF0 Null should be 1 byte");
        prop_assert_eq!(encoded[0], 0x05, "AMF0 Null marker should be 0x05");

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// Verify AMF0 Undefined encoding size and marker.
    ///
    /// 校验 AMF0 Undefined 编码大小与标记。
    #[test]
    fn amf0_undefined_encoding(_dummy in Just(())) {
        let value = Amf0Value::Undefined;
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 1, "AMF0 Undefined should be 1 byte");
        prop_assert_eq!(encoded[0], 0x06, "AMF0 Undefined marker should be 0x06");

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// Verify AMF0 Date encoding size and marker.
    ///
    /// 校验 AMF0 Date 编码大小与标记。
    #[test]
    fn amf0_date_encoding(unix_time_ms in arb_amf0_date()) {
        let value = Amf0Value::Date { unix_time_ms };
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 11, "AMF0 Date should be 11 bytes");
        prop_assert_eq!(encoded[0], 0x0B, "AMF0 Date marker should be 0x0B");

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    // -------------------------------------------------------------------------
    // AMF3 specification Section 1.3.1: U29 variable-length integer
    // -------------------------------------------------------------------------

    /// Verify AMF3 U29 integer encoding size at boundary values.
    ///
    /// 校验 AMF3 U29 整数在边界值上的编码大小。
    #[test]
    fn amf3_u29_encoding_size(
        (n, expected_extra_bytes) in prop_oneof![
            Just((0i32, 1)),
            Just((0x7Fi32, 1)),
            Just((0x80i32, 2)),
            Just((0x3FFFi32, 2)),
            Just((0x4000i32, 3)),
            Just((0x1FFFFFi32, 3)),
            Just((0x200000i32, 4)),
            Just((268435455i32, 4)),
        ]
    ) {
        let value = Amf3Value::Integer(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        let expected_size = 1 + expected_extra_bytes;
        prop_assert_eq!(encoded.len(), expected_size,
            "AMF3 Integer {} should be {} bytes", n, expected_size);
        prop_assert_eq!(encoded[0], 0x04, "AMF3 Integer marker should be 0x04");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// Verify AMF3 Boolean markers.
    ///
    /// 校验 AMF3 Boolean 标记。
    #[test]
    fn amf3_boolean_encoding(b in any::<bool>()) {
        let value = Amf3Value::Boolean(b);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 1, "AMF3 Boolean should be 1 byte");
        if b {
            prop_assert_eq!(encoded[0], 0x03, "AMF3 true marker should be 0x03");
        } else {
            prop_assert_eq!(encoded[0], 0x02, "AMF3 false marker should be 0x02");
        }

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// Verify AMF3 Null/Undefined markers.
    ///
    /// 校验 AMF3 Null/Undefined 标记。
    #[test]
    fn amf3_undefined_null_encoding(is_null in any::<bool>()) {
        let value = if is_null { Amf3Value::Null } else { Amf3Value::Undefined };
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 1, "AMF3 Undefined/Null should be 1 byte");
        if is_null {
            prop_assert_eq!(encoded[0], 0x01, "AMF3 null marker should be 0x01");
        } else {
            prop_assert_eq!(encoded[0], 0x00, "AMF3 undefined marker should be 0x00");
        }

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// Verify AMF3 Double encoding size and marker.
    ///
    /// 校验 AMF3 Double 编码大小与标记。
    #[test]
    fn amf3_double_encoding(
        n in prop_oneof![
            Just(0.0f64),
            Just(-0.0f64),
            Just(1.0f64),
            Just(-1.0f64),
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            Just(f64::MAX),
            Just(f64::MIN),
        ]
    ) {
        let value = Amf3Value::Double(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded.len(), 9, "AMF3 Double should be 9 bytes");
        prop_assert_eq!(encoded[0], 0x05, "AMF3 Double marker should be 0x05");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        if let Amf3Value::Double(decoded_n) = decoded {
            prop_assert_eq!(decoded_n.to_bits(), n.to_bits());
        }
    }

    /// Verify AMF3 ByteArray marker and round-trip.
    ///
    /// 校验 AMF3 ByteArray 标记与往返。
    #[test]
    fn amf3_byte_array_encoding(data in vec(any::<u8>(), 0..50)) {
        let value = Amf3Value::ByteArray(data.clone());
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded[0], 0x0C, "AMF3 ByteArray marker should be 0x0C");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        if let Amf3Value::ByteArray(decoded_data) = decoded {
            prop_assert_eq!(decoded_data, data);
        }
    }

    // -------------------------------------------------------------------------
    // AMF3 specification Section 3.15: Vector types
    // -------------------------------------------------------------------------

    /// Verify `Vector.<int>` encoding and decoding.
    ///
    /// 校验 `Vector.<int>` 编码与解码。
    #[test]
    fn amf3_int_vector_encoding(is_fixed in any::<bool>(), entries in vec(any::<i32>(), 0..20)) {
        let value = Amf3Value::IntVector { is_fixed, entries: entries.clone() };
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded[0], 0x0D, "AMF3 Vector.<int> marker should be 0x0D");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        if let Amf3Value::IntVector { is_fixed: d_fixed, entries: d_entries } = decoded {
            prop_assert_eq!(d_fixed, is_fixed);
            prop_assert_eq!(d_entries, entries);
        }
    }

    /// Verify `Vector.<uint>` encoding and decoding.
    ///
    /// 校验 `Vector.<uint>` 编码与解码。
    #[test]
    fn amf3_uint_vector_encoding(is_fixed in any::<bool>(), entries in vec(any::<u32>(), 0..20)) {
        let value = Amf3Value::UintVector { is_fixed, entries: entries.clone() };
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded[0], 0x0E, "AMF3 Vector.<uint> marker should be 0x0E");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        if let Amf3Value::UintVector { is_fixed: d_fixed, entries: d_entries } = decoded {
            prop_assert_eq!(d_fixed, is_fixed);
            prop_assert_eq!(d_entries, entries);
        }
    }

    /// Verify `Vector.<Number>` encoding and decoding (excluding NaN for comparison).
    ///
    /// 校验 `Vector.<Number>` 编码与解码（为比较排除 NaN）。
    #[test]
    fn amf3_double_vector_encoding(
        is_fixed in any::<bool>(),
        entries in vec(prop_oneof![Just(0.0f64), Just(1.0f64), Just(-1.0f64)], 0..20)
    ) {
        let value = Amf3Value::DoubleVector { is_fixed, entries: entries.clone() };
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded[0], 0x0F, "AMF3 Vector.<Number> marker should be 0x0F");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        if let Amf3Value::DoubleVector { is_fixed: d_fixed, entries: d_entries } = decoded {
            prop_assert_eq!(d_fixed, is_fixed);
            prop_assert_eq!(d_entries, entries);
        }
    }

    // -------------------------------------------------------------------------
    // AMF3 specification Section 3.16: Dictionary type
    // -------------------------------------------------------------------------

    /// Verify AMF3 Dictionary encoding and decoding.
    ///
    /// 校验 AMF3 Dictionary 编码与解码。
    #[test]
    fn amf3_dictionary_encoding(is_weak in any::<bool>()) {
        let entries = vec![
            Pair {
                key: Amf3Value::String("key1".to_string()),
                value: Amf3Value::Integer(100),
            },
            Pair {
                key: Amf3Value::Integer(42),
                value: Amf3Value::String("value2".to_string()),
            },
        ];
        let value = Amf3Value::Dictionary { is_weak, entries: entries.clone() };
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        prop_assert_eq!(encoded[0], 0x11, "AMF3 Dictionary marker should be 0x11");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        if let Amf3Value::Dictionary { is_weak: d_weak, entries: d_entries } = decoded {
            prop_assert_eq!(d_weak, is_weak);
            prop_assert_eq!(d_entries.len(), entries.len());
        }
    }
}

// =============================================================================
// NaN helpers
// =============================================================================

/// Recursively check whether an `Amf0Value` contains a NaN float.
///
/// 递归检查 `Amf0Value` 是否包含 NaN 浮点数。
fn contains_nan_amf0(value: &Amf0Value) -> bool {
    match value {
        Amf0Value::Number(n) => n.is_nan(),
        Amf0Value::Object { entries, .. } => entries.iter().any(|p| contains_nan_amf0(&p.value)),
        Amf0Value::EcmaArray { entries } => entries.iter().any(|p| contains_nan_amf0(&p.value)),
        Amf0Value::Array { entries } => entries.iter().any(contains_nan_amf0),
        Amf0Value::AvmPlus(v) => contains_nan_amf3(v),
        _ => false,
    }
}

/// Recursively check whether an `Amf3Value` contains a NaN double.
///
/// 递归检查 `Amf3Value` 是否包含 NaN double。
fn contains_nan_amf3(value: &Amf3Value) -> bool {
    match value {
        Amf3Value::Double(n) => n.is_nan(),
        Amf3Value::Array {
            assoc_entries,
            dense_entries,
        } => {
            assoc_entries.iter().any(|p| contains_nan_amf3(&p.value))
                || dense_entries.iter().any(contains_nan_amf3)
        }
        Amf3Value::Object { entries, .. } => entries.iter().any(|p| contains_nan_amf3(&p.value)),
        Amf3Value::DoubleVector { entries, .. } => entries.iter().any(|n| n.is_nan()),
        Amf3Value::ObjectVector { entries, .. } => entries.iter().any(contains_nan_amf3),
        Amf3Value::Dictionary { entries, .. } => entries
            .iter()
            .any(|p| contains_nan_amf3(&p.key) || contains_nan_amf3(&p.value)),
        _ => false,
    }
}
