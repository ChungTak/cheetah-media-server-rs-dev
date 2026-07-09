#!/usr/bin/env python3
"""Extract compact RTSP capture fixtures from local pcap files.

This tool is intentionally stdlib-only so it can run in minimal CI/dev images.
"""

from __future__ import annotations

import argparse
import dataclasses
import ipaddress
import struct
import sys
from pathlib import Path
from urllib.parse import urlparse

MAGIC = b"RSF1"
MANIFEST_HEADER = (
    "case\tsource_pcap\tstream_name\tmedia_sig\tpush_transport\tpull_transport\t"
    "role\tfixture\texpect_methods\texpect_rtp_min\texpect_rtcp_min\texpect_tracks_min\tnotes"
)
SKIPPED_HEADER = "case\tsource_pcap\treason"

# kind values must match crates/cheetah-rtsp-property-tests/tests/support/rtsp_capture_fixture.rs
KIND_RTSP_TCP_C2S = 1
KIND_RTSP_TCP_S2C = 2
KIND_UDP_PUBLISH_RTP = 3
KIND_UDP_PUBLISH_RTCP = 4
KIND_UDP_PLAY_RTP = 5
KIND_UDP_PLAY_RTCP = 6
KIND_TCP_INTERLEAVED_RTP = 7
KIND_TCP_INTERLEAVED_RTCP = 8
FLAG_STANDARD_ASSERTABLE = 0x01
FLAG_PROBE_ONLY = 0x02
FLAG_TRUNCATED_PREFIX = 0x04

DEFAULT_CASES = [
    {
        "case": "h264_tcp_publish_play",
        "pcap": "from_file_017_source_200kbps_768x320_fc8200c552__push_tcp__pull_tcp.pcap",
        "fixture": "standard/h264_tcp_publish_play.rtspcap",
        "role": "standard_publish_tcp",
        "media_sig": "v=h264@768x320;a=aac@ch2",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD,DESCRIBE,PLAY",
        "expect_rtp_min": 1,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 1,
    },
    {
        "case": "h264_udp_publish_play",
        "pcap": "from_file_017_source_200kbps_768x320_fc8200c552__push_udp__pull_udp.pcap",
        "fixture": "standard/h264_udp_publish_play.rtspcap",
        "role": "standard_publish_udp",
        "media_sig": "v=h264@768x320;a=aac@ch2",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD,DESCRIBE,PLAY",
        "expect_rtp_min": 1,
        "expect_rtcp_min": 1,
        "expect_tracks_min": 1,
    },
    {
        "case": "h265_tcp_publish_play",
        "pcap": "from_file_003_bbb_1920x1080_hevc_5d24fd11cc__push_tcp__pull_tcp.pcap",
        "fixture": "standard/h265_tcp_publish_play.rtspcap",
        "role": "standard_publish_tcp",
        "media_sig": "v=h265@1920x1080;a=aac@ch6",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD,DESCRIBE,PLAY",
        "expect_rtp_min": 1,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 1,
    },
    {
        "case": "audio_only_udp_publish_play",
        "pcap": "from_file_010_fallback_audio_f1932e2a04__push_udp__pull_udp.pcap",
        "fixture": "standard/audio_only_udp_publish_play.rtspcap",
        "role": "standard_publish_udp",
        "media_sig": "v=none@0x0;a=pcm@ch1",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD,DESCRIBE,PLAY",
        "expect_rtp_min": 1,
        "expect_rtcp_min": 1,
        "expect_tracks_min": 1,
    },
    {
        "case": "av1_probe",
        "pcap": "from_file_005_big_buck_bunny_av1_1080_10s_5mb_dd25581306__push_tcp__pull_tcp.pcap",
        "fixture": "probes/av1_probe.rtspcap",
        "role": "compat_probe",
        "media_sig": "v=av1@1920x1080;a=none@ch0",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD",
        "expect_rtp_min": 0,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 0,
    },
    {
        "case": "vp8_probe",
        "pcap": "from_file_011_fallback_video_vp8_fb9c7e866d__push_udp__pull_udp.pcap",
        "fixture": "probes/vp8_probe.rtspcap",
        "role": "compat_probe",
        "media_sig": "v=vp8@320x240;a=none@ch0",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD",
        "expect_rtp_min": 0,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 0,
    },
    {
        "case": "vp9_probe",
        "pcap": "from_file_012_fallback_video_vp9_72cd0a041b__push_tcp__pull_udp.pcap",
        "fixture": "probes/vp9_probe.rtspcap",
        "role": "compat_probe",
        "media_sig": "v=vp9@320x240;a=none@ch0",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD",
        "expect_rtp_min": 0,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 0,
    },
    {
        "case": "h266_probe",
        "pcap": "from_file_014_chainsaw_man_04_vvc_1080p_aac_qpa0_qp20_ae3b4d9277__push_tcp__pull_tcp.pcap",
        "fixture": "probes/h266_probe.rtspcap",
        "role": "compat_probe",
        "media_sig": "v=h266@1920x1080;a=aac@ch2",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD",
        "expect_rtp_min": 0,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 0,
    },
    {
        "case": "high_bitrate_probe",
        "pcap": "from_file_016_hd_club_4k_chimei_inn_40mbps_b01f503846__push_tcp__pull_tcp.pcap",
        "fixture": "probes/high_bitrate_probe.rtspcap",
        "role": "compat_probe",
        "media_sig": "v=h264@3840x2160;a=aac@ch2",
        "expect_methods": "OPTIONS,ANNOUNCE,SETUP,RECORD",
        "expect_rtp_min": 0,
        "expect_rtcp_min": 0,
        "expect_tracks_min": 0,
    },
]


class CaptureError(Exception):
    """Fatal parsing error for a pcap file."""


class SkippedCapture(Exception):
    """Expected skip for unusable local capture files."""

    def __init__(self, reason: str) -> None:
        super().__init__(reason)
        self.reason = reason


@dataclasses.dataclass(frozen=True)
class Summary:
    case: str
    media_sig: str
    stream_name: str
    push_transport: str
    pull_transport: str


@dataclasses.dataclass(frozen=True)
class Packet:
    ts_us: int
    proto: str
    src_ip: str
    src_port: int
    dst_ip: str
    dst_port: int
    payload: bytes


@dataclasses.dataclass(frozen=True)
class CaptureRecord:
    kind: int
    flags: int
    flow_id: int
    delta_us: int
    payload: bytes


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--max-fixture-bytes", type=int, default=524_288)
    args = parser.parse_args()

    if args.max_fixture_bytes < 64:
        raise SystemExit("--max-fixture-bytes must be >= 64")
    if not args.source_dir.is_dir():
        raise SystemExit(f"source dir does not exist: {args.source_dir}")

    summaries = load_summaries(args.source_dir)
    out_dir = args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "standard").mkdir(exist_ok=True)
    (out_dir / "probes").mkdir(exist_ok=True)

    rows: list[list[str]] = []
    skipped_rows: list[list[str]] = []
    for case_def in DEFAULT_CASES:
        pcap_path = args.source_dir / case_def["pcap"]
        try:
            packets = parse_pcap_packets(pcap_path)
            records = build_records(case_def, packets)
            kept, truncated = write_rtspcap(
                out_dir / case_def["fixture"], records, args.max_fixture_bytes
            )
        except SkippedCapture as err:
            print(f"skip {case_def['case']}: {err.reason}", file=sys.stderr)
            skipped_rows.append([case_def["case"], case_def["pcap"], err.reason])
            continue
        except CaptureError as err:
            raise SystemExit(f"{pcap_path}: {err}") from err

        summary = summaries.get(case_def["pcap"])
        stream_name = summary.stream_name if summary else case_def["case"]
        media_sig = summary.media_sig if summary and summary.media_sig != "unknown" else case_def["media_sig"]
        push_transport = summary.push_transport if summary else infer_transport(case_def["pcap"], "push")
        pull_transport = summary.pull_transport if summary else infer_transport(case_def["pcap"], "pull")
        rows.append(
            [
                case_def["case"],
                case_def["pcap"],
                stream_name,
                media_sig,
                push_transport,
                pull_transport,
                case_def["role"],
                case_def["fixture"],
                case_def["expect_methods"],
                str(case_def["expect_rtp_min"]),
                str(case_def["expect_rtcp_min"]),
                str(case_def["expect_tracks_min"]),
                f"records={kept};source_packets={len(packets)};truncated_prefix={1 if truncated else 0}",
            ]
        )

    if not rows:
        raise SystemExit("no fixtures generated from selected captures")

    manifest = out_dir / "manifest.tsv"
    manifest.write_text(
        MANIFEST_HEADER + "\n" + "\n".join("\t".join(row) for row in rows) + "\n",
        encoding="utf-8",
    )
    skipped_manifest = out_dir / "skipped.tsv"
    skipped_manifest.write_text(
        SKIPPED_HEADER + "\n" + "\n".join("\t".join(row) for row in skipped_rows) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(rows)} fixtures to {out_dir}")
    if skipped_rows:
        print(f"wrote {len(skipped_rows)} skipped rows to {skipped_manifest}", file=sys.stderr)
    return 0


def load_summaries(source_dir: Path) -> dict[str, Summary]:
    summaries: dict[str, Summary] = {}
    for name in ("summary_from_files.tsv", "summary.tsv"):
        path = source_dir / name
        if not path.exists():
            continue
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
        if not lines:
            continue
        header = lines[0].split("\t")
        try:
            case_idx = header.index("case")
            pcap_idx = header.index("pcap")
            note_idx = header.index("note")
        except ValueError:
            continue
        for line in lines[1:]:
            if not line:
                continue
            fields = line.split("\t")
            if len(fields) <= max(case_idx, pcap_idx, note_idx):
                continue
            note = parse_note(fields[note_idx])
            pcap_name = Path(fields[pcap_idx]).name
            summaries[pcap_name] = Summary(
                case=fields[case_idx],
                media_sig=note.get("media_sig", "unknown"),
                stream_name=stream_name_from_target(note.get("stream_target", fields[case_idx])),
                push_transport=note.get("push_transport", infer_transport(pcap_name, "push")),
                pull_transport=note.get("pull_transport", infer_transport(pcap_name, "pull")),
            )
    return summaries


def parse_note(note: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for part in note.split(";"):
        key, sep, value = part.partition("=")
        if sep:
            values[key] = value
    return values


def stream_name_from_target(target: str) -> str:
    parsed = urlparse(target)
    if parsed.path:
        parts = [part for part in parsed.path.split("/") if part]
        if parts:
            return parts[-1]
    return target


def infer_transport(name: str, direction: str) -> str:
    marker = f"__{direction}_"
    idx = name.find(marker)
    if idx == -1:
        return "tcp"
    tail = name[idx + len(marker) :]
    if tail.startswith("udp"):
        return "udp"
    return "tcp"


def parse_pcap_packets(path: Path) -> list[Packet]:
    if not path.exists():
        raise SkippedCapture("skipped_missing_pcap")
    data = path.read_bytes()
    if not data:
        raise SkippedCapture("skipped_empty_pcap")
    if len(data) < 24:
        raise CaptureError("truncated pcap global header")

    endian = pcap_endian(data[:4])
    if endian is None:
        raise CaptureError("unsupported pcap magic")

    _magic, major, minor, _thiszone, _sigfigs, snaplen, linktype = struct.unpack(
        f"{endian}IHHiiii", data[:24]
    )
    if major != 2 or minor != 4:
        raise CaptureError(f"unsupported pcap version {major}.{minor}")
    if snaplen <= 0:
        raise CaptureError(f"invalid pcap snaplen {snaplen}")
    if linktype not in (1, 276):
        raise CaptureError(f"unsupported pcap linktype {linktype}")

    packets: list[Packet] = []
    offset = 24
    packet_index = 0
    while offset < len(data):
        if len(data) - offset < 16:
            raise CaptureError(f"truncated packet header at packet {packet_index}")
        ts_sec, ts_usec, incl_len, _orig_len = struct.unpack(
            f"{endian}IIII", data[offset : offset + 16]
        )
        offset += 16
        if incl_len > snaplen:
            raise CaptureError(
                f"captured length {incl_len} exceeds snaplen {snaplen} at packet {packet_index}"
            )
        if incl_len > len(data) - offset:
            raise CaptureError(f"truncated packet payload at packet {packet_index}")
        packet = data[offset : offset + incl_len]
        offset += incl_len
        packet_index += 1

        parsed = parse_ipv4_transport_packet(linktype, packet)
        if parsed is None:
            continue
        proto, src_ip, src_port, dst_ip, dst_port, payload = parsed
        if not payload:
            continue
        packets.append(
            Packet(
                ts_us=ts_sec * 1_000_000 + ts_usec,
                proto=proto,
                src_ip=src_ip,
                src_port=src_port,
                dst_ip=dst_ip,
                dst_port=dst_port,
                payload=payload,
            )
        )

    if not packets:
        raise SkippedCapture("skipped_no_transport_payload")
    return packets


def pcap_endian(magic: bytes) -> str | None:
    if magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1"):
        return "<"
    if magic in (b"\xa1\xb2\xc3\xd4", b"\xa1\xb2\x3c\x4d"):
        return ">"
    return None


def parse_ipv4_transport_packet(
    linktype: int, packet: bytes
) -> tuple[str, str, int, str, int, bytes] | None:
    if linktype == 276:
        if len(packet) < 20:
            return None
        ethertype = int.from_bytes(packet[0:2], "big")
        l3 = packet[20:]
    elif linktype == 1:
        if len(packet) < 14:
            return None
        ethertype = int.from_bytes(packet[12:14], "big")
        l3 = packet[14:]
    else:
        return None

    if ethertype != 0x0800 or len(l3) < 20:
        return None
    version = l3[0] >> 4
    if version != 4:
        return None

    ihl = (l3[0] & 0x0F) * 4
    if ihl < 20 or ihl > len(l3):
        return None

    total_len = int.from_bytes(l3[2:4], "big")
    if total_len < ihl or total_len > len(l3):
        total_len = len(l3)

    proto = l3[9]
    src_ip = str(ipaddress.ip_address(l3[12:16]))
    dst_ip = str(ipaddress.ip_address(l3[16:20]))
    l4 = l3[ihl:total_len]

    if proto == 6:  # TCP
        if len(l4) < 20:
            return None
        src_port = int.from_bytes(l4[0:2], "big")
        dst_port = int.from_bytes(l4[2:4], "big")
        data_offset = (l4[12] >> 4) * 4
        if data_offset < 20 or data_offset > len(l4):
            return None
        return "tcp", src_ip, src_port, dst_ip, dst_port, bytes(l4[data_offset:])

    if proto == 17:  # UDP
        if len(l4) < 8:
            return None
        src_port = int.from_bytes(l4[0:2], "big")
        dst_port = int.from_bytes(l4[2:4], "big")
        return "udp", src_ip, src_port, dst_ip, dst_port, bytes(l4[8:])

    return None


def build_records(case_def: dict[str, object], packets: list[Packet]) -> list[CaptureRecord]:
    base_ts = min(packet.ts_us for packet in packets)
    flow_ids: dict[tuple[object, ...], int] = {}

    def flow_id(key: tuple[object, ...]) -> int:
        if key not in flow_ids:
            flow_ids[key] = len(flow_ids) + 1
        return flow_ids[key]

    records: list[CaptureRecord] = []
    base_flags = (
        FLAG_PROBE_ONLY if case_def["role"] == "compat_probe" else FLAG_STANDARD_ASSERTABLE
    )

    # RTSP TCP control plane and interleaved frames.
    for packet in packets:
        if packet.proto != "tcp":
            continue
        if packet.src_port != 8554 and packet.dst_port != 8554:
            continue
        if not packet.payload:
            continue

        kind = KIND_RTSP_TCP_C2S if packet.dst_port == 8554 else KIND_RTSP_TCP_S2C
        records.append(
            CaptureRecord(
                kind=kind,
                flags=base_flags,
                flow_id=flow_id((packet.proto, packet.src_ip, packet.src_port, packet.dst_ip, packet.dst_port)),
                delta_us=max(0, packet.ts_us - base_ts),
                payload=packet.payload,
            )
        )

        for channel, payload in iter_interleaved_frames(packet.payload):
            records.append(
                CaptureRecord(
                    kind=KIND_TCP_INTERLEAVED_RTP if channel % 2 == 0 else KIND_TCP_INTERLEAVED_RTCP,
                    flags=base_flags,
                    flow_id=flow_id(("tcp-interleaved", channel)),
                    delta_us=max(0, packet.ts_us - base_ts),
                    payload=payload,
                )
            )

    if not records:
        raise SkippedCapture("skipped_no_rtsp_8554")

    push_transport = infer_transport(case_def["pcap"], "push")
    pull_transport = infer_transport(case_def["pcap"], "pull")

    loopback_udp = [
        packet
        for packet in packets
        if packet.proto == "udp" and packet.src_ip == "127.0.0.1" and packet.dst_ip == "127.0.0.1"
    ]

    if push_transport == "udp" or pull_transport == "udp":
        udp_records = classify_udp_records(
            loopback_udp, base_ts, flow_id, push_transport, pull_transport, base_flags
        )
        records.extend(udp_records)

    # Stable ordering by capture timestamp and original insertion sequence.
    records.sort(key=lambda record: (record.delta_us, record.flow_id, record.kind))
    return records


def iter_interleaved_frames(payload: bytes) -> list[tuple[int, bytes]]:
    out: list[tuple[int, bytes]] = []
    cursor = 0
    # Parse contiguous "$" framed payload only; ignore mixed text+binary tails.
    while cursor + 4 <= len(payload) and payload[cursor] == 0x24:
        channel = payload[cursor + 1]
        frame_len = int.from_bytes(payload[cursor + 2 : cursor + 4], "big")
        cursor += 4
        if cursor + frame_len > len(payload):
            break
        out.append((channel, payload[cursor : cursor + frame_len]))
        cursor += frame_len
    return out


def classify_udp_records(
    packets: list[Packet],
    base_ts: int,
    flow_id_fn,
    push_transport: str,
    pull_transport: str,
    base_flags: int,
) -> list[CaptureRecord]:
    if not packets:
        return []

    by_flow: dict[tuple[str, int, str, int], list[Packet]] = {}
    for packet in packets:
        key = (packet.src_ip, packet.src_port, packet.dst_ip, packet.dst_port)
        by_flow.setdefault(key, []).append(packet)

    def is_rtp(payload: bytes) -> bool:
        return len(payload) >= 2 and (payload[0] >> 6) == 2 and payload[1] not in (200, 201, 202, 203, 204)

    def is_rtcp(payload: bytes) -> bool:
        return len(payload) >= 2 and (payload[0] >> 6) == 2 and payload[1] in (200, 201, 202, 203, 204)

    rtp_flows: list[tuple[tuple[str, int, str, int], list[Packet], int]] = []
    rtcp_flows: list[tuple[tuple[str, int, str, int], list[Packet], int]] = []
    for key, flow_packets in by_flow.items():
        rtp_bytes = sum(len(packet.payload) for packet in flow_packets if is_rtp(packet.payload))
        rtcp_bytes = sum(len(packet.payload) for packet in flow_packets if is_rtcp(packet.payload))
        if rtp_bytes > 0:
            rtp_flows.append((key, flow_packets, rtp_bytes))
        if rtcp_bytes > 0:
            rtcp_flows.append((key, flow_packets, rtcp_bytes))

    rtp_flows.sort(key=lambda item: item[2], reverse=True)
    rtcp_flows.sort(key=lambda item: item[2], reverse=True)

    selected: list[tuple[int, list[Packet], callable]] = []

    if push_transport == "udp" and rtp_flows:
        selected.append((KIND_UDP_PUBLISH_RTP, rtp_flows[0][1], is_rtp))
    if pull_transport == "udp" and len(rtp_flows) > 1:
        selected.append((KIND_UDP_PLAY_RTP, rtp_flows[1][1], is_rtp))

    if push_transport == "udp" and rtcp_flows:
        selected.append((KIND_UDP_PUBLISH_RTCP, rtcp_flows[0][1], is_rtcp))
    if pull_transport == "udp" and len(rtcp_flows) > 1:
        selected.append((KIND_UDP_PLAY_RTCP, rtcp_flows[1][1], is_rtcp))

    records: list[CaptureRecord] = []
    for kind, flow_packets, accept in selected:
        if not flow_packets:
            continue
        key = ("udp", flow_packets[0].src_ip, flow_packets[0].src_port, flow_packets[0].dst_ip, flow_packets[0].dst_port)
        fid = flow_id_fn(key)
        for packet in flow_packets:
            if not accept(packet.payload):
                continue
            records.append(
                CaptureRecord(
                    kind=kind,
                    flags=base_flags,
                    flow_id=fid,
                    delta_us=max(0, packet.ts_us - base_ts),
                    payload=packet.payload,
                )
            )

    return records


def write_rtspcap(
    path: Path, records: list[CaptureRecord], max_fixture_bytes: int
) -> tuple[int, bool]:
    if not records:
        raise SkippedCapture("skipped_no_capture_records")

    kept: list[CaptureRecord] = []
    total_bytes = 8  # magic + record_count
    for record in records:
        entry_size = 12 + len(record.payload)
        if entry_size > max_fixture_bytes:
            raise SkippedCapture(
                f"skipped_record_too_large:{entry_size}>{max_fixture_bytes}"
            )
        if kept and total_bytes + entry_size > max_fixture_bytes:
            break
        if not kept and total_bytes + entry_size > max_fixture_bytes:
            # keep at least one whole record if it fits the global limit
            break
        kept.append(record)
        total_bytes += entry_size

    if not kept:
        raise SkippedCapture("skipped_empty_after_max_size")
    truncated = len(kept) < len(records)

    blob = bytearray()
    blob.extend(MAGIC)
    blob.extend(struct.pack(">I", len(kept)))
    for record in kept:
        flags = record.flags | (FLAG_TRUNCATED_PREFIX if truncated else 0)
        blob.extend(struct.pack(">BBHI", record.kind, flags, record.flow_id, record.delta_us))
        blob.extend(struct.pack(">I", len(record.payload)))
        blob.extend(record.payload)

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(bytes(blob))
    return len(kept), truncated


if __name__ == "__main__":
    raise SystemExit(main())
