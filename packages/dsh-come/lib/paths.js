'use strict';

const os = require('node:os');
const path = require('node:path');

/**
 * 安装路径决策。
 * - Windows: %LOCALAPPDATA%\dsh-come\dsh-come.exe（用户级，无管理员需求）
 * - macOS/Linux: ~/.local/bin/dsh-come（与 scripts/install.sh 完全一致）
 * 版本标记文件与二进制同目录，供 status/update 比对。
 */

function appDir() {
  if (process.platform === 'win32') {
    const base = process.env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
    return path.join(base, 'dsh-come');
  }
  return path.join(os.homedir(), '.local', 'bin');
}

function binName() {
  return process.platform === 'win32' ? 'dsh-come.exe' : 'dsh-come';
}

function binPath() {
  return path.join(appDir(), binName());
}

function versionFilePath() {
  return path.join(appDir(), 'dsh-come.version');
}

module.exports = { appDir, binName, binPath, versionFilePath };
