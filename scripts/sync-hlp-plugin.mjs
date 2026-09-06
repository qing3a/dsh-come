#!/usr/bin/env node
// sync-hlp-plugin.mjs —— 发布前把最新 HLP 插件同步进 dsh-come 的发行版插件源（v1.4.0 P1-7）
//
// 用法：node scripts/sync-hlp-plugin.mjs [--hlp-path <HLP 开发仓库路径>]
//   默认 HLP 路径 = <dsh-come>/../Harness-LocalApp
// 行为：
//   1. 校验 HLP 源插件完整性（package.json / index.js / lib / 两个业务 Server 的 package.json）
//   2. 清理目标 target/release/hlp-plugin/dsh-light-cockpit 后整体复制，
//      排除：.git、*.log、测试脚本、data/（真实邮箱/询价数据不进发行版——运行时数据由插件自建）
//   3. 验证：目标 package.json version 非空、business/*/node_modules 存在、文件数 > 阈值
//   4. 输出同步报告；任何一步失败 exit 1

import { cpSync, existsSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const comeRoot = path.resolve(here, '..');

// --hlp-path 参数
const argIdx = process.argv.indexOf('--hlp-path');
const hlpRoot = path.resolve(argIdx > 0 ? process.argv[argIdx + 1] : path.join(comeRoot, '..', 'Harness-LocalApp'));

const srcPlugin = path.join(hlpRoot, 'dsh-light-cockpit');
const dstRoot = path.join(comeRoot, 'target', 'release', 'hlp-plugin');
const dstPlugin = path.join(dstRoot, 'dsh-light-cockpit');

const EXCLUDE_DIRS = new Set(['.git', 'data', 'node_modules/.cache', 'persisted', 'logs', '.spike']);
const EXCLUDE_FILES = new Set(['.gitignore', 'crm.check.js']);
const EXCLUDE_RE = [/\.log$/, /test-.*\.m?js$/, /check-.*\.js$/, /-test\.js$/];
const isTestFile = (name) => /^test-/.test(name) || /-test\.m?js$/.test(name);

function fail(msg) {
  console.error(`❌ ${msg}`);
  process.exit(1);
}

// 1) 源完整性
for (const rel of ['package.json', 'index.js', 'lib', path.join('business', 'biz-cockpit-mcp', 'package.json'), path.join('business', 'mail-collab-server', 'package.json')]) {
  if (!existsSync(path.join(srcPlugin, rel))) fail(`HLP 源不完整：缺 ${rel}（${srcPlugin}）`);
}
const srcVersion = JSON.parse(readFileSync(path.join(srcPlugin, 'package.json'), 'utf8')).version;
if (!srcVersion) fail('源 package.json 无 version');

// 2) 清理 + 复制（排除规则）
if (existsSync(dstPlugin)) rmSync(dstPlugin, { recursive: true, force: true });
cpSync(srcPlugin, dstPlugin, {
  recursive: true,
  filter: (src) => {
    const rel = path.relative(srcPlugin, src);
    if (!rel) return true;
    const segs = rel.split(path.sep);
    if (segs.some((s) => EXCLUDE_DIRS.has(s))) return false;
    const name = path.basename(src);
    if (EXCLUDE_FILES.has(name) || EXCLUDE_RE.some((re) => re.test(name))) return false;
    // 顶层测试/迁移脚本不进发行版（business 内的 test-* 保留无妨？统一排除测试）
    if (segs.length <= 1 && isTestFile(name)) return false;
    return true;
  },
});

// 3) 验证
const dstManifest = path.join(dstPlugin, 'package.json');
if (!existsSync(dstManifest)) fail('复制后目标 package.json 缺失');
const dstVersion = JSON.parse(readFileSync(dstManifest, 'utf8')).version;
if (!dstVersion) fail('目标 package.json version 为空');
if (dstVersion !== srcVersion) fail(`版本不一致：源 ${srcVersion} vs 目标 ${dstVersion}`);
for (const biz of ['biz-cockpit-mcp', 'mail-collab-server']) {
  if (!existsSync(path.join(dstPlugin, 'business', biz, 'index.js'))) fail(`业务 Server 缺失：business/${biz}/index.js`);
  const nm = path.join(dstPlugin, 'business', biz, 'node_modules');
  if (!existsSync(nm)) fail(`业务 Server node_modules 缺失（MCP 工具起不来）：business/${biz}/node_modules`);
}
let count = 0;
(function walk(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p);
    else count += 1;
  }
})(dstPlugin);
if (count < 1000) fail(`文件数异常（${count} < 1000）——node_modules 可能没复制全`);

// 4) 报告（写 version 标记供 CI/发布用）
const report = { syncedAt: new Date().toISOString(), version: dstVersion, files: count, source: srcPlugin };
writeFileSync(path.join(dstRoot, 'hlp-plugin-version.json'), JSON.stringify(report, null, 2));
console.log(`✅ HLP 插件同步完成：v${dstVersion}，${count} 个文件 → ${dstPlugin}`);
console.log(`   版本标记：${path.join(dstRoot, 'hlp-plugin-version.json')}`);
