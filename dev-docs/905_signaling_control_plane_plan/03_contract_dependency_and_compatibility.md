# 03 · 共享合同依赖与兼容发布

## 1. CT-01：唯一合同来源

`cheetah.media.v1` 的 source of truth 位于 signaling 仓。signaling 必须先发布包含以下内容
的固定 tag：

- typed MediaCapability/Query/Rtp/Proxy/Record/Snapshot/Playback/Output/Control/EventStream；
- 完整 `MediaMutationContext`；
- stable wire error 与 `NOT_APPLIED | APPLIED | UNKNOWN`；
- registry lease/TTL/heartbeat interval/accepted contract version；
- cursor/filter/gap/event header；
- SecretExchange 所需的 handle-only 合同。

若发布合同缺少上述任一 P0 字段，媒体仓停止 mapper 实现并向 signaling 修正合同；不得用
metadata map、JSON、bytes payload 或本地 Proto 补洞。

## 2. CT-02：Cargo 消费

新增的 gRPC adapter 只通过固定 git revision 消费 `cheetah-signal-contracts`：

```text
cheetah-signal-contracts = {
  git = "<signaling repository>",
  rev = "<full commit>",
  version = "<published version>",
  default-features = false
}
```

发布 tag、完整 commit、descriptor SHA-256 和最低/最高 contract version 同时写入：

- Cargo.lock；
- adapter build metadata；
- capability response；
- startup structured log；
- SBOM/release evidence。

只写 tag 不写 full revision 不算固定。媒体仓不得增加 `proto/cheetah/media/v1` 副本。

## 3. CT-03：Descriptor 与 breaking gate

CI 从锁定的 signaling revision 取得 descriptor，并执行：

- checksum 与仓库记录一致；
- buf lint/breaking baseline 通过；
- generated Rust code 可编译；
- old reader/new writer、new reader/old writer；
- enum 0 值为 UNSPECIFIED；
- 删除字段已 reserved name/number；
- 新字段不复用旧 generic 字段语义。

adapter build script 将 checksum、tag、revision 编译进只读版本信息，不在构建时访问网络。

## 4. 版本协商

媒体节点配置：

```text
minimum_supported_contract_version
maximum_supported_contract_version
required_descriptor_checksum
```

注册时发送支持区间、当前 descriptor checksum 和 capability generation。signaling 返回唯一
accepted version。规则固定：

- 无交集：注册失败，gRPC health 保持 NotServing；
- checksum 不匹配且不在显式兼容表：VersionMismatch；
- accepted version 只在重新注册后变化；
- heartbeat 不暗中升级 contract；
- 滚动升级窗口内同时保留前一兼容版本 mapper，窗口结束后显式移除。

## 5. DTO 隔离

generated DTO 只能出现在 `cheetah-media-grpc-adapter`：

```text
wire request
  -> validate required proto fields
  -> explicit mapper
  -> cheetah-media-api domain request/context
  -> control-plane facade/provider
  -> domain response/error
  -> explicit wire mapper
```

禁止：

- engine/provider struct 包含 generated DTO；
- SQLite 存储 prost bytes 作为领域状态；
- `serde_json::Value`/map 表达核心 typed operation；
- handler 绕过 mapper 直接构造 engine 内部对象。

未知 enum 值映射为 Unsupported/VersionMismatch，不使用 `_ => default` 静默降级。

## 6. Mapper 矩阵

每个 RPC 在实现前填写：

| Wire RPC | Domain port/method | Context | Resource kind | Capability operation | Error/outcome |
| --- | --- | --- | --- | --- | --- |
| GetCapabilities | capability registry | read | node | capability.read | NOT_APPLIED |
| Query operations | corresponding typed port | read | typed | query/list/get | NOT_APPLIED |
| Create/Open/Start | corresponding typed port | mutation | typed | create/open/start | all |
| Update/Control | corresponding typed port | mutation | typed | update/control | all |
| Stop/Delete | corresponding typed port | mutation | typed | stop/delete | all |
| Subscribe | replayable event API | read | event stream | event.subscribe | NOT_APPLIED |

最终表必须逐 RPC 展开，不允许用“其余类似”省略。

## 7. 外部交付物

signaling 需向媒体仓提供：

- 固定 tag/full revision；
- descriptor 与 SHA-256；
- supported version policy；
- simulator；
- 可通过 endpoint 参数运行的黑盒 contract suite；
- mTLS 测试证书生成方式；
- SecretExchange fake；
- release/compatibility notes。

媒体仓提供 server artifact 和启动配置给 signaling CI，不反向提交或修改 signaling 工作树。
