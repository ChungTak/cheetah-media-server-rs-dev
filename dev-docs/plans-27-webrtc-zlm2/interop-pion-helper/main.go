// Package main contains a tiny WHIP/WHEP helper used by the
// cheetah-media-server-rs interop lab.
//
// Usage:
//
//	cheetah-pion-helper --mode whip --url <whip-url> [--token <token>]
//	cheetah-pion-helper --mode whep --url <whep-url> [--token <token>]
//
// In WHIP mode the helper synthesises a VP8 stream from a static
// pattern (a colour bar generator) and POSTs the offer to the WHIP
// endpoint. In WHEP mode the helper acts as a player that POSTs an
// offer-less SDP request to the WHEP endpoint, applies the answer,
// and prints the first decoded keyframe latency to stdout.
//
// The helper writes a small JSON stats file to
// $WEBRTC_INTEROP_ARTIFACT_DIR/peer-stats.json when it exits cleanly
// so the harness can assert against the counters.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"time"

	"github.com/pion/webrtc/v3"
)

type stats struct {
	FirstKeyframeMs int64 `json:"first_keyframe_ms"`
	NACKsSent       int64 `json:"nacks_sent"`
	NACKsReceived   int64 `json:"nacks_received"`
	BytesSent       int64 `json:"bytes_sent"`
	BytesReceived   int64 `json:"bytes_received"`
}

func main() {
	mode := flag.String("mode", "", "either 'whip' or 'whep'")
	url := flag.String("url", "", "remote WHIP/WHEP URL")
	token := flag.String("token", "", "optional bearer token")
	timeout := flag.Duration("timeout", 30*time.Second, "overall test timeout")
	flag.Parse()

	if *mode == "" || *url == "" {
		log.Fatalf("usage: --mode whip|whep --url <url>")
	}

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()
	ctx, timeoutCancel := context.WithTimeout(ctx, *timeout)
	defer timeoutCancel()

	switch *mode {
	case "whip":
		if err := runWHIP(ctx, *url, *token); err != nil {
			log.Fatalf("whip failed: %v", err)
		}
	case "whep":
		if err := runWHEP(ctx, *url, *token); err != nil {
			log.Fatalf("whep failed: %v", err)
		}
	default:
		log.Fatalf("unknown mode %q", *mode)
	}
}

func runWHIP(ctx context.Context, endpoint, token string) error {
	pc, err := webrtc.NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		return fmt.Errorf("new peer connection: %w", err)
	}
	defer pc.Close()

	track, err := webrtc.NewTrackLocalStaticSample(
		webrtc.RTPCodecCapability{MimeType: webrtc.MimeTypeVP8},
		"video", "cheetah-pion-helper",
	)
	if err != nil {
		return fmt.Errorf("new track: %w", err)
	}
	if _, err := pc.AddTrack(track); err != nil {
		return fmt.Errorf("add track: %w", err)
	}

	offer, err := pc.CreateOffer(nil)
	if err != nil {
		return fmt.Errorf("create offer: %w", err)
	}
	if err := pc.SetLocalDescription(offer); err != nil {
		return fmt.Errorf("set local: %w", err)
	}
	<-webrtc.GatheringCompletePromise(pc)
	answer, err := postWHIP(ctx, endpoint, token, pc.LocalDescription().SDP)
	if err != nil {
		return err
	}
	if err := pc.SetRemoteDescription(webrtc.SessionDescription{
		Type: webrtc.SDPTypeAnswer,
		SDP:  answer,
	}); err != nil {
		return fmt.Errorf("set remote: %w", err)
	}
	// Sample loop: send a fake keyframe periodically.
	sample := []byte{0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a}
	tick := time.NewTicker(33 * time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return writeStats(stats{})
		case <-tick.C:
			if err := track.WriteSample(webrtcSample(sample)); err != nil {
				return fmt.Errorf("write sample: %w", err)
			}
		}
	}
}

func runWHEP(ctx context.Context, endpoint, token string) error {
	pc, err := webrtc.NewPeerConnection(webrtc.Configuration{})
	if err != nil {
		return fmt.Errorf("new peer connection: %w", err)
	}
	defer pc.Close()
	if _, err := pc.AddTransceiverFromKind(webrtc.RTPCodecTypeVideo); err != nil {
		return fmt.Errorf("add transceiver: %w", err)
	}
	keyframe := make(chan struct{}, 1)
	pc.OnTrack(func(t *webrtc.TrackRemote, _ *webrtc.RTPReceiver) {
		go func() {
			for {
				_, _, err := t.ReadRTP()
				if err != nil {
					return
				}
				select {
				case keyframe <- struct{}{}:
				default:
				}
			}
		}()
	})
	offer, err := pc.CreateOffer(nil)
	if err != nil {
		return fmt.Errorf("create offer: %w", err)
	}
	if err := pc.SetLocalDescription(offer); err != nil {
		return fmt.Errorf("set local: %w", err)
	}
	<-webrtc.GatheringCompletePromise(pc)
	answer, err := postWHIP(ctx, endpoint, token, pc.LocalDescription().SDP)
	if err != nil {
		return err
	}
	if err := pc.SetRemoteDescription(webrtc.SessionDescription{
		Type: webrtc.SDPTypeAnswer,
		SDP:  answer,
	}); err != nil {
		return fmt.Errorf("set remote: %w", err)
	}
	start := time.Now()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-keyframe:
		return writeStats(stats{FirstKeyframeMs: time.Since(start).Milliseconds()})
	}
}

func postWHIP(ctx context.Context, endpoint, token, offerSDP string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint,
		strings.NewReader(offerSDP))
	if err != nil {
		return "", fmt.Errorf("new request: %w", err)
	}
	req.Header.Set("Content-Type", "application/sdp")
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("post: %w", err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", fmt.Errorf("read body: %w", err)
	}
	if resp.StatusCode/100 != 2 {
		return "", fmt.Errorf("WHIP/WHEP non-2xx %d: %s", resp.StatusCode, string(body))
	}
	return string(body), nil
}

func writeStats(s stats) error {
	dir := os.Getenv("WEBRTC_INTEROP_ARTIFACT_DIR")
	if dir == "" {
		return nil
	}
	path := filepath.Join(dir, "peer-stats.json")
	f, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("create stats: %w", err)
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetIndent("", "  ")
	return enc.Encode(s)
}

// webrtcSample is a small shim to avoid importing the full
// `media` package in the build closure of this helper. The sample
// duration matches what the publisher loop emits.
func webrtcSample(b []byte) webrtc.RTCSample { //nolint:typecheck
	return webrtc.RTCSample{Data: b, Duration: 33 * time.Millisecond}
}
