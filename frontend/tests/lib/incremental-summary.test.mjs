import assert from 'node:assert/strict';
import fs from 'node:fs';import vm from 'node:vm';import ts from 'typescript';import {test} from 'node:test';
const source=fs.readFileSync(new URL('../../src/lib/incrementalSummary.ts',import.meta.url),'utf8');
const exports={};const ctx=vm.createContext({exports});vm.runInContext(ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,ctx);
const t=(id,start,end,text='测试',speaker='发言人1',partial=false)=>({id,sequence_id:Number(id.split('-').at(-1)),audio_start_time:start,audio_end_time:end,text,speaker,is_partial:partial});
test('five minutes waits for the first closed paragraph, not a hard cut',()=>{
 const list=[t('live-1',280,298),t('live-101',299,313)];
 assert.equal(exports.nextSummaryBoundary(list,0,310),null);
 assert.equal(exports.nextSummaryBoundary(list,0,316),313);
 assert.equal(exports.nextSummaryBoundary([...list,t('live-201',313,315,'继续','发言人1',true)],0,318),null);
});
test('speaker change closes a paragraph and the next target is relative to its end',()=>{
 const list=[t('live-1',290,312),t('live-101',313,325,'下一段','发言人2')];
 assert.equal(exports.nextSummaryBoundary(list,0,315),312);
 assert.equal(exports.nextSummaryBoundary(list,312,600),null);
});
test('final speaker subdivisions do not move words between cached intervals',()=>{
 const before=[t('live-101',295,310,'第一部分第二部分')];
 const after=[t('live-101',295,300,'第一部分'),t('live-102',301,310,'第二部分','发言人2')];
 assert.equal(exports.sourceText(before,0,300),exports.sourceText(after,0,300));
 assert.equal(exports.intervalSegments(after,300,600).length,0);
});
