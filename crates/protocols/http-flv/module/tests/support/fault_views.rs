#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct FaultView {
    pub name: &'static str,
    pub chunks: Vec<Vec<u8>>,
}

pub fn build_flv_fault_views(bytes: &[u8]) -> Vec<FaultView> {
    vec![
        FaultView {
            name: "single_buffer",
            chunks: vec![bytes.to_vec()],
        },
        FaultView {
            name: "one_byte_chunks",
            chunks: bytes.iter().map(|b| vec![*b]).collect(),
        },
        FaultView {
            name: "coalesced_4",
            chunks: coalesced(bytes, 4),
        },
        FaultView {
            name: "prefix_truncated",
            chunks: vec![bytes[..bytes.len().saturating_div(2)].to_vec()],
        },
        FaultView {
            name: "suffix_truncated_tag",
            chunks: vec![bytes[..bytes.len().saturating_sub(1)].to_vec()],
        },
        FaultView {
            name: "duplicate_record",
            chunks: duplicate_middle(coalesced(bytes, 8)),
        },
        FaultView {
            name: "swap_adjacent",
            chunks: swap_adjacent(coalesced(bytes, 8)),
        },
        FaultView {
            name: "drop_every_nth",
            chunks: drop_every_nth(coalesced(bytes, 8), 3),
        },
        FaultView {
            name: "chunked_split_every_byte",
            chunks: bytes.iter().map(|b| vec![*b]).collect(),
        },
        FaultView {
            name: "ws_fragmented_binary",
            chunks: coalesced(bytes, (bytes.len() / 2).max(1)),
        },
    ]
}

fn coalesced(bytes: &[u8], n: usize) -> Vec<Vec<u8>> {
    let size = n.max(1);
    bytes.chunks(size).map(|chunk| chunk.to_vec()).collect()
}

fn duplicate_middle(mut chunks: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    if chunks.len() > 2 {
        let mid = chunks.len() / 2;
        let copy = chunks[mid].clone();
        chunks.insert(mid, copy);
    }
    chunks
}

fn swap_adjacent(mut chunks: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    if chunks.len() > 3 {
        chunks.swap(1, 2);
    }
    chunks
}

fn drop_every_nth(chunks: Vec<Vec<u8>>, nth: usize) -> Vec<Vec<u8>> {
    let nth = nth.max(1);
    chunks
        .into_iter()
        .enumerate()
        .filter_map(|(idx, chunk)| {
            if idx > 0 && idx.is_multiple_of(nth) {
                None
            } else {
                Some(chunk)
            }
        })
        .collect()
}
