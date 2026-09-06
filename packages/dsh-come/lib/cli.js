'use strict';

const fs = require('node:fs');
const fsp = require('node:fs/promises');
const { spawn } = require('node:child_process');

const { detect } = require('./platform');
const { binPath, versionFilePath } = require('./paths');
const { compareVersions } = require('./download');
const { install, readLocalVersion, localBinaryExists } = require('./install');

const HELP = `dsh-come — DSH 桌面壳的 npm 安装器

用法:
  dsh-come [参数...]            启动壳并透传参数（等价于直接运行 dsh-come 二进制）
  dsh-come install [--force] [--source registry|github]   安装 / 强制重装
  dsh-come update               检查并更新到最新版
  dsh-come status               查看本地版本 / 安装路径 / 最新版本
  dsh-come uninstall            删除二进制（保留配置与数据）
  dsh-come help                 显示本帮助

示例:
  npm install -g dsh-come       安装（postinstall 自动就位二进制，走 npm registry）
  npx dsh-come install          免安装直用（npx 临时环境）
  dsh-come install --source github   强制从 GitHub Releases 拉取（网络可达时）
  dsh-come web                  启动壳并打开 dsh web（参数透传）

二进制来源优先级: npm registry 平台包（默认，国内镜像友好）→ GitHub Releases（fallback）
`;

async function ensureInstalled() {
  if (await localBinaryExists()) return;
  console.log('==> 未检测到 dsh-come 二进制，先执行安装');
  await install();
}

async function cmdStatus() {
  const info = detect();
  const local = await readLocalVersion();
  const exists = await localBinaryExists();
  // registry 分发模型：npm 上的主包版本即"最新可安装版本"，无网络依赖
  const latest = require('../package.json').version;
  console.log(`平台        ${info.platform}/${info.arch}（${info.suffix}）`);
  console.log(`安装路径    ${binPath()}`);
  console.log(`本地版本    ${exists ? local || '(已装但无版本标记)' : '未安装'}`);
  console.log(`最新版本    ${latest}（npm registry）`);
  console.log(`升级方式    npm install -g dsh-come@latest`);
}

async function cmdInstall(args) {
  const srcArg = args.find((a) => a.startsWith('--source='));
  const src = srcArg ? srcArg.split('=')[1] : args.includes('--source')
    ? args[args.indexOf('--source') + 1]
    : 'auto';
  if (!['auto', 'registry', 'github'].includes(src)) {
    throw new Error(`未知来源 ${src}（可选: auto / registry / github）`);
  }
  const r = await install({ force: args.includes('--force'), source: src });
  if (r.status === 'up-to-date') {
    console.log(`==> 已是最新 v${r.version}（${r.source}），无需安装。--force 可强制重装`);
  }
}

async function cmdUpdate() {
  const r = await install(); // 版本比对在 install 内部完成
  if (r.status === 'up-to-date') {
    console.log(`==> 已是最新 v${r.version}（${r.source}）`);
  }
}

async function cmdUninstall() {
  const target = binPath();
  const ver = versionFilePath();
  let removed = false;
  for (const p of [target, ver]) {
    try {
      await fsp.rm(p, { force: true });
      removed = true;
    } catch (err) {
      // Windows 上二进制可能被托盘进程占用
      if (err.code === 'EPERM' || err.code === 'EBUSY' || err.code === 'EACCES') {
        console.error(`删除失败（文件被占用？请先退出托盘中的 dsh-come）: ${p}`);
      } else {
        throw err;
      }
    }
  }
  console.log(removed ? `==> 已删除 ${target}` : '==> 无已安装的二进制');
}

async function run() {
  const args = process.argv.slice(2);
  const cmd = args[0];

  switch (cmd) {
    case 'help':
    case '--help':
    case '-h':
      console.log(HELP);
      return;
    case 'install':
      await cmdInstall(args);
      return;
    case 'update':
      await cmdUpdate();
      return;
    case 'status':
      await cmdStatus();
      return;
    case 'uninstall':
      await cmdUninstall();
      return;
    default:
      break;
  }

  // 默认：确保二进制存在 → spawn 并透传全部参数
  await ensureInstalled();
  const target = binPath();
  const child = spawn(target, args, { stdio: 'inherit' });
  child.on('error', (err) => {
    console.error(`启动失败: ${target}\n${err.message}`);
    process.exitCode = 1;
  });
  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exitCode = code ?? 0;
    }
  });
}

run().catch((err) => {
  console.error(`错误: ${err.message || err}`);
  if (process.env.DSH_COME_DEBUG) console.error(err.stack);
  process.exitCode = 1;
});
