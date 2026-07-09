#![no_main]
use libfuzzer_sys::fuzz_target;

use cheetah_codec::SampleTable;

// The fuzzer feeds an arbitrary bytestring as the four sample-table primitives
// (`stts`, `stsc`, `stsz`, `stco`). The intent is to exercise the
// cross-referenced index construction without panicking.
fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }

    // Use the input bytes as a tiny "specification" for the table. We bound
    // every count by 64 to keep allocations tight even on adversarial input.
    let stts_count = (data[0] as usize % 16).min(data.len() / 8);
    let stsz_count = (data[1] as usize % 16).min(data.len() / 4);
    let stsc_count = (data[2] as usize % 8).min(data.len() / 12);
    let chunk_count = (data[3] as usize % 8).max(1);

    let mut st = SampleTable::default();
    let mut idx = 4;
    for _ in 0..stts_count {
        if idx + 8 > data.len() {
            break;
        }
        let count = u32::from_be_bytes([data[idx], data[idx + 1], data[idx + 2], data[idx + 3]])
            % 64
            + 1;
        let delta = u32::from_be_bytes([data[idx + 4], data[idx + 5], data[idx + 6], data[idx + 7]])
            % 1_000_000;
        st.stts.push((count, delta));
        idx += 8;
    }
    for _ in 0..stsc_count {
        if idx + 12 > data.len() {
            break;
        }
        let first =
            u32::from_be_bytes([data[idx], data[idx + 1], data[idx + 2], data[idx + 3]]) % 8 + 1;
        let spc = u32::from_be_bytes([data[idx + 4], data[idx + 5], data[idx + 6], data[idx + 7]])
            % 16
            + 1;
        let sd =
            u32::from_be_bytes([data[idx + 8], data[idx + 9], data[idx + 10], data[idx + 11]])
                % 4
                + 1;
        st.stsc.push((first, spc, sd));
        idx += 12;
    }
    if st.stsc.is_empty() {
        st.stsc.push((1, 1, 1));
    }
    for _ in 0..stsz_count {
        if idx + 4 > data.len() {
            break;
        }
        let size =
            u32::from_be_bytes([data[idx], data[idx + 1], data[idx + 2], data[idx + 3]]) % 4096;
        st.stsz_sizes.push(size);
        idx += 4;
    }
    for i in 0..chunk_count {
        st.stco.push(((i as u64).saturating_mul(4096)).min(u64::from(u32::MAX)));
    }
    let _ = st.build_index(48_000);
});
