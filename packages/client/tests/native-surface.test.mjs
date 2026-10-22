import assert from 'node:assert/strict';
import { test } from 'node:test';
import { Scope } from '../src/scope.ts';
import { NativeSurface } from '../src/surface.ts';

test('a late attachment is destroyed if its component is disposed during startup', async () => {
  const requests=[];
  let resolve;
  const transport={dispatch:async request=>{
    requests.push(request);
    if(request.action==='surface.attach')return new Promise(r=>resolve=r);
    return null;
  }};
  const scope=new Scope(transport,Promise.resolve('scope'));
  const surface=new NativeSurface(scope,{id:'scene'});
  await new Promise(r=>setImmediate(r));
  const disposing=surface.dispose();
  resolve({viewport:'v',adapter:'test',bufferBytes:1});
  await disposing;
  assert.equal(requests.filter(r=>r.action==='surface.dispose').length,1);
  assert.ok(!requests.some(r=>r.action==='surface.place'));
});

test('suspension awaits hiding acknowledgement before a covering dialog opens', async () => {
  const requests=[];let acknowledge;
  const transport={dispatch:async request=>{
    requests.push(request);
    if(request.action==='surface.attach')return {viewport:'v',adapter:'test',bufferBytes:1};
    if(request.action==='surface.place'&&request.visible===false)return new Promise(r=>acknowledge=r);
    return null;
  }};
  const scope=new Scope(transport,Promise.resolve('scope'));
  const surface=new NativeSurface(scope,{id:'scene'});
  await surface.ready;
  let hidden=false;const hiding=surface.place([0,0,100,100],false).then(()=>hidden=true);
  await new Promise(r=>setImmediate(r));assert.equal(hidden,false);
  acknowledge(null);await hiding;assert.equal(hidden,true);
  await surface.dispose();
});
