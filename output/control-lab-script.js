
'use strict';
document.title += ' ' + location.port;
document.querySelector('#scrollbox').className='scrollbox';
const canvas=document.querySelector('#canvas'),ctx=canvas.getContext('2d'),guide=document.querySelector('#guide'),g=guide.getContext('2d'),nameInput=document.querySelector('#name');
let drawing=false,sequence=0,clicks=0,strokes=0,plan={id:0,points:[]},lastPlan=-1;
const emit=p=>fetch('/event',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({seq:++sequence,...p})}).catch(()=>{});
function expectedPoint(x,y){
  const points=plan.points||[];
  if(!points.length)return null;
  let best={x:points[0][0],y:points[0][1],error:Math.hypot(x-points[0][0],y-points[0][1])};
  for(let i=1;i<points.length;i++){
    const a=points[i-1],b=points[i],dx=b[0]-a[0],dy=b[1]-a[1];
    const length=dx*dx+dy*dy;
    const t=length?Math.max(0,Math.min(1,((x-a[0])*dx+(y-a[1])*dy)/length)):0;
    const px=a[0]+t*dx,py=a[1]+t*dy,error=Math.hypot(x-px,y-py);
    if(error<best.error)best={x:px,y:py,error};
  }
  return best;
}
function record(e){
  const r=canvas.getBoundingClientRect();
  const expected=Number.isFinite(e.clientX)?expectedPoint(e.clientX,e.clientY):null;
  const p={type:e.type,x:e.clientX,y:e.clientY,canvasX:e.clientX-r.x,canvasY:e.clientY-r.y,
    button:e.button,buttons:e.buttons,delta:e.deltaY,meta:e.metaKey,ctrl:e.ctrlKey,alt:e.altKey,shift:e.shiftKey,
    key:e.key,code:e.code,detail:e.detail,target:e.target.id,trusted:e.isTrusted,focused:document.hasFocus(),
    value:e.target.value,planId:plan.id,expectedX:expected?.x,expectedY:expected?.y,errorCSSPixels:expected?.error};
  emit(p);
  if(Number.isFinite(e.clientX)){
    document.querySelector('#actual').textContent=e.clientX.toFixed(1)+' / '+e.clientY.toFixed(1);
    document.querySelector('#error').textContent=expected?expected.error.toFixed(2):'—';
    document.querySelector('#last').textContent=e.type+' · '+(e.isTrusted?'TRUSTED':'UNTRUSTED')+
      '\n'+((plan.points?.length||0)>1?'Nearest path: ':'Expected: ')+
      (expected?expected.x.toFixed(1)+' / '+expected.y.toFixed(1):'— / —');
  }
  return p;
}
for(const type of ['pointerdown','pointermove','pointerup','pointercancel','click','contextmenu','keydown','keyup','input'])document.addEventListener(type,record,true);
window.addEventListener('focus',()=>emit({type:'window-focus'}));window.addEventListener('blur',()=>emit({type:'window-blur'}));
ctx.strokeStyle='#286995';ctx.lineWidth=4;ctx.lineCap='round';ctx.lineJoin='round';
canvas.onpointerdown=e=>{if(e.button!==0)return;drawing=true;canvas.setPointerCapture(e.pointerId);ctx.beginPath();ctx.moveTo(e.offsetX,e.offsetY);};
canvas.onpointermove=e=>{if(!drawing)return;for(const sample of e.getCoalescedEvents?.()||[e]){ctx.lineTo(sample.clientX-canvas.getBoundingClientRect().left,sample.clientY-canvas.getBoundingClientRect().top);}ctx.stroke();};
canvas.onpointerup=e=>{if(e.button!==0)return;drawing=false;strokes++;emit({type:'stroke-complete',strokes});document.querySelector('#status').textContent=strokes+' complete strokes';};
canvas.onpointercancel=()=>{drawing=false;};canvas.oncontextmenu=e=>e.preventDefault();
document.querySelector('#click-test').onclick=()=>{clicks++;document.querySelector('#click-test').textContent='Click test · '+clicks;emit({type:'button-click',clicks});};
document.querySelector('#clear').onclick=()=>{ctx.clearRect(0,0,1000,600);nameInput.value='';emit({type:'clear'});};
window.addEventListener('wheel',e=>{record(e);emit({type:'wheel-detail',x:e.clientX,y:e.clientY,delta:e.deltaY,target:e.target.closest('.scrollbox')?.id||e.target.id,meta:e.metaKey,ctrl:e.ctrlKey,trusted:e.isTrusted,focused:document.hasFocus(),planId:plan.id});},{passive:true});
for(const id of ['scrollbox','scrollbox2'])document.getElementById(id).onscroll=e=>emit({type:'scrolled',target:id,top:e.target.scrollTop});
function ready(){const geometry={};for(const id of ['name','click-test','clear','range','canvas','scrollbox','scrollbox2']){const r=document.getElementById(id).getBoundingClientRect();geometry[id]={x:r.x,y:r.y,width:r.width,height:r.height};}emit({type:'ready',geometry,innerWidth,innerHeight,outerWidth,outerHeight,devicePixelRatio});}
function showPlan(value){plan=value;g.clearRect(0,0,1000,600);const r=canvas.getBoundingClientRect();if(plan.points?.length){g.strokeStyle='#82a9c0';g.lineWidth=1.5;g.setLineDash([5,5]);g.beginPath();plan.points.forEach((p,i)=>i?g.lineTo(p[0]-r.x,p[1]-r.y):g.moveTo(p[0]-r.x,p[1]-r.y));g.stroke();g.setLineDash([]);for(const p of plan.points){const x=p[0]-r.x,y=p[1]-r.y;g.beginPath();g.arc(x,y,9,0,2*Math.PI);g.moveTo(x-15,y);g.lineTo(x+15,y);g.moveTo(x,y-15);g.lineTo(x,y+15);g.stroke();}}emit({type:'plan-ready',planId:plan.id});}
document.querySelector('#set-expected').onclick=()=>showPlan({id:Date.now(),points:[[Number(document.querySelector('#expected-x').value),Number(document.querySelector('#expected-y').value)]]});
async function pollPlan(){try{const response=await fetch('/plan',{cache:'no-store'});if(!response.headers.get('Content-Type')?.includes('application/json'))return;const value=await response.json();if(value.id!==lastPlan){lastPlan=value.id;showPlan(value);}}catch{}}
window.addEventListener('load',()=>{ready();pollPlan();});window.addEventListener('resize',ready);setInterval(pollPlan,250);
