#!/usr/bin/env python3
"""Extract compact RTMP TCP payload fixtures from local pcap captures."""

from __future__ import annotations

import argparse
import dataclasses
import ipaddress
import struct
import sys
from pathlib import Path
from urllib.parse import urlparse

MAGIC = b"CRF1"
MANIFEST_HEADER = (
    "case\tsource_pcap\tstream_name\tmedia_sig\trole\tfixture\t"
    "expect_connected\texpect_publish\texpect_play\texpect_media_min\tnotes"
)
DEFAULT_CASES = [
    {
        "case": "h264_aac_publish",
        "pcap": "from_file_017_source_200kbps_768x320_fc8200c552.pcap",
        "fixture": "standard/h264_aac_publish.rtmpflow",
        "role": "server_publish_c2s",
        "media_sig": "v=h264@768x320;a=aac@ch2",
        "expect": (1, 1, 0, 1),
        "note": "standard h264/aac publish",
    },
    {
        "case": "h265_aac_publish",
        "pcap": "from_file_003_bbb_1920x1080_hevc_5d24fd11cc.pcap",
        "fixture": "standard/h265_aac_publish.rtmpflow",
        "role": "server_publish_c2s",
        "media_sig": "v=h265@1920x1080;a=aac@ch2",
        "expect": (1, 1, 0, 1),
        "note": "standard h265/aac publish",
    },
    {
        "case": "h265_large_publish",
        "pcap": "from_file_018_spreed_1080p_hevc_ccd40e8693.pcap",
        "fixture": "standard/h265_large_publish.rtmpflow",
        "role": "server_publish_c2s",
        "media_sig": "v=h265@1920x1080;a=aac@ch2",
        "expect": (1, 1, 0, 1),
        "note": "standard h265 large publish",
    },
    {
        "case": "audio_only_publish",
        "pcap": "from_file_010_fallback_audio_f1932e2a04.pcap",
        "fixture": "standard/audio_only_publish.rtmpflow",
        "role": "server_publish_c2s",
        "media_sig": "v=none@0x0;a=aac@ch2",
        "expect": (1, 1, 0, 1),
        "note": "standard audio-only publish",
    },
    {
        "case": "av1_probe",
        "pcap": "from_file_005_big_buck_bunny_av1_1080_10s_5mb_dd25581306.pcap",
        "fixture": "probes/av1_probe.rtmpflow",
        "role": "robustness_probe",
        "media_sig": "v=av1@1920x1080;a=none@ch0",
        "expect": (0, 0, 0, 0),
        "note": "enhanced/compat probe av1",
    },
    {
        "case": "vp8_probe",
        "pcap": "from_file_011_fallback_video_vp8_fb9c7e866d.pcap",
        "fixture": "probes/vp8_probe.rtmpflow",
        "role": "robustness_probe",
        "media_sig": "v=vp8@320x240;a=none@ch0",
        "expect": (0, 0, 0, 0),
        "note": "enhanced/compat probe vp8",
    },
    {
        "case": "vp9_probe",
        "pcap": "from_file_012_fallback_video_vp9_72cd0a041b.pcap",
        "fixture": "probes/vp9_probe.rtmpflow",
        "role": "robustness_probe",
        "media_sig": "v=vp9@320x240;a=none@ch0",
        "expect": (0, 0, 0, 0),
        "note": "enhanced/compat probe vp9",
    },
    {
        "case": "h266_probe",
        "pcap": "from_file_013_arknights_reimei_zensou_01_vvc_1080p_aac_c8237ebd76.pcap",
        "fixture": "probes/h266_probe.rtmpflow",
        "role": "robustness_probe",
        "media_sig": "v=h266@1920x1080;a=aac@ch2",
        "expect": (0, 0, 0, 0),
        "note": "enhanced/compat probe h266/vvc",
    },
]


class CaptureError(Exception):
    """Fatal capture parsing error."""


class SkippedCapture(Exception):
    """Expected skip for unusable local capture files."""


@dataclasses.dataclass(frozen=True)
class FlowKey:
    src_ip: str
    src_port: int
    dst_ip: str
    dst_port: int


@dataclasses.dataclass
class Flow:
    key: FlowKey
    records: list[bytes]

    @property
    def payload_bytes(self) -> int:
        return sum(len(record) for record in self.records)


@dataclasses.dataclass(frozen=True)
class Summary:
    case: str
    media_sig: str
    stream_name: str


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-dir", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--max-fixture-bytes", type=int, default=262_144)
    args = parser.parse_args()

    source_dir = args.source_dir
    out_dir = args.out_dir
    max_fixture_bytes = args.max_fixture_bytes
    if max_fixture_bytes < 16:
        raise SystemExit("--max-fixture-bytes must allow at least one framed record")
    if not source_dir.is_dir():
        raise SystemExit(f"source dir does not exist: {source_dir}")

    summaries = load_summaries(source_dir)
    rows = []
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "standard").mkdir(exist_ok=True)
    (out_dir / "probes").mkdir(exist_ok=True)

    for case_def in DEFAULT_CASES:
        pcap_path = source_dir / case_def["pcap"]
        try:
            flows = parse_pcap_payload_flows(pcap_path)
            flow = select_flow(case_def["role"], flows)
            if flow is None:
                raise SkippedCapture("no RTMP TCP payload flow on port 1935")
            fixture_rel = Path(case_def["fixture"])
            kept_records = write_rtmpflow(out_dir / fixture_rel, flow.records, max_fixture_bytes)
        except SkippedCapture as err:
            print(f"skip {case_def['case']}: {err}", file=sys.stderr)
            continue
        except CaptureError as err:
            raise SystemExit(f"{pcap_path}: {err}") from err

        summary = summaries.get(case_def["pcap"])
        media_sig = summary.media_sig if summary else case_def["media_sig"]
        stream_name = summary.stream_name if summary else case_def["case"]
        expect_connected, expect_publish, expect_play, expect_media_min = case_def["expect"]
        note = f"{case_def['note']};records={kept_records};payload_bytes={flow.payload_bytes}"
        rows.append(
            [
                case_def["case"],
                case_def["pcap"],
                stream_name,
                media_sig,
                case_def["role"],
                fixture_rel.as_posix(),
                str(expect_connected),
                str(expect_publish),
                str(expect_play),
                str(expect_media_min),
                note,
            ]
        )

    if not rows:
        raise SystemExit("no fixtures generated from selected captures")

    manifest = out_dir / "manifest.tsv"
    manifest.write_text(
        MANIFEST_HEADER + "\n" + "\n".join("\t".join(row) for row in rows) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(rows)} fixtures to {out_dir}")
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
            pcap_name = Path(fields[pcap_idx]).name
            note = parse_note(fields[note_idx])
            summaries[pcap_name] = Summary(
                case=fields[case_idx],
                media_sig=note.get("media_sig", "unknown"),
                stream_name=stream_name_from_target(note.get("stream_target", fields[case_idx])),
            )
    return summaries


def parse_note(note: str) -> dict[str, str]:
    values = {}
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


def parse_pcap_payload_flows(path: Path) -> dict[FlowKey, Flow]:
    if not path.exists():
        raise SkippedCapture("pcap file is missing")
    data = path.read_bytes()
    if not data:
        raise SkippedCapture("pcap file is empty")
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

    flows: dict[FlowKey, Flow] = {}
    offset = 24
    packet_index = 0
    while offset < len(data):
        if len(data) - offset < 16:
            raise CaptureError(f"truncated packet header at packet {packet_index}")
        _ts_sec, _ts_usec, incl_len, _orig_len = struct.unpack(
            f"{endian}IIII", data[offset : offset + 16]
        )
        offset += 16
        if incl_len > len(data) - offset:
            raise CaptureError(f"truncated packet payload at packet {packet_index}")
        packet = data[offset : offset + incl_len]
        offset += incl_len
        packet_index += 1
        payload = parse_packet_tcp_payload(linktype, packet)
        if payload is None:
            continue
        key, tcp_payload = payload
        if not tcp_payload:
            continue
        flow = flows.setdefault(key, Flow(key=key, records=[]))
        flow.records.append(tcp_payload)

    return flows


def pcap_endian(magic: bytes) -> str | None:
    if magic in (b"\xd4\xc3\xb2\xa1", b"\x4d\x3c\xb2\xa1"):
        return "<"
    if magic in (b"\xa1\xb2\xc3\xd4", b"\xa1\xb2\x3c\x4d"):
        return ">"
    return None


def parse_packet_tcp_payload(linktype: int, packet: bytes) -> tuple[FlowKey, bytes] | None:
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
    ihl = (l3[0] & 0x0F) * 4
    if version != 4 or ihl < 20 or len(l3) < ihl:
        return None
    total_len = int.from_bytes(l3[2:4], "big")
    if total_len < ihl or total_len > len(l3):
        return None
    flags_fragment = int.from_bytes(l3[6:8], "big")
    if flags_fragment & 0x1FFF:
        return None
    if l3[9] != 6:
        return None

    src_ip = str(ipaddress.IPv4Address(l3[12:16]))
    dst_ip = str(ipaddress.IPv4Address(l3[16:20]))
    tcp = l3[ihl:total_len]
    if len(tcp) < 20:
        return None
    src_port = int.from_bytes(tcp[0:2], "big")
    dst_port = int.from_bytes(tcp[2:4], "big")
    data_offset = (tcp[12] >> 4) * 4
    if data_offset < 20 or len(tcp) < data_offset:
        return None
    key = FlowKey(src_ip=src_ip, src_port=src_port, dst_ip=dst_ip, dst_port=dst_port)
    return key, tcp[data_offset:]


def select_flow(role: str, flows: dict[FlowKey, Flow]) -> Flow | None:
    if role in ("server_publish_c2s", "robustness_probe"):
        return select_publish_c2s(flows)
    if role == "server_play_c2s":
        return select_play_c2s(flows)
    if role in ("client_publish_s2c", "client_play_s2c"):
        return select_server_s2c(flows)
    raise CaptureError(f"unsupported role {role}")


def select_publish_c2s(flows: dict[FlowKey, Flow]) -> Flow | None:
    candidates = [flow for flow in flows.values() if flow.key.dst_port == 1935 and flow.records]
    return max(candidates, key=lambda flow: flow.payload_bytes, default=None)


def select_play_c2s(flows: dict[FlowKey, Flow]) -> Flow | None:
    candidates = [flow for flow in flows.values() if flow.key.dst_port == 1935 and flow.records]
    if not candidates:
        return None
    return min(candidates, key=lambda flow: flow.payload_bytes)


def select_server_s2c(flows: dict[FlowKey, Flow]) -> Flow | None:
    candidates = [flow for flow in flows.values() if flow.key.src_port == 1935 and flow.records]
    return max(candidates, key=lambda flow: flow.payload_bytes, default=None)


def write_rtmpflow(path: Path, records: list[bytes], max_fixture_bytes: int) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    kept: list[bytes] = []
    encoded_len = 8
    for record in records:
        if not record:
            continue
        next_len = encoded_len + 4 + len(record)
        if next_len > max_fixture_bytes:
            break
        kept.append(record)
        encoded_len = next_len
    if not kept:
        raise SkippedCapture("selected flow has no record within max fixture size")

    with path.open("wb") as handle:
        handle.write(MAGIC)
        handle.write(struct.pack(">I", len(kept)))
        for record in kept:
            handle.write(struct.pack(">I", len(record)))
            handle.write(record)
    return len(kept)


if __name__ == "__main__":
    raise SystemExit(main())
