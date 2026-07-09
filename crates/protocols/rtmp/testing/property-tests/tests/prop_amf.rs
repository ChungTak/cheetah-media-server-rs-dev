//! AMF0/AMF3 的 Property-Based Testing
//!
//! 进行 AMF 编码/解码的 roundtrip 测试。

use cheetah_rtmp_core::{Amf0Value, Amf3Value, AmfValue, AmfVersion, Pair};
use proptest::collection::vec;
use proptest::prelude::*;

// =============================================================================
// AMF0 Arbitrary 值生成策略
// =============================================================================

/// 生成 AMF0 String 的策略（有长度限制）
fn arb_amf0_string() -> impl Strategy<Value = String> {
    // AMF0 通常字符串最多 0xFFFF (65535) 字节
    prop::collection::vec(prop::char::any(), 0..100).prop_map(|chars| chars.into_iter().collect())
}

/// 生成 AMF0 Number 的策略（仅有效 f64）
fn arb_amf0_number() -> impl Strategy<Value = f64> {
    prop_oneof![
        // 通常的有限数值
        prop::num::f64::NORMAL,
        // 特殊值
        Just(0.0),
        Just(-0.0),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        // NaN 无法在 roundtrip 中比较，但编码/解码本身是可行的
        Just(f64::NAN),
    ]
}

/// 生成 AMF0 Date 用的 i64 (unix_time_ms) 的策略
fn arb_amf0_date() -> impl Strategy<Value = i64> {
    // 与 f64 往返转换安全的有限范围
    -253402300799999i64..=253402300799999i64
}

/// 生成 AMF0 叶值（无递归）的策略
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

/// 生成 AMF0 值的策略（有递归、带深度限制）
fn arb_amf0_value() -> impl Strategy<Value = Amf0Value> {
    arb_amf0_leaf().prop_recursive(
        3,  // 最大深度
        64, // 最大节点数
        10, // 每层最大元素数
        |inner| {
            prop_oneof![
                // Object (匿名)
                vec((arb_amf0_string(), inner.clone()), 0..5).prop_map(|entries| {
                    Amf0Value::Object {
                        class_name: None,
                        entries: entries
                            .into_iter()
                            .map(|(key, value)| Pair { key, value })
                            .collect(),
                    }
                }),
                // Object (具名)
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
                // EcmaArray
                vec((arb_amf0_string(), inner.clone()), 0..5).prop_map(|entries| {
                    Amf0Value::EcmaArray {
                        entries: entries
                            .into_iter()
                            .map(|(key, value)| Pair { key, value })
                            .collect(),
                    }
                }),
                // StrictArray
                vec(inner, 0..5).prop_map(|entries| Amf0Value::Array { entries }),
            ]
        },
    )
}

// =============================================================================
// AMF3 Arbitrary 值生成策略
// =============================================================================

/// 生成 AMF3 Integer 的策略（i29 范围）
fn arb_amf3_integer() -> impl Strategy<Value = i32> {
    // AMF3 Integer 是 29-bit signed: -268435456 to 268435455
    -268435456i32..=268435455i32
}

/// 生成 AMF3 String 的策略
fn arb_amf3_string() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 0..100).prop_map(|chars| chars.into_iter().collect())
}

/// 生成可用作 AMF3 对象键的 String 的策略
/// （AMF3 中空字符串被用作终止符，因此不能作为键使用）
fn arb_amf3_key() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 1..50).prop_map(|chars| chars.into_iter().collect())
}

/// 生成可用作 AMF3 ObjectVector 类名的 String 的策略
/// （AMF3 中 "*" 意味着"任意类型"，因此不能作为类名使用）
fn arb_amf3_class_name() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::char::any(), 1..50)
        .prop_map(|chars| chars.into_iter().collect())
        .prop_filter("not asterisk", |s| s != "*")
}

/// 生成 AMF3 Double 的策略
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

/// 生成 AMF3 Date 用的 i64 (unix_time_ms) 的策略
fn arb_amf3_date() -> impl Strategy<Value = i64> {
    -253402300799999i64..=253402300799999i64
}

/// 生成 AMF3 叶值（无递归）的策略
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
        // IntVector
        (any::<bool>(), vec(any::<i32>(), 0..10))
            .prop_map(|(is_fixed, entries)| Amf3Value::IntVector { is_fixed, entries }),
        // UintVector
        (any::<bool>(), vec(any::<u32>(), 0..10))
            .prop_map(|(is_fixed, entries)| Amf3Value::UintVector { is_fixed, entries }),
        // DoubleVector
        (any::<bool>(), vec(arb_amf3_double(), 0..10))
            .prop_map(|(is_fixed, entries)| Amf3Value::DoubleVector { is_fixed, entries }),
    ]
}

/// 生成 AMF3 值的策略（有递归、带深度限制）
fn arb_amf3_value() -> impl Strategy<Value = Amf3Value> {
    arb_amf3_leaf().prop_recursive(
        3,  // 最大深度
        64, // 最大节点数
        10, // 每层最大元素数
        |inner| {
            prop_oneof![
                // Array (assoc 键不可为空字符串)
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
                // Object (匿名、dynamic) - 键不可为空字符串
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
                // Object (具名、sealed + dynamic) - 键不可为空字符串
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
                // ObjectVector (class_name "*" 意味着"任意类型"，因此不可使用)
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
                // Dictionary
                (any::<bool>(), vec((inner.clone(), inner), 0..5)).prop_map(
                    |(is_weak, entries)| {
                        Amf3Value::Dictionary {
                            is_weak,
                            entries: entries
                                .into_iter()
                                .map(|(key, value)| Pair { key, value })
                                .collect(),
                        }
                    }
                ),
            ]
        },
    )
}

// =============================================================================
// AMF0 Proptest
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// AMF0 值的 roundtrip 测试: encode → decode → 与原值一致
    #[test]
    fn amf0_roundtrip(value in arb_amf0_value()) {
        // 包含 NaN 的值无法比较，因此跳过
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

    /// AMF0 Number 的特殊值测试
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

    /// AMF0 String 的边界值测试（短字符串和长字符串）
    #[test]
    fn amf0_string_boundary(len in prop_oneof![
        Just(0usize),
        Just(1usize),
        Just(0xFFFEusize),
        Just(0xFFFFusize),
        Just(0x10000usize),  // LongString 边界
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

    /// AMF0 Date 的 roundtrip 测试
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
// AMF3 Proptest
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// AMF3 值的 roundtrip 测试: encode → decode → 与原值一致
    #[test]
    fn amf3_roundtrip(value in arb_amf3_value()) {
        // 包含 NaN 的值无法比较，因此跳过
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

    /// AMF3 Integer 的边界值测试（U29 编码）
    #[test]
    fn amf3_integer_boundary(n in prop_oneof![
            // 正的边界值
        Just(0i32),
        Just(0x7Fi32),       // 1 字节上限
        Just(0x80i32),
        Just(0x3FFFi32),     // 2 字节上限
        Just(0x4000i32),
        Just(0x1FFFFFi32),   // 3 字节上限
        Just(0x200000i32),
            Just(268435455i32),  // i29 最大值

            // 负的边界值
        Just(-1i32),
            Just(-268435456i32), // i29 最小值
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

    /// AMF3 Integer 的完整范围测试
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

    /// AMF3 Double 的特殊值测试
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

    /// AMF3 ByteArray 的 roundtrip 测试
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

    /// AMF3 Vector 系列的 roundtrip 测试
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

    /// AMF3 Date 的 roundtrip 测试
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
// AmfValue Proptest
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// AmfValue (AMF0) 的 roundtrip 测试
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

    /// AmfValue (AMF3) 的 roundtrip 测试
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
// 基于 AMF 规范的边界值测试
// =============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    // -------------------------------------------------------------------------
    // AMF0 规范 Section 2: Type Markers 的测试
    // -------------------------------------------------------------------------

    /// AMF0 规范 Section 2.2: Number Type
    /// IEEE-754 double precision floating point (8 bytes)
    #[test]
    fn amf0_number_ieee754_compliance(
        n in prop_oneof![
            // 规格化数
            Just(1.0f64),
            Just(-1.0f64),
            Just(f64::MIN),
            Just(f64::MAX),
            Just(f64::MIN_POSITIVE),
            // 零
            Just(0.0f64),
            Just(-0.0f64),
            // 无穷大
            Just(f64::INFINITY),
            Just(f64::NEG_INFINITY),
            // 非规格化数
            Just(5e-324f64),  // 接近最小正非规格化数的值
        ]
    ) {
        let value = Amf0Value::Number(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        // Number: marker (1 byte) + DOUBLE (8 bytes) = 9 bytes
        prop_assert_eq!(encoded.len(), 9, "AMF0 Number should be 9 bytes");
        prop_assert_eq!(encoded[0], 0x00, "AMF0 Number marker should be 0x00");

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        if let Amf0Value::Number(decoded_n) = decoded {
            prop_assert_eq!(decoded_n.to_bits(), n.to_bits());
        }
    }

    /// AMF0 规范 Section 2.3: Boolean Type
    /// marker (1 byte) + U8 (1 byte)
    #[test]
    fn amf0_boolean_encoding(b in any::<bool>()) {
        let value = Amf0Value::Boolean(b);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        // Boolean: marker (1 byte) + value (1 byte) = 2 bytes
        prop_assert_eq!(encoded.len(), 2, "AMF0 Boolean should be 2 bytes");
        prop_assert_eq!(encoded[0], 0x01, "AMF0 Boolean marker should be 0x01");
        // 规范: 0 is false, <> 0 is true
        if b {
            prop_assert_ne!(encoded[1], 0, "true should be non-zero");
        } else {
            prop_assert_eq!(encoded[1], 0, "false should be zero");
        }

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// AMF0 规范 Section 2.4/2.14: String/LongString Type
    /// String: marker + U16 length + UTF-8 chars (最大 65535 字节)
    /// LongString: marker + U32 length + UTF-8 chars (最大 4GB)
    #[test]
    fn amf0_string_encoding_size(len in 0usize..=100usize) {
        let s: String = (0..len).map(|_| 'a').collect();
        let value = Amf0Value::String(s.clone());
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        if len <= 0xFFFF {
            // String: marker (1) + U16 length (2) + chars
            prop_assert_eq!(encoded[0], 0x02, "String marker should be 0x02");
            prop_assert_eq!(encoded.len(), 1 + 2 + len);
        } else {
            // LongString: marker (1) + U32 length (4) + chars
            prop_assert_eq!(encoded[0], 0x0C, "LongString marker should be 0x0C");
            prop_assert_eq!(encoded.len(), 1 + 4 + len);
        }

        let (_, decoded) = Amf0Value::decode(&encoded).expect("decode failed");
        if let Amf0Value::String(decoded_s) = decoded {
            prop_assert_eq!(decoded_s, s);
        }
    }

    /// AMF0 规范 Section 2.7: null Type
    /// marker only (1 byte)
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

    /// AMF0 规范 Section 2.8: undefined Type
    /// marker only (1 byte)
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

    /// AMF0 规范 Section 2.13: Date Type
    /// marker (1) + DOUBLE (8) + S16 timezone (2) = 11 bytes
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
    // AMF3 规范 Section 1.3.1: U29 Variable Length Integer 的测试
    // -------------------------------------------------------------------------

    /// AMF3 U29 编码的字节数边界测试
    /// 规范: 0x00-0x7F (1 byte), 0x80-0x3FFF (2 bytes),
    ///       0x4000-0x1FFFFF (3 bytes), 0x200000-0x3FFFFFFF (4 bytes)
    #[test]
    fn amf3_u29_encoding_size(
        (n, expected_extra_bytes) in prop_oneof![
            // 1 byte encoding: 0x00 - 0x7F
            Just((0i32, 1)),
            Just((0x7Fi32, 1)),
            // 2 byte encoding: 0x80 - 0x3FFF
            Just((0x80i32, 2)),
            Just((0x3FFFi32, 2)),
            // 3 byte encoding: 0x4000 - 0x1FFFFF
            Just((0x4000i32, 3)),
            Just((0x1FFFFFi32, 3)),
            // 4 byte encoding: 0x200000 - 0x0FFFFFFF (i29 max positive)
            Just((0x200000i32, 4)),
            Just((268435455i32, 4)),  // 0x0FFFFFFF
        ]
    ) {
        let value = Amf3Value::Integer(n);
        let mut encoded = Vec::new();
        value.encode(&mut encoded);

        // Integer: marker (1) + U29 (variable)
        let expected_size = 1 + expected_extra_bytes;
        prop_assert_eq!(encoded.len(), expected_size,
            "AMF3 Integer {} should be {} bytes", n, expected_size);
        prop_assert_eq!(encoded[0], 0x04, "AMF3 Integer marker should be 0x04");

        let (_, decoded) = Amf3Value::decode(&encoded).expect("decode failed");
        prop_assert_eq!(decoded, value);
    }

    /// AMF3 规范 Section 3.4/3.5: Boolean Types (true/false)
    /// 仅有独立的标记 (1 byte each)
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

    /// AMF3 规范 Section 3.2/3.3: undefined/null Types
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

    /// AMF3 规范 Section 3.7: Double Type
    /// marker (1) + IEEE-754 double (8) = 9 bytes
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

    /// AMF3 规范 Section 3.14: ByteArray Type
    /// marker (1) + U29B-value + data
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
    // AMF3 规范 Section 3.15: Vector Types 的测试
    // -------------------------------------------------------------------------

    /// AMF3 Vector.<int> 测试
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

    /// AMF3 Vector.<uint> 测试
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

    /// AMF3 Vector.<Number> 测试 (排除 NaN)
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
    // AMF3 规范 Section 3.16: Dictionary Type 的测试
    // -------------------------------------------------------------------------

    /// AMF3 Dictionary 测试
    #[test]
    fn amf3_dictionary_encoding(is_weak in any::<bool>()) {
        // 简单的 Dictionary (键值对)
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
// 辅助函数
// =============================================================================

/// 检查 AMF0 值是否包含 NaN
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

/// 检查 AMF3 值是否包含 NaN
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
