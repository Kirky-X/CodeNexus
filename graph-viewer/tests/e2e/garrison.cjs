let chromium;
try {
  ({ chromium } = require("playwright"));
} catch {
  ({ chromium } = require(process.env.PLAYWRIGHT_PATH || "/home/kirky/.local/lib/node_modules/playwright"));
}

const SHOT = '/home/kirky/projects/CodeNexus/graph-viewer/gui-test-screenshots';
const errors = [];

(async () => {
  const browser = await chromium.launch({
    headless: true,
    args: ['--enable-unsafe-swiftshader', '--use-gl=angle', '--use-angle=swiftshader'],
  });
  const page = await (await browser.newContext({ viewport: { width: 1600, height: 1000 } })).newPage();
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text().slice(0, 200)); });
  page.on('pageerror', (e) => errors.push(String(e).slice(0, 200)));

  await page.goto('http://localhost:5174/', { timeout: 15000 });
  await page.waitForLoadState('domcontentloaded');

  /* 上传真实 garrison.lbug */
  await page.locator('input[type="file"]').first().setInputFiles(process.env.GARRISON_LBUG || '../.codenexus/garrison.lbug');
  await page.waitForTimeout(6000);
  await page.screenshot({ path: `${SHOT}/garrison_1_loaded.png` });

  const header = await page.locator('header').innerText();
  console.log('header:', header.replace(/\n/g, ' | ').slice(0, 160));

  /* hover 验证（真实数据上悬停出 tooltip）*/
  let hoverOk = false;
  outer:
  for (const y of [500, 440, 560, 380]) {
    for (let x = 700; x <= 1300; x += 18) {
      await page.mouse.move(x, y);
      await page.waitForTimeout(40);
      if (await page.locator('div.bg-background\\/95').count() > 0) { hoverOk = true; break outer; }
    }
  }
  console.log('hover tooltip on real data:', hoverOk);

  /* 点击节点打开面板 */
  let panelOk = false;
  for (let y = 380; y <= 640 && !panelOk; y += 14) {
    for (let x = 660; x <= 1340 && !panelOk; x += 14) {
      await page.mouse.click(x, y);
      await page.waitForTimeout(40);
      if (/OUTBOUND|INBOUND|出向|入向/.test(await page.locator('body').innerText())) panelOk = true;
    }
  }
  console.log('node click panel:', panelOk);
  await page.screenshot({ path: `${SHOT}/garrison_2_panel.png` });

  console.log('console/page errors:', JSON.stringify(errors.slice(0, 5)));
  await browser.close();
})().catch((e) => { console.error('FATAL', e); process.exit(1); });
