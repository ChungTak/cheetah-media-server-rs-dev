# HTTP-FLV Fixture Set

本目录保存小型可提交的 `.flvstream` 样例（标准 FLV 字节流），用于：

- manifest 一致性与路径安全校验
- FLV demux 边界校验
- HTTP-FLV / WS-FLV pull 端到端回归

约束：

- 单文件必须有界（当前测试要求 <= 64 KiB）
- `manifest.tsv` 中 `fixture` 必须是相对路径，且禁止越级路径
- probe 样例只要求可 bounded 处理，不要求业务可播放
