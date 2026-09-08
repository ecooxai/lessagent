const {JSDOM}=require('jsdom'),fs=require('fs'),assert=require('node:assert/strict');
const w=new JSDOM('<div id="host"></div>',{runScripts:'outside-only',pretendToBeVisual:true}).window;
const source=fs.readFileSync('web/app.js','utf8');
let focused=true;
w.document.hasFocus=()=>focused;
w.eval(source.slice(source.indexOf('function uiActive()'),source.indexOf('function resumeUi()')));
w.el=(tag,cls,text)=>{const n=w.document.createElement(tag);n.className=cls||'';if(text!==undefined)n.textContent=text;return n;};
w.button=(text,fn,cls)=>{const n=w.el('button',cls,text);n.onclick=fn;return n;};
w.notice=e=>{throw e;};
w.ResizeObserver=class{observe(){}disconnect(){}};
w.navigator.clipboard={writeText:async()=>{}};
w.eval(source.slice(source.indexOf('function iconButton('),source.indexOf('function createFileBrowser(')));
const wait=ms=>new Promise(r=>setTimeout(r,ms));
const cell=c=>[c,'Default','Default',false,false,false,false];
let revision=1,rows=[[cell('a')],[cell('b')]],reads=0,releases=[],inputs=[],deferScreen=false,releaseScreen,lastSignal;
w.act=async(name,args,signal)=>{
  if(name==='terminal_input'){inputs.push(args.text);await new Promise(r=>releases.push(r));return {ok:true};}
  if(name==='terminal_resize')return {ok:true};
  assert.equal(name,'terminal_screen');reads++;lastSignal=signal;
  assert.equal(args.compact,true);assert.equal(args.wait_ms,25000);
  const data=args.revision===revision?{unchanged:true,exited:false}:{revision,rows,cursor:[0,0],exited:false,scrollback:0};
  if(deferScreen){deferScreen=false;await new Promise(r=>releaseScreen=r);}
  return data;
};
(async()=>{const host=w.document.querySelector('#host'),cleanup=w.mountTerminal(host,'t','w');try{
  await wait(30);const screen=host.querySelector('pre'),first=screen.children[0],second=screen.children[1];
  assert.equal(screen.textContent,'ab');
  rows=[[cell('x')],[cell('b')]];revision++;
  w.document.dispatchEvent(new w.Event('lessagent:resume'));await wait(30);
  assert.notEqual(screen.children[0],first);assert.equal(screen.children[1],second,'unchanged row reuses DOM');
  const input=host.querySelector('textarea');
  for(const value of ['1','2','3']){input.value=value;input.oninput({});}
  assert.deepEqual(inputs,['1']);releases.shift()();await wait(5);assert.deepEqual(inputs,['1','23']);releases.shift()();await wait(30);
  focused=false;const before=reads;rows=[[cell('z')],[cell('b')]];revision++;
  await wait(550);assert.equal(reads,before,'unfocused surface does not fetch');assert.equal(screen.textContent,'xb');
  focused=true;w.document.dispatchEvent(new w.Event('lessagent:resume'));await wait(30);assert.equal(screen.textContent,'zb','focus catches up');
  host.hidden=true;const hiddenReads=reads;await wait(550);assert.equal(reads,hiddenReads,'hidden dock does not fetch');
  host.hidden=false;deferScreen=true;rows=[[cell('q')]];revision++;
  w.document.dispatchEvent(new w.Event('lessagent:resume'));await wait(10);focused=false;releaseScreen();await wait(20);assert.equal(screen.textContent,'zb','in-flight response cannot paint unfocused UI');
  focused=true;w.document.dispatchEvent(new w.Event('lessagent:resume'));await wait(30);assert.equal(screen.textContent,'q');
  deferScreen=true;w.document.dispatchEvent(new w.Event('lessagent:resume'));await wait(10);
  focused=false;w.dispatchEvent(new w.Event('blur'));assert.equal(lastSignal.aborted,true,'blur cancels waiting request');
  releaseScreen();await wait(20);
  cleanup();const cleanedReads=reads;await wait(60);assert.equal(reads,cleanedReads);
  console.log('PASS: incremental rows, ordered/coalesced input, focus/hidden suspension, resume and disposal');
}finally{cleanup();w.close();}})().catch(e=>{console.error(e);process.exitCode=1;});
