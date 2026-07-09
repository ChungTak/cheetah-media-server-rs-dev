//! Property tests covering invariants documented in the WebRTC plan.

use cheetah_webrtc_core::{preprocess_remote_sdp, SdpCompatReport};
use proptest::prelude::*;

proptest! {
    /// The preprocessor is idempotent: running it on its own output
    /// produces an empty `SdpCompatReport`.
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
    #[test]
    fn preprocess_does_not_panic(input in any::<String>()) {
        let _ = preprocess_remote_sdp(&input);
    }

    /// Output always ends with `\r\n` when the input is non-empty.
    #[test]
    fn preprocess_always_terminates_with_crlf(input in r"v=0[\r\n][a-zA-Z0-9 \t\r\n=:/.\-]{0,200}") {
        let (out, _) = preprocess_remote_sdp(&input);
        prop_assert!(out.ends_with("\r\n"));
    }
}
