'use strict';

/**
 * preuninstall：npm uninstall -g dsh-come 时清理二进制。
 * 只删除二进制与版本标记，不触碰任何配置/数据目录（保守原则）。
 */

const fsp = require('node:fs/promises');
const { binPath, versionFilePath } = require('../lib/paths');

async function main() {
  if (process.env.npm_config_global !== 'true') return;
  const paths = [binPath(), versionFilePath()];
  for (const p of paths) {
    try {
      await fsp.rm(p, { force: true });
      console.log(`[dsh-come] 已清理 ${p}`);
    } catch (err) {
      if (err.code === 'EPERM' || err.code === 'EBUSY' || err.code === 'EACCES') {
        console.warn(`[dsh-come] 删除失败（托盘进程占用？可稍后执行 dsh-come uninstall）: ${p}`);
      }
    }
  }
}

main();
