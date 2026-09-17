let chromium;
try {
  ({ chromium } = require("playwright"));
} catch {
  ({ chromium } = require(process.env.PLAYWRIGHT_PATH || "/home/kirky/.local/lib/node_modules/playwright"));
}

const SHOT = '/home/kirky/projects/CodeNexus/graph-viewer/gui-test-screenshots';
const consoleErrors = [];
const pageErrors = [];
let pass = 0, fail = 0;

function log(id, ok, detail) {
  ok ? pass++ : fail++;
  console.log(`[${id}] ${ok ? 'PASS' : 'FAIL'} — ${detail}`);
}

async function shot(page, name) {
  const p = `${SHOT}/${name}.png`;
  await page.screenshot({ path: p });
  console.log(`  shot: ${p}`);
}

(async () => {
  const browser = await chromium.launch({
    headless: true,
    args: ['--enable-unsafe-swiftshader', '--use-gl=angle', '--use-angle=swiftshader'],
  });
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await ctx.newPage();
  page.on('console', (m) => { if (m.type() === 'error') consoleErrors.push(m.text().slice(0, 200)); });
  page.on('pageerror', (e) => pageErrors.push(String(e).slice(0, 200)));

  await page.goto('http://localhost:5174/', { timeout: 15000 });
  await page.waitForLoadState('domcontentloaded');
  await page.getByRole('button', { name: /Demo Mode|演示模式/ }).click();
  await page.waitForTimeout(2800);

  /* V1: BUG-1 — fresh mount, NO unlock; hover raster finds tooltip, click opens panel
   * 漂移动画让节点持续移动：扫描命中后必须在同一点立即点击，
   * 否则数百毫秒的间隔足以让 ~10px 的球体漂出点击位置 */
  let hoverHit = null;
  let panelHit = null;
  outer1:
  for (const y of [482, 430, 530, 380, 580]) {
    for (let x = 600; x <= 1100; x += 15) {
      await page.mouse.move(x, y);
      await page.waitForTimeout(50);
      if (await page.locator('div.bg-background\\/95').count() > 0) {
        hoverHit = [x, y];
        await page.mouse.click(x, y); /* 原位立即点击 */
        await page.waitForTimeout(800);
        if (/OUTBOUND|INBOUND/.test(await page.locator('body').innerText())) {
          panelHit = hoverHit;
          break outer1;
        }
      }
    }
  }
  log('V1a hover tooltip on fresh mount', !!hoverHit, `tooltip at ${hoverHit}`);
  await shot(page, 'v1_hover_tooltip');
  log('V1b node click opens panel on fresh mount', !!panelHit, `panel opened at ${panelHit}`);
  await shot(page, 'v1_panel_open');

  /* V2: BUG-5 — aria labels */
  const closeAria = await page.locator('button[aria-label="Close"], button[aria-label="关闭"]').count();
  const reselectAria = await page.locator('header button[aria-label]').count();
  log('V2 aria-labels', closeAria > 0 && reselectAria > 0, `closeAria=${closeAria}, reselectAria=${reselectAria}`);

  /* V3: BUG-3/4 — trace flow in EN (default) */
  const link = page.locator('main button').filter({ hasText: 'query_nodes' }).last();
  const hadLink = await link.count();
  if (hadLink) { await link.click(); await page.waitForTimeout(900); }
  const traceBtn = page.getByRole('button', { name: 'Call Trace', exact: false });
  const hasTraceBtn = await traceBtn.count();
  log('V3a trace button (EN) in panel', hadLink > 0 && hasTraceBtn > 0, `link=${hadLink}, "Call Trace" btn=${hasTraceBtn}`);
  if (hasTraceBtn) {
    await traceBtn.first().click();
    await page.waitForTimeout(1200);
    const body = await page.locator('body').innerText();
    const badge = /Call Trace/.test(body);
    const hud = /Trace:/.test(body);
    log('V3b call trace activates', badge && hud, `header badge=${badge}, HUD trace line=${hud}`);
    await shot(page, 'v3_trace_active');
    // clear trace via badge ×
    await page.getByRole('button', { name: /Clear trace|清除追踪/ }).first().click();
    await page.waitForTimeout(400);
    const after = await page.locator('header').innerText();
    log('V3c trace cleared', !/Call Trace/.test(after), 'badge gone from header');
  }

  /* V4: BUG-4 — i18n live switch */
  await page.getByText('中', { exact: true }).first().click();
  await page.waitForTimeout(500);
  const zhBody = await page.locator('body').innerText();
  const zhTrace = /函数调用追踪/.test(zhBody);
  const zhPanel = /节点类型/.test(zhBody);
  log('V4 i18n switch to ZH', zhTrace && zhPanel, `trace btn ZH=${zhTrace}, panel ZH=${zhPanel}`);
  await shot(page, 'v4_zh_trace_btn');
  await page.getByText('EN', { exact: true }).first().click();
  await page.waitForTimeout(400);

  /* V5: BUG-2 — file tree visible and interactive */
  const ftTitle = page.getByText(/File Tree/).first();
  const ftBox = await ftTitle.boundingBox().catch(() => null);
  const visible = ftBox && ftBox.y > 0 && ftBox.y < 900;
  log('V5a file tree visible in viewport', !!visible, `File Tree title box=${JSON.stringify(ftBox)}`);
  const srcRow = page.locator('main button').filter({ hasText: 'src' }).first();
  const srcCount = await srcRow.count();
  if (srcCount) {
    await srcRow.click();
    await page.waitForTimeout(500);
    // expand shows children (directories/files); click a node entry
    const child = page.locator('main button').filter({ hasText: /main\.rs|graph\.rs|engine\.rs/ }).first();
    const hadChild = await child.count();
    if (hadChild) { await child.click(); await page.waitForTimeout(700); }
    const body = await page.locator('body').innerText();
    const cleared = await page.getByRole('button', { name: 'Clear', exact: true }).count();
    log('V5b file tree interactive', srcCount > 0 && hadChild > 0 && cleared > 0,
      `src row=${srcCount}, children=${hadChild}, highlight via file select (Clear btn)=${cleared}`);
  } else {
    log('V5b file tree interactive', false, 'src row not found');
  }
  await shot(page, 'v5_file_tree');

  /* V6: UI-OPT — HUD dedup: header has counts, canvas HUD does not duplicate */
  const headerText = await page.locator('header').innerText();
  const mainText = await page.locator('main').innerText();
  const headerHasCounts = /\d+ nodes \/ \d+ edges/.test(headerText);
  const mainDup = /\d+ nodes \/ \d+ edges/.test(mainText);
  log('V6 HUD dedup', headerHasCounts && !mainDup, `header counts=${headerHasCounts}, HUD duplicate=${mainDup}`);

  /* V7: regressions — Class toggle, file filter, back
   * header 计数格式为 "shown / total nodes / edges"（采样透明化），
   * demo 全量展示时省略 " / total" */
  await page.getByText('Class', { exact: true }).first().click();
  await page.waitForTimeout(700);
  const h1 = await page.locator('header').innerText();
  const dropped = /20 \/ 25 nodes \/ 16 edges/.test(h1) || /20 nodes \/ 16 edges/.test(h1);
  await page.getByText('Class', { exact: true }).first().click();
  await page.waitForTimeout(500);
  log('V7a Class toggle regression', dropped, `header after off: ${h1.match(/\d+ \/ \d+ nodes \/ \d+ edges|\d+ nodes \/ \d+ edges/)?.[0]}`);

  await page.getByPlaceholder(/Filter by file path/i).fill('parse');
  await page.waitForTimeout(600);
  const h2 = await page.locator('header').innerText();
  log('V7b file filter regression', /3 \/ 25 nodes \/ 2 edges/.test(h2) || /3 nodes \/ 2 edges/.test(h2), h2.match(/\d+ \/ \d+ nodes \/ \d+ edges|\d+ nodes \/ \d+ edges/)?.[0]);
  await page.getByPlaceholder(/Filter by file path/i).fill('');
  await page.waitForTimeout(400);

  /* V8: back to landing */
  await page.locator('header').getByText('CodeNexus').click();
  await page.waitForTimeout(600);
  const back = await page.getByText(/Drop .lbug file here/).count();
  log('V8 back to landing', back > 0, 'landing visible');

  /* V9: labels screen-size when zoomed deep */
  await page.getByRole('button', { name: /Demo Mode|演示模式/ }).click();
  await page.waitForTimeout(2500);
  for (let i = 0; i < 18; i++) {
    await page.mouse.move(850, 480);
    await page.mouse.wheel(0, -150);
    await page.waitForTimeout(90);
  }
  await page.waitForTimeout(1000);
  await shot(page, 'v9_zoomed_labels');

  console.log(`\n===== RESULT: ${pass} passed, ${fail} failed =====`);
  console.log('console errors:', JSON.stringify(consoleErrors));
  console.log('page errors:', JSON.stringify(pageErrors));
  await browser.close();
})().catch((e) => { console.error('FATAL', e); process.exit(1); });
