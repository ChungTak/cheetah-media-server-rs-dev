// Phase 06 Playwright scaffold — drives a Chrome instance through
// a WHIP publish + WHEP play round trip against a running
// cheetah/ZLM media server. The script is intentionally short:
// production interop runs use a richer stats schema, but the goal
// here is to demonstrate the wiring (page.evaluate + getStats) so
// the nightly job can iterate without rewrites.
//
// Run locally:
//   npx playwright test dev-docs/plans-27-webrtc-zlm2/interop-playwright/whip-whep.spec.ts
//
// Required env (also documented in interop-runner.md):
//   WEBRTC_INTEROP_ZLM_WHIP_URL=http://127.0.0.1:8080/index/api/whip?...
//   WEBRTC_INTEROP_ZLM_WHEP_URL=http://127.0.0.1:8080/index/api/whep?...
//   WEBRTC_INTEROP_ARTIFACT_DIR=/tmp/playwright-artifacts (optional)

import { test, expect, type Page } from '@playwright/test';
import * as fs from 'fs';
import * as path from 'path';

interface PeerStats {
  bytesSent: number;
  bytesReceived: number;
  nacksReceived: number;
  packetsLost: number;
  framesDecoded: number | null;
  iceState: string;
  dtlsState: string;
}

const WHIP_URL = process.env.WEBRTC_INTEROP_ZLM_WHIP_URL;
const WHEP_URL = process.env.WEBRTC_INTEROP_ZLM_WHEP_URL;
const ARTIFACT_DIR = process.env.WEBRTC_INTEROP_ARTIFACT_DIR ?? '/tmp/playwright-artifacts';

test.skip(!WHIP_URL || !WHEP_URL, 'WHIP/WHEP env vars not set');

async function captureStats(page: Page, label: string): Promise<PeerStats> {
  const stats = await page.evaluate(async () => {
    const w = window as unknown as { __pc__?: RTCPeerConnection };
    const pc = w.__pc__;
    if (!pc) {
      throw new Error('no peer connection on window');
    }
    const report = await pc.getStats();
    const out: Record<string, number | string | null> = {
      bytesSent: 0,
      bytesReceived: 0,
      nacksReceived: 0,
      packetsLost: 0,
      framesDecoded: null,
      iceState: pc.iceConnectionState,
      dtlsState: 'unknown',
    };
    report.forEach((r: any) => {
      if (r.type === 'outbound-rtp') {
        out.bytesSent = (out.bytesSent as number) + (r.bytesSent ?? 0);
        out.nacksReceived =
          (out.nacksReceived as number) + (r.nackCount ?? 0);
      }
      if (r.type === 'inbound-rtp') {
        out.bytesReceived = (out.bytesReceived as number) + (r.bytesReceived ?? 0);
        out.packetsLost = (out.packetsLost as number) + (r.packetsLost ?? 0);
        if (r.framesDecoded !== undefined) {
          out.framesDecoded = r.framesDecoded;
        }
      }
      if (r.type === 'transport') {
        out.dtlsState = r.dtlsState ?? 'unknown';
      }
    });
    return out;
  });
  fs.mkdirSync(ARTIFACT_DIR, { recursive: true });
  fs.writeFileSync(
    path.join(ARTIFACT_DIR, `${label}-stats.json`),
    JSON.stringify(stats, null, 2),
  );
  return stats as unknown as PeerStats;
}

test('WHIP publish round trip captures stats', async ({ page }) => {
  await page.goto('about:blank');
  await page.evaluate(async (whipUrl) => {
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: 320, height: 240 },
      audio: false,
    });
    const pc = new RTCPeerConnection();
    (window as unknown as { __pc__?: RTCPeerConnection }).__pc__ = pc;
    stream.getTracks().forEach((t) => pc.addTrack(t, stream));
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await new Promise<void>((resolve) => {
      pc.onicegatheringstatechange = () => {
        if (pc.iceGatheringState === 'complete') resolve();
      };
    });
    const response = await fetch(whipUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/sdp' },
      body: pc.localDescription!.sdp,
    });
    if (!response.ok) {
      throw new Error(`WHIP non-2xx: ${response.status}`);
    }
    const answer = await response.text();
    await pc.setRemoteDescription({ type: 'answer', sdp: answer });
  }, WHIP_URL!);
  // Allow time for ICE/DTLS to converge.
  await page.waitForTimeout(3000);
  const stats = await captureStats(page, 'whip');
  expect(stats.iceState === 'connected' || stats.iceState === 'completed').toBe(true);
});

test('WHEP play round trip captures first-frame stats', async ({ page }) => {
  await page.goto('about:blank');
  await page.evaluate(async (whepUrl) => {
    const pc = new RTCPeerConnection();
    (window as unknown as { __pc__?: RTCPeerConnection }).__pc__ = pc;
    pc.addTransceiver('video', { direction: 'recvonly' });
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await new Promise<void>((resolve) => {
      pc.onicegatheringstatechange = () => {
        if (pc.iceGatheringState === 'complete') resolve();
      };
    });
    const response = await fetch(whepUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/sdp' },
      body: pc.localDescription!.sdp,
    });
    if (!response.ok) {
      throw new Error(`WHEP non-2xx: ${response.status}`);
    }
    const answer = await response.text();
    await pc.setRemoteDescription({ type: 'answer', sdp: answer });
  }, WHEP_URL!);
  await page.waitForTimeout(5000);
  const stats = await captureStats(page, 'whep');
  expect(stats.bytesReceived).toBeGreaterThan(0);
});
