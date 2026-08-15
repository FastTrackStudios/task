const { chromium } = require('@playwright/test');
(async () => {
  const b = await chromium.launch({ args: ['--no-sandbox','--disable-gpu'] });
  const p = await b.newPage();
  const logs = [];
  p.on('console', m => logs.push(`[${m.type()}] ${m.text()}`));
  p.on('pageerror', e => logs.push(`[PAGEERROR] ${e.message}`));
  p.on('requestfailed', r => logs.push(`[REQFAIL] ${r.url().slice(0,120)} :: ${r.failure()?.errorText}`));
  const url = process.argv[2] || 'https://tasks-dev.starcommand.live/';
  await p.goto(url, { waitUntil: 'load', timeout: 45000 }).catch(e => logs.push('[GOTO] '+e.message));
  await p.waitForTimeout(5000);
  const main = await p.$eval('#main', el => el.innerHTML).catch(() => '(no #main)');
  console.log('URL', url, '| MAIN_LEN', main.length);
  console.log('--- console/errors ('+logs.length+') ---');
  logs.slice(0, 40).forEach(l => console.log(l.slice(0, 280)));
  await b.close();
})();
