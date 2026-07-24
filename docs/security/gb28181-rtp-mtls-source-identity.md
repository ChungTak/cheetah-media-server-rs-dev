# GB28181/RTP mTLS, SAN/Source Identity and Certificate Rotation

> Scope: this document defines the contract between the **external network adapter**
> (which terminates mTLS, validates client certificates, and extracts the SAN/source
> identity) and the **Cheetah RTP media plane** in this repository.
> The media plane itself does not terminate TLS or manage certificates.

## 1. Goals

- Bind every RTP/RTCP flow to an authenticated source identity supplied by the network adapter.
- Use source identity for source binding, rebind rate limiting, and audit without exposing secrets.
- Define a certificate rotation window that does not tear down existing sessions.

## 2. External network adapter responsibilities

The network adapter MUST:

1. Terminate mTLS for RTP/TCP, RTCP/TCP, and control channels that carry media parameters.
2. Validate the client certificate chain, revocation status, and expiry.
3. Extract a stable source identity from the client certificate SAN, in order:
   - DNS SAN matching the assigned device domain.
   - URI SAN (`URI:spiffe://...` or `URI:https://.../device/<id>`).
   - IP SAN for provisioned static devices.
   - Common Name (CN) only when no SAN is present (legacy).
4. Map the source identity to the media-plane source transport tuple (`ip:port`) at session open.
5. Forward the identity through the existing `AuthCredentials::mtls_identity` field, which is already redacted in logs and can be consumed by `RtpSessionApi` providers.
6. Reject or drop packets whose source tuple does not match the bound identity, unless the media plane requests a rebind.

## 3. Media-plane contract

The media plane uses `RtpSessionParams.source_binding_policy` and `RtpCore` source-rebind rate limiting. The identity assertion is the missing link:

```text
Network adapter              Cheetah RTP media plane
     |                                |
     |-- mTLS handshake -------------->| (adapter validates cert)
     |-- extracts SAN source identity -|
     |-- AuthCredentials.mtls_identity -> (passed in RtpSessionApi call)
     |                                |
     |-- RTP/RTCP packets ------------>| (adapter only forwards tuples
     |                                |  matching the bound identity)
```

The media plane expects:

- `AuthCredentials::mtls_identity` is present when mTLS is required.
- The identity may be logged for diagnostics (SAN is not a secret) but certificates and keys are never logged.
- Source rebind events include the new source tuple and the authenticated identity when available.

## 4. Certificate rotation

1. The adapter keeps a *primary* and a *secondary* certificate/key pair.
2. New connections are accepted with either certificate during the rotation window.
3. Existing RTP sessions keep using the certificate negotiated at session start.
4. Once all sessions using the old certificate have drained, the old pair is removed.
5. Rotation window and drain timeout are configured in the network adapter.

## 5. Identity lifecycle

| Phase | Identity source | Logged fields | Storage |
| --- | --- | --- | --- |
| Session open | `AuthCredentials::mtls_identity` | `source_identity`, `media_key` | session state |
| Packet receive | Adapter-mapped tuple | `source_identity` (sanitized), `ssrc`, `pt` | metrics |
| Source rebind | Rebind command from adapter | `source_identity`, `new_endpoint` | event |
| Session close | Session state | `source_identity`, `close_reason` | event |

## 6. Current status

This repository implements the media-plane side:

- `AuthCredentials` carries `mtls_identity` and redacts secrets from `Debug`.
- `RtpSessionApi` validates `MediaMutationContext` and enforces source-rebind rate limits.

The mTLS termination, SAN extraction, and certificate rotation window are external to this repository and tracked as `BLOCKED` in the audited baseline (SEC-05).
