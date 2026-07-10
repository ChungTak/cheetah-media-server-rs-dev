//! Property tests covering invariants documented in the WebRTC plan.
//!
//! WebRTC 计划中记录的不变量属性测试。

use cheetah_webrtc_core::{preprocess_remote_sdp, SdpCompatReport};
use proptest::prelude::*;

proptest! {
    /// The preprocessor is idempotent: running it on its own output
    /// produces an empty `SdpCompatReport`.
    ///
    /// 预处理器是幂等的：在其输出上再次运行产生空的 `SdpCompatReport`。
    #[test]
    fn preprocess_is_idempotent(
        input in r"v=0\r?\n[a-zA-Z0-9 \t\r\n=:/.\-]{0,200}"
    ) {
        let (first, _) = preprocess_remote_sdp(&input);
        let (second, second_report) = preprocess_remote_sdp(&first);
        prop_assert_eq!(second, first);
        prop_assert_eq!(second_report, SdpCompatReport::default());
    }

    /// The preprocessor never panics on arbitrary inputs.
    ///
    /// 预处理器对任意输入不 panic。
    #[test]
    fn preprocess_does_not_panic(input in any::<String>()) {
        let _ = preprocess_remote_sdp(&input);
    }

    /// Output always ends with `\r\n` when the input is non-empty.
    ///
    /// 输入非空时输出始终以 `\r\n` 结尾。
    #[test]
    fn preprocess_always_terminates_with_crlf(input in r"v=0[\r\n][a-zA-Z0-9 \t\r\n=:/.\-]{0,200}") {
        let (out, _) = preprocess_remote_sdp(&input);
        prop_assert!(out.ends_with("\r\n"));
    }
}
