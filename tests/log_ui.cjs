const {JSDOM}=require('jsdom');
const fs=require('fs'),assert=require('node:assert/strict');
const w=new JSDOM('<button id="logs-nav"></button><section id="view"></section>',{runScripts:'outside-only',url:'http://127.0.0.1/'}).window;
const source=fs.readFileSync('web/app.js','utf8');
const start=source.indexOf('const LOG_PREVIEW_LIMIT');
const end=source.indexOf('function renderFiles()');
assert.ok(start>=0&&end>start,'log UI source block not found');
const timers=[];
w.setTimeout=(fn,delay)=>{const timer={fn,delay,cleared:false};timers.push(timer);return timer;};
w.clearTimeout=timer=>{if(timer)timer.cleared=true;};
w.eval(`
const $=s=>document.querySelector(s);
const el=(tag,cls,text)=>{const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n;};
let state={logs:[]},lastLogMarker=null,logManualPaused=false,logPauseUntil=0,logFreshTimer,logRecentTimer,logResumeTimer;
window.__polls=0;
async function poll(){window.__polls++;}
function button(text,fn,cls){const b=el('button',cls,text);b.type='button';b.onclick=fn;return b;}
function toolbar(title,...actions){const t=el('div','toolbar');t.append(el('h2','',title));const row=el('div','row');row.append(...actions);t.append(row);return t;}
function ui(){return {active:'logs',tabs:[{id:'logs',kind:'logs'}]};}
${source.slice(start,end)}
window.__logs={
  noteLogActivity,buildLogTaskSessions,renderLogs,pauseLogUpdates,handleLogScroll,resumeLogUpdates,
  handleLogPageKey,changeLogPage,setLogTaskFilter,
  setState:value=>state=value,getPause:()=>logPauseUntil,getManual:()=>logManualPaused,getPage:()=>logPage,getFilter:()=>logTaskFilter
};
`);
const api=w.__logs;
const timer=delay=>timers.findLast(t=>t.delay===delay&&!t.cleared);
const detail=(task,current,index)=>({
  tool:'bash',arguments:{command:`echo ${index}`},source:'MCP',agent:'ChatGPT',model:'GPT-5.6 Sol',
  main_task:task,current_task:current,progress:Math.min(100,40+index),quality:95,
  summary:`done ${index}; call ${index}`,current_timestamp:'2026-09-12T08:00:00-07:00',
  output:{exit_code:0,output:`ok-${index}`},
});
const log=(id,at,task='Alpha',current='Implement logs')=>({id:`log-${id}`,at,kind:'tool',message:`bash · ${id}`,details:detail(task,current,id)});
(async()=>{try{
  api.noteLogActivity({at:1,kind:'tool',message:'initial'});
  api.noteLogActivity({at:2,kind:'tool',message:'bash · demo'});
  assert.ok(w.document.querySelector('#logs-nav').classList.contains('log-activity-new'));
  timer(3000).fn();
  assert.ok(w.document.querySelector('#logs-nav').classList.contains('log-activity-recent'));
  timer(10000).fn();
  assert.equal(w.document.querySelector('#logs-nav').className,'');

  const base=Date.parse('2026-09-12T08:00:00-07:00');
  const grouped=[log(1,base,'Same','A'),log(2,base+5*60*1000,'Same','B'),log(3,base+16*60*1000,'Same','C'),log(4,base+17*60*1000,'Other','D')];
  const taskData=api.buildLogTaskSessions(grouped);
  assert.equal(taskData.sessions.length,3,'same task must split only when prior same-name call is >10 minutes old');
  assert.equal(taskData.sessions[0].items.length,2);
  assert.equal(taskData.sessions[1].items.length,1);
  assert.equal(taskData.meta.get('log-2').callIndex,2);
  assert.equal(taskData.meta.get('log-2').callCount,2);
  assert.equal(taskData.meta.get('log-2').duration,5*60*1000);
  assert.notEqual(taskData.sessions[0].id,taskData.sessions[1].id,'same-name sessions must include start time in identity');

  const entries=[];
  for(let i=0;i<26;i++) entries.push(log(i,base+i*30*1000,i<23?'Alpha':'Beta',`Step ${i}`));
  api.setState({logs:entries});
  api.renderLogs(true);
  let rows=[...w.document.querySelectorAll('.log-entry')];
  assert.equal(rows.length,20,'only 20 log items should be mounted per page');
  assert.match(rows[0].textContent,/bash · 25/,'newest item must be first');
  const firstMeta=rows[0].querySelector('.log-meta-line');
  assert.equal(rows[0].querySelector('.log-summary').firstElementChild,firstMeta,'metadata line must start each detailed log item');
  assert.match(firstMeta.textContent,/Beta \/ Step 25 \/ 65\/100 \/ 95\/100/,'metadata line must be date/main/current/progress/quality');
  assert.match(rows[0].querySelector('.log-task-stats').textContent,/Task 1m 0s · Call 3\/3/);
  assert.equal(rows[0].querySelector('.log-call-summary').textContent,'done 25; call 25');
  assert.match(rows[0].querySelector('.log-output').textContent,/ok-25/);
  assert.equal(w.document.querySelector('.log-page-info').textContent,'1/2');
  assert.equal(w.document.querySelectorAll('.log-page-button').length,2);
  assert.equal(w.document.querySelector('.log-filter-icon').getAttribute('aria-label'),'Filter by main task');

  // Right arrow goes to the older page; left returns to the newest page.
  let event=new w.KeyboardEvent('keydown',{key:'ArrowRight',bubbles:true,cancelable:true});
  assert.equal(api.handleLogPageKey(event),true);
  assert.equal(api.getPage(),1);
  rows=[...w.document.querySelectorAll('.log-entry')];
  assert.equal(rows.length,6);
  assert.match(rows[0].textContent,/bash · 5/);
  event=new w.KeyboardEvent('keydown',{key:'ArrowLeft',bubbles:true,cancelable:true});
  assert.equal(api.handleLogPageKey(event),true);
  assert.equal(api.getPage(),0);

  // Filter menu exposes task sessions and selecting one renders only that session.
  const filterButtons=[...w.document.querySelectorAll('.log-task-filter-menu button')];
  assert.equal(filterButtons[0].querySelector('.log-task-filter-name').textContent,'All main tasks');
  assert.equal(filterButtons[0].querySelector('.log-task-filter-count').textContent,'26 calls');
  const beta=filterButtons.find(b=>b.querySelector('.log-task-filter-name')?.textContent==='Beta');
  assert.ok(beta,'Beta task should be listed in filter');
  assert.equal(beta.querySelector('.log-task-filter-count').textContent,'3 calls');
  assert.equal(beta.textContent,'Beta3 calls','filter rows should only contain task name and call count');
  assert.ok(!beta.textContent.includes('2026'),'filter rows must not include session date');
  await beta.onclick();
  assert.ok(api.getFilter());
  rows=[...w.document.querySelectorAll('.log-entry')];
  assert.equal(rows.length,3);
  assert.ok(rows.every(row=>row.querySelector('.log-meta-line').textContent.includes('Beta')));
  assert.match(w.document.querySelector('.toolbar h2').textContent,/Backend logs · Beta/);

  // Manual pause freezes updates; interaction pause is only two seconds.
  let pause=w.document.querySelector('.log-pause');
  await pause.onclick();
  assert.equal(api.getManual(),true);
  const newer=log(26,base+26*30*1000,'Beta','Step 26');
  api.setState({logs:[...entries,newer]});
  api.renderLogs();
  assert.equal([...w.document.querySelectorAll('.log-entry')].length,3,'manual pause should hold visible rows');
  await w.document.querySelector('.log-pause').onclick();
  assert.equal(api.getManual(),false);
  assert.equal([...w.document.querySelectorAll('.log-entry')].length,4);
  w.document.querySelector('.logs').dispatchEvent(new w.MouseEvent('click',{bubbles:true}));
  assert.ok(api.getPause()>Date.now());
  assert.ok(timer(2000),'interaction resume must use 2 seconds');
  timer(2000).fn();
  assert.equal(api.getPause(),0);

  const refresh=w.document.querySelector('.log-pause').nextElementSibling;
  await refresh.onclick();
  assert.equal(w.__polls,1);
  console.log('PASS: log metadata order, 10-minute task sessions, duration/counters, filter, 20-item pagination, arrow keys, 2s interaction pause');
}finally{w.close();}})().catch(e=>{console.error(e);process.exitCode=1;});
