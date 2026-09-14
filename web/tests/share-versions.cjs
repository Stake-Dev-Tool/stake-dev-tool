// Real Svelte UI with explicitly mocked API fixtures; not server authorization tests.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const base = process.env.SDT_TEST_URL || 'http://127.0.0.1:5194';
const frontId = 'abcde123-0000-4000-8000-000000000001';
const front = {id:frontId,created_at:'2026-09-10T12:00:00Z',files_count:2,total_size:15,is_latest:true};
const share = {id:'share-id',slug:'test-link',revision_number:null,front_bundle_id:null,created_at:front.created_at,max_concurrent_sessions:25,sessions_count:0,spins_count:0,active_sessions:0};
async function fixture(browser,{role='owner',fronts='ok',billing={enabled:false},shares=[]}={}) {
 const page=await browser.newPage(); page.setDefaultTimeout(5000);
 page.on('pageerror',error=>pageErrors.push(error.message));
 const state={fronts,posts:[],frontRequests:0};
 await page.route('**/api/**',async route=>{
  const path=new URL(route.request().url()).pathname;
  const workspace={id:'ws',slug:'fixture',name:'Fixture'}; let data;
  if(path==='/api/auth/me') data={id:'user',email:'fixture@example.invalid',email_verified:true};
  else if(path==='/api/workspaces') data=[{workspace,role}];
  else if(path==='/api/workspaces/fixture') data={workspace,role,members:[]};
  else if(path.endsWith('/billing')) data=billing;
  else if(path.endsWith('/games')) data=[{id:'game',slug:'example',name:'Fixture game',head_number:6,revisions_count:2}];
  else if(path.endsWith('/revisions')) data=[6,4].map(number=>({number,created_at:front.created_at,files_count:1,total_size:2}));
  else if(path.endsWith('/front-bundles')) {state.frontRequests++; if(state.fronts==='error') return route.fulfill({status:503,json:{error:{code:'unavailable',message:'Front service unavailable'}}}); data=state.fronts==='empty'?[]:[front];}
  else if(path.endsWith('/shares')) {
   if(route.request().method()==='POST') {const input=route.request().postDataJSON();state.posts.push(input);data={...share,...input,id:`share-${state.posts.length}`,slug:`test-link-${state.posts.length}`};}
   else data=shares;
  } else return route.fulfill({status:404,json:{error:{code:'fixture_unhandled',message:path}}});
  return route.fulfill({json:data});
 });
 await page.goto(base+'/w/fixture/g/example#share');
 await page.getByRole('heading',{name:'Share',exact:true}).waitFor();
 return {page,state};
}
const tests=[];const pageErrors=[];function test(name,run){tests.push({name,run});}
test('independent math/front selections send exact pinned/latest POST payloads',async browser=>{
 const {page,state}=await fixture(browser);
 for(const [math,frontValue] of [['4',frontId],['latest',frontId],['6','latest'],['latest','latest']]) {
  await page.getByRole('button',{name:'New share link',exact:true}).click();
  const mathSelect=page.getByLabel('Math version',{exact:true});
  const frontSelect=page.getByLabel('Front version',{exact:true});
  await frontSelect.locator(`option[value="${frontId}"]`).waitFor({state:'attached'});
  assert.match(await frontSelect.innerText(),/abcde123.*2026/);
  await mathSelect.selectOption(math);await frontSelect.selectOption(frontValue);
  await page.getByRole('button',{name:'Create share link',exact:true}).click();
  await page.getByText('Share link created',{exact:false}).waitFor();
  await page.getByRole('button',{name:'New share link',exact:true}).waitFor();
  assert.deepEqual(state.posts.at(-1),{max_concurrent_sessions:25,revision_number:math==='latest'?null:Number(math),front_bundle_id:frontValue==='latest'?null:frontValue});
 }
 assert.equal(state.posts.length,4); assert.equal(state.frontRequests,4,'load on every open without expanding Front builds');
 await page.close();
});
test('front loading failure blocks creation until retry; empty is explicit',async browser=>{
 const {page,state}=await fixture(browser,{fronts:'error'});
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByRole('alert').filter({hasText:'Could not load front versions'}).waitFor();
 assert(await page.getByRole('button',{name:'Create share link',exact:true}).isDisabled());
 assert.equal(state.posts.length,0);
 state.fronts='empty';await page.getByRole('button',{name:'Retry front versions'}).click();
 await page.getByText('No front versions uploaded yet.',{exact:true}).waitFor();
 assert.equal(await page.getByLabel('Front version',{exact:true}).inputValue(),'latest');
 assert(await page.getByRole('button',{name:'Create share link',exact:true}).isEnabled());
 await page.close();
});
test('existing links expose both policies to members without write controls',async browser=>{
 const {page}=await fixture(browser,{role:'member',shares:[share,{...share,id:'pinned',slug:'pinned-link',revision_number:4,front_bundle_id:frontId}]});
 for(const text of ['Math · latest (tracking)','Front · latest (tracking)','Math · pinned rev 4','Front · pinned abcde123']) await page.getByText(text,{exact:true}).waitFor();
 for(const name of ['New share link','Revoke','Delete','Enable feedback']) assert.equal(await page.getByRole('button',{name,exact:true}).count(),0);
 await page.getByRole('button',{name:/Front builds/}).click();await page.getByRole('link',{name:'Download build files'}).waitFor();
 await page.close();
});
test('capped history warns that pins may become latest without banning valid pins',async browser=>{
 const {page}=await fixture(browser,{billing:{enabled:true,plan:'free',limits:{max_revisions_per_game:1,max_front_bundles_per_game:1,max_share_link_days:7,max_active_share_links:1},usage:{active_share_links:0}}});
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByText(/Pinned versions can be pruned/).waitFor();
 await page.getByLabel('Math version',{exact:true}).selectOption('4');
 await page.getByLabel('Front version',{exact:true}).selectOption(frontId);
 assert(await page.getByRole('button',{name:'Create share link',exact:true}).isEnabled());
 await page.close();
});
async function navigate(page,path){await page.evaluate(path=>{const a=document.createElement('a');a.href=path;document.body.append(a);a.click();a.remove();},path);}
test('cancel/reopen invalidates delayed front choices and disables loading submit',async browser=>{
 const {page}=await fixture(browser);let release;const held=new Promise(r=>release=r);let calls=0;
 await page.route('**/front-bundles',async route=>{if(++calls===1){await held;await route.fulfill({json:[{...front,id:'deadbeef-0000-4000-8000-000000000001'}]});}else await route.fulfill({json:[front]});});
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByRole('status').filter({hasText:'Loading front versions'}).waitFor();
 assert(await page.getByRole('button',{name:'Create share link',exact:true}).isDisabled());
 await page.getByRole('button',{name:'Cancel',exact:true}).click();
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByLabel('Front version',{exact:true}).selectOption(frontId);
 release();await page.waitForTimeout(250);
 assert.equal(await page.getByLabel('Front version',{exact:true}).inputValue(),frontId);
 assert.equal(await page.locator('option[value^="deadbeef"]').count(),0);
 await page.close();
});
test('old admin role cannot leak into a different workspace Share panel',async browser=>{
 const {page}=await fixture(browser);let release;const held=new Promise(r=>release=r);
 await page.route('**/api/workspaces/fixture',async route=>{await held;await route.fulfill({json:{workspace:{slug:'fixture',name:'Old'},role:'admin',members:[]}});});
 await page.route('**/api/workspaces/other',route=>route.fulfill({json:{workspace:{slug:'other',name:'Other'},role:'member',members:[]}}));
 await navigate(page,'/w/fixture/g/second#share');
 await page.waitForTimeout(150);
 await navigate(page,'/w/other/g/example#share');
 await page.getByRole('heading',{name:'Share',exact:true}).waitFor();
 release();await page.waitForTimeout(250);
 assert.equal(await page.getByRole('button',{name:'New share link',exact:true}).count(),0);
 await page.close();
});
test('failed create preserves both pins and surfaces server errors without retrying latest',async browser=>{
 const {page,state}=await fixture(browser,{role:'admin'});
 const attempts=[];
 await page.route('**/shares',route=>{if(route.request().method()!=='POST') return route.fallback();attempts.push(route.request().postDataJSON());return route.fulfill({status:422,json:{error:{code:'bundle_not_found',message:'missing'}}});});
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByLabel('Math version',{exact:true}).selectOption('4');await page.getByLabel('Front version',{exact:true}).selectOption(frontId);
 await page.getByRole('button',{name:'Create share link',exact:true}).click();
 await page.getByText('That front bundle no longer exists — reload and retry.',{exact:true}).waitFor();
 assert.equal(await page.getByLabel('Math version',{exact:true}).inputValue(),'4');assert.equal(await page.getByLabel('Front version',{exact:true}).inputValue(),frontId);
 assert.deepEqual(attempts,[{revision_number:4,front_bundle_id:frontId,max_concurrent_sessions:25}]);
 assert.equal(state.posts.length,0);await page.close();
});
test('a cancelled pinned selection missing on reopen cannot silently create latest',async browser=>{
 const {page,state}=await fixture(browser);
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByLabel('Front version',{exact:true}).selectOption(frontId);
 await page.getByRole('button',{name:'Cancel',exact:true}).click();state.fronts='empty';
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByText('No front versions uploaded yet.',{exact:true}).waitFor();
 await page.getByText('The selected front version is not in the loaded versions. Choose another version.',{exact:true}).waitFor();
 assert(await page.getByRole('button',{name:'Create share link',exact:true}).isDisabled());
 await page.getByLabel('Front version',{exact:true}).selectOption('latest');
 await page.getByRole('button',{name:'Create share link',exact:true}).click();
 await page.getByRole('button',{name:'New share link',exact:true}).waitFor();
 assert.deepEqual(state.posts,[{max_concurrent_sessions:25,revision_number:null,front_bundle_id:null}]);await page.close();
});
test('front selector discloses the API newest-50 limit',async browser=>{
 const {page}=await fixture(browser);
 await page.route('**/front-bundles',route=>route.fulfill({json:Array.from({length:50},(_,n)=>({...front,id:`abcde123-0000-4000-8000-${String(n).padStart(12,'0')}`}))}));
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByText('Showing the newest 50 front versions; older versions are not listed.',{exact:true}).waitFor();
 assert.equal(await page.getByLabel('Front version',{exact:true}).locator('option').count(),51);
 await page.close();
});
test('delayed create response cannot populate another game',async browser=>{
 const {page}=await fixture(browser);let release;const held=new Promise(r=>release=r);let requested;const started=new Promise(r=>requested=r);
 await page.route('**/games/example/shares',async route=>{if(route.request().method()!=='POST')return route.fallback();requested();await held;return route.fulfill({json:{...share,slug:'stale-created-link'}});});
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 await page.getByLabel('Front version',{exact:true}).selectOption(frontId);
 await page.getByRole('button',{name:'Create share link',exact:true}).click();await started;
 await navigate(page,'/w/fixture/g/other#share');
 await page.getByRole('button',{name:'New share link',exact:true}).waitFor();
 release();await page.waitForTimeout(250);
 assert.equal(await page.getByText('stale-created-link',{exact:false}).count(),0);
 assert.equal(await page.getByText('Share link created',{exact:false}).count(),0);
 await page.getByRole('button',{name:'New share link',exact:true}).click();
 assert.equal(await page.getByLabel('Math version',{exact:true}).inputValue(),'latest');
 await page.getByLabel('Front version',{exact:true}).selectOption('latest');
 assert(await page.getByRole('button',{name:'Create share link',exact:true}).isEnabled());
 await page.close();
});
(async()=>{const browser=await chromium.launch({headless:true});let failed=0;try{for(const {name,run} of tests){try{pageErrors.length=0;await run(browser);assert.deepEqual(pageErrors,[],'no browser runtime errors');console.log('PASS '+name);}catch(e){failed++;console.error('FAIL '+name+'\n'+e.stack);}}}finally{await browser.close();}console.log(`${tests.length-failed}/${tests.length} passed (local API fixtures)`);process.exitCode=failed?1:0;})();
