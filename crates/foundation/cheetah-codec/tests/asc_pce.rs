use cheetah_codec::{aac_channel_count_from_asc, AacAudioSpecificConfig};

fn decode_hex(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16).expect("hex digit");
        let lo = (bytes[i + 1] as char).to_digit(16).expect("hex digit");
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    out
}

#[test]
fn pce_5_1_asc_parse() {
    // From bbb_sunflower_1080p_30fps_normal.flv: ch_cfg=0 + PCE for 5.1
    let asc = decode_hex("118004c844002000c40c4c61766336312e332e31303056e500");
    let cfg = AacAudioSpecificConfig::from_bytes(&asc).unwrap();
    assert_eq!(cfg.audio_object_type, 2);
    assert_eq!(cfg.sampling_frequency_index, 3);
    assert_eq!(cfg.channel_configuration, 0);
    let chans = aac_channel_count_from_asc(&asc);
    assert_eq!(chans, Some(6), "expected 5.1 (6 channels) parsed from PCE");
}
