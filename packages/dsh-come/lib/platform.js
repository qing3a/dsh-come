'use strict';

/**
 * 平台/架构 → 发行资产映射。
 * 与 .github/workflows/release.yml 的矩阵构建严格对齐：
 *   win32   → dsh-come.exe（x64 runner 构建）
 *   darwin  → dsh-come-macos（universal，lipo 合并 x64+arm64）
 *   linux   → dsh-come-linux（x64 runner 构建）
 */

const RELEASE = {
  win: {
    asset: 'dsh-come.exe',
    manifest: 'update-win.json',
    arch: ['x64', 'arm64'], // arm64 无原生资产，x64 exe 可在模拟下运行（给 warning）
  },
  macos: {
    asset: 'dsh-come-macos',
    manifest: 'update-macos.json',
    arch: ['x64', 'arm64'], // universal 双架构通用
  },
  linux: {
    asset: 'dsh-come-linux',
    manifest: 'update-linux.json',
    arch: ['x64'], // 目前只有 x64 资产
  },
};

function detect() {
  const platform = process.platform;
  const arch = process.arch;

  let suffix = null;
  if (platform === 'win32') suffix = 'win';
  else if (platform === 'darwin') suffix = 'macos';
  else if (platform === 'linux') suffix = 'linux';

  if (!suffix) {
    throw new Error(
      `不支持的平台: ${platform}（dsh-come 目前仅支持 Windows / macOS / Linux）`
    );
  }

  const spec = RELEASE[suffix];
  if (!spec.arch.includes(arch)) {
    throw new Error(
      `当前架构 ${arch} 暂无发行资产（${suffix} 仅提供 ${spec.arch.join('/')}）`
    );
  }

  return {
    platform,
    arch,
    suffix,
    asset: spec.asset,
    manifest: spec.manifest,
    archWarning:
      platform === 'win32' && arch === 'arm64'
        ? 'Windows ARM64 无原生资产，将安装 x64 版本（依赖系统模拟层运行）'
        : null,
  };
}

module.exports = { detect, RELEASE };
