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
let state={logs:[]},lastLogMarker=null,logPauseUntil=0,logFreshTimer,logRecentTimer,logResumeTimer;
window.__polls=0;
async function poll(){window.__polls++;}
function button(text,fn,cls){const b=el('button',cls,text);b.type='button';b.onclick=fn;return b;}
function toolbar(title,...actions){const t=el('div','toolbar');t.append(el('h2','',title),...actions);return t;}
function ui(){return {active:'logs',tabs:[{id:'logs',kind:'logs'}]};}
${source.slice(start,end)}
window.__logs={noteLogActivity,logToolPreview,renderLogs,pauseLogUpdates,setState:value=>state=value,getPause:()=>logPauseUntil};
`);
const api=w.__logs;
const timer=delay=>timers.findLast(t=>t.delay===delay&&!t.cleared);
(async()=>{try{
  api.noteLogActivity({at:1,kind:'tool',message:'initial'});
  assert.equal(w.document.querySelector('#logs-nav').className,'');
  api.noteLogActivity({at:2,kind:'tool',message:'bash · demo'});
  assert.ok(w.document.querySelector('#logs-nav').classList.contains('log-activity-new'));
  timer(3000).fn();
  assert.ok(w.document.querySelector('#logs-nav').classList.contains('log-activity-recent'));
  timer(10000).fn();
  assert.equal(w.document.querySelector('#logs-nav').className,'');

  const command='x'.repeat(700);
  const oldLog={at:10,kind:'tool',message:'virtual_pointer · demo',details:{tool:'virtual_pointer',arguments:{action:'drag',x:10,y:20,to_x:30,to_y:40,button:'left'}}};
  const newLog={at:20,kind:'tool',message:'bash · demo',details:{tool:'bash',arguments:{command,wait_ms:1000}}};
  api.setState({logs:[oldLog,newLog]});
  api.renderLogs(true);
  let rows=[...w.document.querySelectorAll('.log-entry')];
  assert.match(rows[0].textContent,/bash · demo/,'latest log should render first');
  assert.match(rows[1].textContent,/virtual_pointer · demo/);
  assert.match(rows[1].querySelector('.log-preview').textContent,/action=drag/);
  assert.match(rows[1].querySelector('.log-preview').textContent,/x=10/);
  const preview=rows[0].querySelector('.log-preview').textContent;
  assert.equal(preview,'command: '+command.slice(0,500)+'…');
  assert.ok(rows[0].querySelector('.log-full').textContent.includes(command),'full detail should retain the whole command');

  api.pauseLogUpdates();
  assert.ok(api.getPause()>Date.now());
  const newest={at:30,kind:'tool',message:'virtual_keyboard · demo',details:{tool:'virtual_keyboard',arguments:{action:'key',key:'cmd+a'}}};
  api.setState({logs:[oldLog,newLog,newest]});
  api.renderLogs();
  rows=[...w.document.querySelectorAll('.log-entry')];
  assert.match(rows[0].textContent,/bash · demo/,'scroll pause should hold the visible list');
  timer(2000).fn();
  rows=[...w.document.querySelectorAll('.log-entry')];
  assert.match(rows[0].textContent,/virtual_keyboard · demo/,'list should catch up after two seconds');
  assert.match(rows[0].querySelector('.log-preview').textContent,/key=cmd\+a/);

  const refresh=[...w.document.querySelectorAll('button')].find(b=>b.textContent==='Refresh');
  await refresh.onclick();
  assert.equal(w.__polls,1,'manual refresh should fetch backend state');
  console.log('PASS: log UI activity colors, newest-first details, command expansion, and scroll pause');
}finally{w.close();}})().catch(e=>{console.error(e);process.exitCode=1;});
