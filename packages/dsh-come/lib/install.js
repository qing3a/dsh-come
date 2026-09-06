'use strict';

const fs = require('node:fs');
const fsp = require('node:fs/promises');
const path = require('node:path');

const { detect } = require('./platform');
const { appDir, binPath, versionFilePath } = require('./paths');
const { compareVersions, downloadVerifiedStream, fetchManifest } = require('./download');

/** 平台包名 → 包内二进制相对路径（与 packages/dsh-come-* 的 files/bin 对齐） */
const PLATFORM_PKGS = {
  'win32-x64': { pkg: '@qing3a/dsh-come-win32-x64', file: 'bin/dsh-come.exe' },
  'darwin-x64': { pkg: '@qing3a/dsh-come-darwin', file: 'bin/dsh-come-macos' },
  'darwin-arm64': { pkg: '@qing3a/dsh-come-darwin', file: 'bin/dsh-come-macos' },
  'linux-x64': { pkg: '@qing3a/dsh-come-linux-x64', file: 'bin/dsh-come-linux' },
};

function platformKey() {
  const { platform, arch } = detect();
  return `${platform}-${arch}`;
}

/** 从 node_modules 定位平台包二进制绝对路径；未安装返回 null */
function resolvePlatformBinary() {
  const key = platformKey();
  const spec = PLATFORM_PKGS[key];
  if (!spec) return null; // win arm64 / linux arm64 无平台包 → 走 GitHub fallback
  try {
    return require.resolve(`${spec.pkg}/${spec.file}`);
  } catch {
    return null;
  }
}

/** 读本地已安装版本（标记文件），缺失/损坏返回 null */
async function readLocalVersion() {
  try {
    const v = (await fsp.readFile(versionFilePath(), 'utf8')).trim();
    return v ? v : null;
  } catch {
    return null;
  }
}

async function localBinaryExists() {
  try {
    const st = await fsp.stat(binPath());
    return st.size > 0;
  } catch {
    return false;
  }
}

/** 从平台包复制二进制就位（registry 分发主路径，无网络请求） */
async function installFromRegistry({ force = false, quiet = false } = {}) {
  const info = detect();
  const src = resolvePlatformBinary();
  if (!src) return null;

  const version = require('../package.json').version;
  const localVersion = await readLocalVersion();
  const exists = await localBinaryExists();
  if (!force && exists && localVersion && compareVersions(localVersion, version) >= 0) {
    return { status: 'up-to-date', version: localVersion, bin: binPath(), source: 'registry', ...info };
  }

  if (!quiet) {
    const action = exists ? '更新到' : '安装';
    console.log(`==> ${action} dsh-come v${version}（registry/${info.suffix}）`);
  }

  await fsp.mkdir(appDir(), { recursive: true });
  await fsp.copyFile(src, binPath());
  if (process.platform !== 'win32') {
    await fsp.chmod(binPath(), 0o755);
  }
  const tmpVer = versionFilePath() + '.tmp-' + process.pid;
  await fsp.writeFile(tmpVer, version + '\n', 'utf8');
  await fsp.rename(tmpVer, versionFilePath());

  if (!quiet) {
    console.log(`==> 完成。已安装到 ${binPath()}`);
  }
  return { status: exists ? 'updated' : 'installed', version, bin: binPath(), source: 'registry', ...info };
}

/** 从 GitHub Releases 下载（fallback：平台包缺失 / 显式 --source github） */
async function installFromGitHub({ force = false, quiet = false } = {}) {
  const info = detect();
  const manifest = await fetchManifest(info.suffix);

  const localVersion = await readLocalVersion();
  const exists = await localBinaryExists();
  if (!force && exists && localVersion && compareVersions(localVersion, manifest.version) >= 0) {
    return {
      status: 'up-to-date',
      version: localVersion,
      bin: binPath(),
      source: 'github',
      ...info,
      manifest,
    };
  }

  if (!quiet) {
    const action = exists ? '更新到' : '安装';
    console.log(`==> ${action} dsh-come v${manifest.version}（github/${info.suffix}）`);
    if (info.archWarning) console.warn(`    ${info.archWarning}`);
  }

  await downloadVerifiedStream(manifest.url, manifest.sha256, binPath());
  if (process.platform !== 'win32') {
    await fsp.chmod(binPath(), 0o755);
  }
  const tmpVer = versionFilePath() + '.tmp-' + process.pid;
  await fsp.writeFile(tmpVer, manifest.version + '\n', 'utf8');
  await fsp.rename(tmpVer, versionFilePath());

  if (!quiet) {
    console.log(`==> 完成。已安装到 ${binPath()}`);
  }
  return { status: exists ? 'updated' : 'installed', version: manifest.version, bin: binPath(), source: 'github', ...info, manifest };
}

/**
 * 安装编排：registry 复制优先，平台包缺失时回退 GitHub。
 * @param {object} opts
 * @param {boolean} opts.force
 * @param {'registry'|'github'|'auto'} [opts.source]
 */
async function install({ force = false, quiet = false, source = 'auto' } = {}) {
  if (source === 'github') return installFromGitHub({ force, quiet });
  const r = await installFromRegistry({ force, quiet });
  if (r) return r;
  if (!quiet) {
    console.log('==> 平台包未随 npm 安装（optionalDependencies 未命中？），回退 GitHub Releases');
  }
  return installFromGitHub({ force, quiet });
}

module.exports = { install, readLocalVersion, localBinaryExists, compareVersions, resolvePlatformBinary, PLATFORM_PKGS };
