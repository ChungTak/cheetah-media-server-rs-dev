# Phase 06: minimal Pion WHIP/WHEP helper used by the nightly interop
# lab. This image builds a tiny Go binary that can run as either a
# WHIP publisher (sending a synthetic VP8 stream) or a WHEP player
# (validating that an incoming stream produces a first keyframe and
# dumps getStats-like counters).
#
# The actual Go source lives in `interop-pion-helper/main.go`. The
# helper is intentionally small so reviewers can audit the protocol
# behaviour we exercise during the nightly run.

FROM golang:1.22-alpine AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY main.go ./
RUN CGO_ENABLED=0 go build -o /bin/cheetah-pion-helper main.go

FROM alpine:3.19
COPY --from=build /bin/cheetah-pion-helper /usr/local/bin/cheetah-pion-helper
ENV WEBRTC_INTEROP_PION_BIN=/usr/local/bin/cheetah-pion-helper
# Run under an unprivileged user because the helper does not need
# root for the loopback stack we exercise.
RUN adduser -D -u 1000 pion
USER pion
ENTRYPOINT ["/usr/local/bin/cheetah-pion-helper"]
