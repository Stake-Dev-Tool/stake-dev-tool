// UI regression fixtures only: exercises real Svelte UI, not server authorization.
// Start Vite on 5193; PLAYWRIGHT_MODULE may point to an installed Playwright package.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const base = process.env.SDT_TEST_URL || 'http://127.0.0.1:5193';
const frontId = '00000000-0000-4000-8000-000000000001';
async function fixture(browser, { role = 'member', fronts = 'ok', math = true } = {}) {
 const page = await browser.newPage({viewport:{width:1168,height:940}});
 let frontState = fronts;
 await page.route('**/api/**', async route => {
  const path = new URL(route.request().url()).pathname;
  const workspace = {id:'test-workspace',slug:'fixture',name:'Fixture team'};
  const user = {id:'test-user',display_name:'Test member',email:'fixture@example.invalid',email_verified:true};
  let data;
  if(path==='/api/auth/me') data=user;
  else if(path==='/api/workspaces') data=[{workspace,role}];
  else if(path==='/api/workspaces/fixture') data={workspace,role,members:[{user_id:user.id,role}]};
  else if(path.endsWith('/billing')) data={enabled:false};
  else if(path.endsWith('/games')) data=[{id:'test-game',slug:'example',name:'Fixture game',head_number:math?6:null,revisions_count:math?2:0}];
  else if(path.endsWith('/revisions')) data=math?[6,4].map(number=>({number,message:'Math fixture '+number,created_at:`2026-09-0${number}T12:00:00Z`,files_count:1,total_size:2})):[];
  else if(path.endsWith('/front-bundles/check')) data={missing:[]};
  else if(path.endsWith('/front-bundles') && route.request().method()==='POST') { frontState='ok'; data={id:frontId,created_at:'2026-09-10T12:00:00Z'}; }
  else if(path.endsWith('/front-bundles')) {
   if(frontState==='error') return route.fulfill({status:503,json:{error:{code:'fixture_unavailable',message:'Front service unavailable'}}});
   data=frontState==='empty'?[]:[{id:frontId,created_at:'2026-09-10T12:00:00Z',files_count:2,total_size:15,is_latest:true}];
  }
  else if(path.endsWith('/revisions/6')) data={number:6,message:'Math fixture 6',created_at:'2026-09-06T12:00:00Z',files:[{path:'index.json',hash:'a'.repeat(64),size:2}],stats:null};
  else if(path.endsWith('/shares')) data=[];
  else return route.fulfill({status:404,json:{error:{code:'fixture_unhandled',message:path}}});
  return route.fulfill({status:200,json:data});
 });
 return {page, recover:()=>{frontState='ok';}};
}
const tests = [];
function test(name, run) { tests.push({name,run}); }
test('overview shows typed independent builds and native downloads without Share', async browser => {
 const {page}=await fixture(browser);
 await page.goto(base+'/w/fixture/g/example');
 await page.getByRole('heading',{name:'Build history',exact:true}).waitFor({timeout:5000});
 const table=page.getByRole('table',{name:'Build history',exact:true});
 await table.getByText(frontId,{exact:true}).waitFor();
 const rows=table.locator('tbody tr');
 assert.equal(await rows.count(),3);
 assert.match(await rows.first().innerText(),/Front/);
 assert.match(await rows.nth(1).innerText(),/Math/);
 assert.match(await table.innerText(),/rev 6/);
 const download=table.getByRole('link',{name:'Download front build'});
 assert.equal(await download.getAttribute('href'),`/api/workspaces/fixture/games/example/front-bundles/${frontId}/download`);
 assert.notEqual(await download.getAttribute('download'),null);
 assert.equal(await table.getByRole('link',{name:'Download math build'}).count(),2);
 assert.equal(await page.getByRole('button',{name:'Delete game',exact:true}).count(),0);
 assert.equal(await page.locator('select option').count(),4,'compare contains only the two math revisions');
 await page.close();
});
test('revision identifies math files and exposes independent front builds directly', async browser => {
 const {page}=await fixture(browser);
 await page.goto(base+'/w/fixture/g/example/r/6');
 await page.getByText('Math revision 6',{exact:true}).waitFor({timeout:5000});
 await page.getByRole('heading',{name:'Frontend builds',exact:true}).waitFor();
 await page.getByText('These are game-level frontend builds, not attachments to this math revision.',{exact:true}).waitFor();
 await page.getByRole('link',{name:'Download front build',exact:true}).waitFor();
 const math=page.getByRole('link',{name:'Download math build',exact:true});
 assert.equal(await math.getAttribute('href'),'/api/workspaces/fixture/games/example/revisions/6/download');
 assert.equal(await page.getByRole('button',{name:'Delete',exact:true}).count(),0);
 await page.getByRole('link',{name:'All game builds'}).click();
 await page.getByRole('heading',{name:'Build history',exact:true}).waitFor();
 await page.close();
});
test('front upload uses an explicit label and refreshes build history', async browser => {
 const fs=require('node:fs'); const os=require('node:os'); const path=require('node:path');
 const dir=fs.mkdtempSync(path.join(os.tmpdir(),'sdt-front-fixture-'));
 fs.writeFileSync(path.join(dir,'index.html'),'<!doctype html><title>Fixture</title>');
 const {page}=await fixture(browser,{fronts:'empty'});
 try {
  await page.goto(base+'/w/fixture/g/example');
  await page.getByRole('button',{name:'Upload math / front',exact:true}).click({timeout:5000});
  await page.locator('input[type=file]').setInputFiles(dir);
  await page.getByRole('button',{name:'Upload front build',exact:true}).click();
  await page.getByRole('table',{name:'Build history'}).getByText(frontId,{exact:true}).waitFor({timeout:5000});
 } finally {await page.close();fs.rmSync(dir,{recursive:true,force:true});}
});
test('build deep links return from Share on the reused game page', async browser => {
 const {page}=await fixture(browser);
 await page.goto(base+'/w/fixture/g/example#share');
 await page.getByRole('heading',{name:'Share',exact:true}).waitFor();
 await page.evaluate(()=>{location.hash='front';});
 await page.getByRole('heading',{name:'Build history',exact:true}).waitFor({timeout:5000});
 await page.getByRole('link',{name:'Download front build',exact:true}).waitFor();
 await page.evaluate(()=>{location.hash='share';});
 await page.getByRole('heading',{name:'Share',exact:true}).waitFor();
 await page.evaluate(()=>{location.hash='builds';});
 await page.getByRole('heading',{name:'Build history',exact:true}).waitFor();
 await page.close();
});
test('chronology compares timestamp instants rather than ISO text', async browser => {
 const {page}=await fixture(browser);
 await page.route('**/front-bundles',route=>route.fulfill({json:[{id:frontId,created_at:'2026-09-06T13:00:00+02:00',files_count:2,total_size:15,is_latest:true}]}));
 await page.goto(base+'/w/fixture/g/example');
 const table=page.getByRole('table',{name:'Build history'});
 await table.getByText(frontId,{exact:true}).waitFor();
 assert.match(await table.locator('tbody tr').first().innerText(),/rev 6/);
 await page.close();
});
test('revision frontend preview stays compact and explicitly math-only', async browser => {
 const {page}=await fixture(browser);
 await page.route('**/front-bundles',route=>route.fulfill({json:[1,2,3,4].map(n=>({id:frontId.slice(0,-1)+n,created_at:`2026-09-${14-n}T12:00:00Z`,files_count:2,total_size:15,is_latest:n===1}))}));
 await page.goto(base+'/w/fixture/g/example/r/6');
 await page.getByRole('link',{name:'Download front build',exact:true}).first().waitFor();
 assert.equal(await page.getByRole('link',{name:'Download front build',exact:true}).count(),1,'detail previews only newest frontend; full list is on game page');
 await page.getByText('This revision contains math files only; frontend builds are versioned separately.',{exact:true}).waitFor();
 await page.getByText('Showing the latest front build. Open All game builds for more.',{exact:true}).waitFor();
 await page.close();
});
async function navigate(page, path) {
 await page.evaluate(path=>{const a=document.createElement('a');a.href=path;document.body.append(a);a.click();a.remove();},path);
}
test('late overview response cannot overwrite a different game', async browser => {
 const {page}=await fixture(browser);
 let release; const held=new Promise(resolve=>release=resolve);
 let started; const requested=new Promise(resolve=>started=resolve);
 await page.route('**/games/example/revisions',async route=>{started();await held;await route.fulfill({json:[{number:999,message:'STALE MATH',created_at:'2026-09-12T12:00:00Z',files_count:1,total_size:2}]});});
 await page.goto(base+'/w/fixture/g/example'); await requested;
 await navigate(page,'/w/fixture/g/other');
 await page.getByRole('heading',{name:'Build history',exact:true}).waitFor();
 release(); await page.waitForTimeout(200);
 assert.equal(await page.getByText('STALE MATH',{exact:true}).count(),0);
 assert.match(await page.getByRole('table',{name:'Build history'}).innerText(),/rev 6/);
 await page.close();
});
test('late revision poll and old admin role cannot leak across workspaces', async browser => {
 const {page}=await fixture(browser);
 let release; const held=new Promise(resolve=>release=resolve);
 let started; const polling=new Promise(resolve=>started=resolve); let calls=0;
 await page.route('**/api/workspaces/fixture',async route=>{await held;await route.fulfill({json:{workspace:{slug:'fixture',name:'Old'},role:'admin',members:[]}});});
 await page.route('**/api/workspaces/other',route=>route.fulfill({json:{workspace:{slug:'other',name:'Other'},role:'member',members:[]}}));
 await page.route('**/workspaces/fixture/games/example/revisions/6',async route=>{
  calls++; if(calls>1){started();await held;}
  await route.fulfill({json:{number:6,message:calls>1?'STALE DETAIL':'Polling math',created_at:'2026-09-06T12:00:00Z',files:[],stats:{status:'pending',modes:[]}}});
 });
 await page.goto(base+'/w/fixture/g/example/r/6'); await polling;
 await navigate(page,'/w/other/g/example/r/6');
 await page.getByRole('heading',{name:'Math fixture 6',exact:true}).waitFor();
 release(); await page.waitForTimeout(200);
 assert.equal(await page.getByRole('heading',{name:'STALE DETAIL',exact:true}).count(),0);
 assert.equal(await page.getByRole('button',{name:'Delete',exact:true}).count(),0);
 await page.close();
});
test('front-only game distinguishes missing math from available front builds', async browser => {
 const {page}=await fixture(browser,{math:false});
 await page.goto(base+'/w/fixture/g/example');
 await page.getByRole('heading',{name:'No math revisions yet',exact:true}).waitFor({timeout:5000});
 await page.getByRole('link',{name:'Download front build',exact:true}).waitFor();
 assert.equal(await page.getByRole('link',{name:'Download math build',exact:true}).count(),0);
 assert.equal(await page.locator('select').count(),0);
 await page.close();
});
test('front errors are retryable without hiding math on either page', async browser => {
 for(const suffix of ['', '/r/6']) {
  const {page,recover}=await fixture(browser,{fronts:'error'});
  await page.goto(base+'/w/fixture/g/example'+suffix);
  await page.getByRole('alert').filter({hasText:'Could not load front builds'}).waitFor();
  assert((await page.getByRole('link',{name:'Download math build',exact:true}).count())>0);
  assert.equal(await page.getByText(/No front builds yet/).count(),0);
  recover();await page.getByRole('button',{name:'Retry front builds',exact:true}).click();
  await page.getByRole('link',{name:'Download front build',exact:true}).waitFor();
  await page.close();
 }
});
test('owner and admin retain management while builds stay visible', async browser => {
 for(const role of ['owner','admin']) {
  const {page}=await fixture(browser,{role});
  await page.goto(base+'/w/fixture/g/example');
  await page.getByRole('button',{name:'Delete game',exact:true}).waitFor();
  await page.getByRole('link',{name:'Download front build',exact:true}).waitFor();
  await page.getByText('Math rev 6',{exact:true}).waitFor({timeout:5000});
  await page.getByRole('button',{name:'Compare math',exact:true}).waitFor();
  await navigate(page,'/w/fixture/g/example/r/6');
  await page.getByRole('button',{name:'Delete',exact:true}).waitFor();
  await page.getByRole('link',{name:'Download front build',exact:true}).waitFor();
  await page.close();
 }
});
(async()=>{
 const browser=await chromium.launch({headless:true}); let failed=0;
 try { for(const {name,run} of tests) {try {await run(browser); console.log('PASS '+name);} catch(e) {failed++;console.error('FAIL '+name+'\n'+e.stack);} } }
 finally {await browser.close();}
 console.log(`${tests.length-failed}/${tests.length} passed (local API fixtures)`);process.exitCode=failed?1:0;
})();
