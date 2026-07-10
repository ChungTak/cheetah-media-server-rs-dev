//! HLS CUE/Marker support (SCTE-35 style ad insertion markers).
//!
//! HLS CUE/标记支持（SCTE-35 风格广告插入标记）。

/// A CUE marker event to be inserted into the HLS playlist.
///
/// 要插入 HLS 播放列表的 CUE 标记事件。
#[derive(Debug, Clone)]
pub enum CueMarker {
    /// Start of an ad break. Duration is given in seconds.
    ///
    /// 广告开始。duration 以秒为单位。
    CueOut { duration_secs: f64 },
    /// End of an ad break (return to main content).
    ///
    /// 广告结束（返回主内容）。
    CueIn,
}

/// Format CUE markers as HLS playlist tags.
///
/// Outputs `#EXT-X-CUE-OUT:duration` or `#EXT-X-CUE-IN`.
///
/// 将 CUE 标记格式化为 HLS 播放列表标签。
/// 输出 `#EXT-X-CUE-OUT:duration` 或 `#EXT-X-CUE-IN`。
pub fn format_cue_tags(markers: &[CueMarker]) -> String {
    let mut out = String::new();
    for m in markers {
        match m {
            CueMarker::CueOut { duration_secs } => {
                out.push_str(&format!("#EXT-X-CUE-OUT:{duration_secs:.1}\n"));
            }
            CueMarker::CueIn => {
                out.push_str("#EXT-X-CUE-IN\n");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_out_tag() {
        let tags = format_cue_tags(&[CueMarker::CueOut {
            duration_secs: 30.0,
        }]);
        assert_eq!(tags, "#EXT-X-CUE-OUT:30.0\n");
    }

    #[test]
    fn cue_in_tag() {
        let tags = format_cue_tags(&[CueMarker::CueIn]);
        assert_eq!(tags, "#EXT-X-CUE-IN\n");
    }

    #[test]
    fn multiple_markers() {
        let tags = format_cue_tags(&[
            CueMarker::CueOut {
                duration_secs: 15.0,
            },
            CueMarker::CueIn,
        ]);
        assert!(tags.contains("#EXT-X-CUE-OUT:15.0"));
        assert!(tags.contains("#EXT-X-CUE-IN"));
    }
}
