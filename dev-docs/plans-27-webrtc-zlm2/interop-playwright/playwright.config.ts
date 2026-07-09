// Phase 06 Playwright config scaffold.
//
// Run locally:
//   npm install --save-dev @playwright/test
//   npx playwright install chromium
//   npx playwright test --config dev-docs/plans-27-webrtc-zlm2/interop-playwright/playwright.config.ts

import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: __dirname,
  timeout: 60_000,
  use: {
    headless: true,
    launchOptions: {
      args: [
        '--no-sandbox',
        '--use-fake-ui-for-media-stream',
        '--use-fake-device-for-media-stream',
      ],
    },
  },
  reporter: [
    ['list'],
    [
      'json',
      {
        outputFile: process.env.WEBRTC_INTEROP_ARTIFACT_DIR
          ? `${process.env.WEBRTC_INTEROP_ARTIFACT_DIR}/playwright-report.json`
          : 'playwright-report.json',
      },
    ],
  ],
});
