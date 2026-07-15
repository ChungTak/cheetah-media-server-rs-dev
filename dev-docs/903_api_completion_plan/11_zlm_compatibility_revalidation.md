# 11 · 兼容接口重验

## 1. 证据等级

| 等级 | 含义 | 可宣称内容 |
| --- | --- | --- |
| L0 | 路由、参数、JSON golden，fake provider | wire 映射存在 |
| L1 | production provider + 本地真实媒体或文件 | 单接口真实可用 |
| L2 | 多接口流程和生命周期 | 场景可用 |
| L3 | 独立 HTTP 客户端黑盒测试 | 外部兼容可用 |
| L4 | 长稳、故障、性能与资源泄漏 | 可发布 |

已有目录中的每一接口重新登记 method/path、必选/可选参数、别名、成功/错误字段、domain mapping、capability、证据等级。不能因 64 个路由均挂载就写成 64 个能力均实现。

## 2. 高价值重验

- RTP create/connect/list/get/update/stop 必须委托同一 RtpApi；SSRC 更新真实生效后才返回成功。
- getSnap 的返回体必须是可解码 JPEG，错误不能返回 200 加伪图片。
- proxy 创建后的成功场景等待 Connected 与真实 frame；SSRF 拒绝场景单列。
- record start/stop 产生可解复用文件；load/control 回放必须委托 PlaybackApi 并输出帧。
- media list、online、keyframe、URL 必须使用真实 engine 状态和 output registry。
- hook 的通知与准入语义分别映射，不把异步通知响应当作同步准入。

危险或非媒体能力允许稳定返回 capability-not-supported，但必须保持约定的 HTTP/JSON 错误形状，不执行 shell、任意文件或任意代理请求。

## 3. Golden 与黑盒

golden fixture 固定字段名、数值/字符串转换、布尔别名、缺省值、数组顺序和错误码。测试应通过 adapter 公共入口，不直接调用私有 DTO helper作为唯一证据。

L3 启动完整服务器，用独立客户端只通过 HTTP：创建媒体资源、查询、修改、下载、停止，再检查 engine/provider 的可观察结果。测试使用动态端口和临时目录，不依赖开发机服务。

## 4. 任务与完成标准

- `ZLM-01`：重建完整接口目录和真实证据列。
- `ZLM-02`：修正 RTP/快照/代理/回放高价值映射。
- `ZLM-03`：补字段、错误和别名 golden。
- `ZLM-04`：补 L3 黑盒流程与资源清理。

发布报告按接口列出最高证据等级；低于 L1 的条目不得描述为“功能已实现”，只能描述为“兼容响应已定义”。

