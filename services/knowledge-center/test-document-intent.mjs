import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const code=readFileSync(new URL('./pi-worker.mjs',import.meta.url),'utf8');
const expression=code.match(/const wantsDocument=(.*);/)[1];
const wants=query=>Function('data','return '+expression)({query});
for(const query of ['生成doc','请把上述内容整理成文档','Generate a doc from the above answer.','帮我做个Word'])assert.equal(wants(query),true,query);
for(const query of ['如何生成doc','不要生成doc','白鹤滩现在什么状态'])assert.equal(wants(query),false,query);
console.log('7 document intent cases passed');
