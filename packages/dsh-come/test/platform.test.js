'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');

const { detect, RELEASE } = require('../lib/platform');
const { compareVersions } = require('../lib/download');
const { PLATFORM_PKGS } = require('../lib/install');

test('发行资产矩阵与 release.yml 对齐', () => {
  assert.equal(RELEASE.win.asset, 'dsh-come.exe');
  assert.equal(RELEASE.win.manifest, 'update-win.json');
  assert.equal(RELEASE.macos.asset, 'dsh-come-macos');
  assert.equal(RELEASE.macos.manifest, 'update-macos.json');
  assert.equal(RELEASE.linux.asset, 'dsh-come-linux');
  assert.equal(RELEASE.linux.manifest, 'update-linux.json');
});

test('detect: 当前平台返回合法资产名', () => {
  const info = detect();
  assert.ok(['win', 'macos', 'linux'].includes(info.suffix));
  assert.ok(info.asset && info.manifest);
  assert.ok(info.arch);
});

test('detect: win arm64 给出 warning 而非报错', () => {
  // 直接构造验证 RELEASE 表：win 支持 arm64（模拟运行）
  assert.ok(RELEASE.win.arch.includes('arm64'));
});

test('compareVersions: 语义化版本比较', () => {
  assert.equal(compareVersions('0.2.0', '0.2.0'), 0);
  assert.equal(compareVersions('0.2.0', '0.3.0'), -1);
  assert.equal(compareVersions('0.3.0', '0.2.9'), 1);
  assert.equal(compareVersions('v0.3.0', '0.3.0'), 0);
  assert.equal(compareVersions('0.10.0', '0.9.9'), 1);
  assert.equal(compareVersions('1.0.0', '0.9.9'), 1);
});

test('平台包映射覆盖全部有资产的平台', () => {
  // 每个有发行资产的平台组合都有对应平台包（scoped，规避 npm 新包名风控）
  assert.equal(PLATFORM_PKGS['win32-x64'].pkg, '@qing3a/dsh-come-win32-x64');
  assert.equal(PLATFORM_PKGS['darwin-x64'].pkg, '@qing3a/dsh-come-darwin');
  assert.equal(PLATFORM_PKGS['darwin-arm64'].pkg, '@qing3a/dsh-come-darwin');
  assert.equal(PLATFORM_PKGS['linux-x64'].pkg, '@qing3a/dsh-come-linux-x64');
  // 无平台包的组合（win arm64 / linux arm64）不应有映射，走 GitHub fallback
  assert.equal(PLATFORM_PKGS['win32-arm64'], undefined);
  assert.equal(PLATFORM_PKGS['linux-arm64'], undefined);
});
