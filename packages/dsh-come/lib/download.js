'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const fsp = require('node:fs/promises');
const http = require('node:http');
const https = require('node:https');
const path = require('node:path');
const { URL } = require('node:url');

const BASE = 'https://github.com/qing3a/dsh-come/releases/latest/download';

/** 语义化版本比较：a>b → 1，a<b → -1，相等 → 0 */
function compareVersions(a, b) {
  const pa = String(a).replace(/^v/, '').split('.').map(Number);
  const pb = String(b).replace(/^v/, '').split('.').map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const da = pa[i] || 0;
    const db = pb[i] || 0;
    if (da > db) return 1;
    if (da < db) return -1;
  }
  return 0;
}

function httpGet(url, redirects = 0) {
  if (redirects > 5) throw new Error(`重定向过多: ${url}`);
  const { protocol, hostname, pathname, search } = new URL(url);
  const mod = protocol === 'https:' ? https : http;
  return new Promise((resolve, reject) => {
    const req = mod.get(
      {
        hostname,
        path: pathname + search,
        headers: {
          'user-agent': 'dsh-come-npm-installer',
          accept: 'application/json, */*',
        },
      },
      (res) => {
        const status = res.statusCode || 0;
        if (status >= 300 && status < 400 && res.headers.location) {
          res.resume();
          return resolve(httpGet(new URL(res.headers.location, url).href, redirects + 1));
        }
        if (status !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${status} — ${url}`));
        }
        resolve(res);
      }
    );
    req.on('error', reject);
    req.setTimeout(30_000, () => req.destroy(new Error('请求超时(30s)')));
  });
}

/** 获取文本（更新清单） */
async function fetchText(url) {
  const res = await httpGet(url);
  const chunks = [];
  for await (const chunk of res) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8');
}

/**
 * 下载 → 边写盘边算 sha256 → 校验通过后原子 rename。
 * 校验失败不落地任何文件。
 */
async function downloadVerifiedStream(url, sha256, dest) {
  const res = await httpGet(url);
  const hash = crypto.createHash('sha256');
  const tmp = dest + '.download-' + process.pid + '-' + Date.now();
  await fsp.mkdir(path.dirname(tmp), { recursive: true });
  try {
    const ws = fs.createWriteStream(tmp);
    await new Promise((resolve, reject) => {
      res.on('data', (c) => hash.update(c));
      res.pipe(ws);
      ws.on('finish', resolve);
      ws.on('error', reject);
      res.on('error', reject);
    });
    const got = hash.digest('hex');
    if (got !== sha256) {
      throw new Error(
        `sha256 校验失败: 期望 ${sha256}\n          实际 ${got}（下载被篡改或清单过期）`
      );
    }
    await fsp.rename(tmp, dest);
  } catch (err) {
    await fsp.rm(tmp, { force: true });
    throw err;
  }
}

/** 读取最新更新清单（按平台） */
async function fetchManifest(suffix) {
  const url = `${BASE}/update-${suffix}.json`;
  const text = await fetchText(url);
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    throw new Error(`更新清单解析失败: ${url}`);
  }
  if (!data.version || !data.url || !data.sha256) {
    throw new Error(`更新清单字段缺失(version/url/sha256): ${url}`);
  }
  return { version: data.version, url: data.url, sha256: data.sha256, source: url };
}

module.exports = {
  BASE,
  compareVersions,
  downloadVerifiedStream,
  fetchManifest,
  fetchText,
};
