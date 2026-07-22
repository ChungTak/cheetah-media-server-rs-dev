/// State for an active SRT publish ingress session.
///
/// 活跃 SRT 发布入口会话的状态。
struct SrtIngressSession {
    stream_key: StreamKey,
    lease: PublishLease,
    publisher: Box<dyn PublisherSink>,
    demuxer: MpegTsDemuxer,
    tracks: Vec<TrackInfo>,
    tracks_published: bool,
}
/// Demux an MPEG-TS payload and push frames into the publisher sink.
///
/// 解复用 MPEG-TS 负载并将帧推入发布者接收端。
fn handle_ingress_payload(session: &mut SrtIngressSession, payload: &[u8]) {
    for event in session.demuxer.push(payload) {
        match event {
            MpegTsDemuxEvent::TrackFound(track) => {
                if merge_track_update(&mut session.tracks, track) {
                    session.tracks_published = false;
                }
            }
            MpegTsDemuxEvent::TrackRemoved(ids) => {
                let before = session.tracks.len();
                session.tracks.retain(|t| !ids.contains(&t.track_id));
                if session.tracks.len() != before {
                    session.tracks_published = false;
                }
            }
            MpegTsDemuxEvent::Frame(frame) => {
                if !session.tracks_published
                    && !session.tracks.is_empty()
                    && session
                        .publisher
                        .update_tracks(session.tracks.clone())
                        .is_ok()
                {
                    session.tracks_published = true;
                }
                let _ = session.publisher.push_frame(Arc::new(frame));
            }
            MpegTsDemuxEvent::Diagnostic(diagnostic) => {
                debug!(stream_key = %session.stream_key, ?diagnostic, "SRT TS demux diagnostic");
            }
        }
    }
}

/// Insert or update a track in the session track list and return whether it changed.
///
/// 在会话轨道列表中插入或更新轨道，并返回是否发生变化。
fn merge_track_update(tracks: &mut Vec<TrackInfo>, track: TrackInfo) -> bool {
    if let Some(existing) = tracks
        .iter_mut()
        .find(|existing| existing.track_id == track.track_id)
    {
        if *existing == track {
            return false;
        }
        *existing = track;
        return true;
    }

    tracks.push(track);
    true
}
