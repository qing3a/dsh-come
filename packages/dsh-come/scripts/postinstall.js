'use strict';

/**
 * postinstall：npm install -g dsh-come 时自动拉取二进制。
 *
 * 容错策略（关键）：
 * - 仅全局安装时自动下载；作为本地依赖安装时跳过（避免把桌面壳二进制
 *   塞进每个依赖它的项目 node_modules）。
 * - 下载失败（网络 / GitHub 限流 / 平台暂无资产）不阻断 npm install，
 *   打印明确 warning；此后任何 `dsh-come` 命令首次执行都会懒安装补上。
 */

const { install } = require('../lib/install');

async function main() {
  if (process.env.npm_config_global !== 'true') {
    console.log('dsh-come: 本地依赖安装模式，跳过二进制下载（全局安装时自动下载）');
    return;
  }
  try {
    await install();
  } catch (err) {
    console.warn(
      `\n[dsh-come] 二进制自动下载失败：${err.message || err}` +
        `\n[dsh-come] npm 安装未回滚；首次运行 \`dsh-come\` 时会自动重试安装。` +
        `\n[dsh-come] 也可手动执行: npx dsh-come install\n`
    );
  }
}

main();
