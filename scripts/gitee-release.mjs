#!/usr/bin/env node
/**
 * gitee-release.mjs
 *
 * 将指定目录下的产物发布到 Gitee Releases（替代已被移除的旧 gitee-release.mjs）。
 * 仅做 Linux 产物的发布（Windows 产物由 build-windows.ps1 本地构建后手动上传，或另起流水线）。
 *
 * 环境变量：
 *   GITEE_TOKEN         Gitee 个人令牌（必填；缺失时仅打印待上传清单并退出 0，不报错）
 *   GITEE_OWNER         仓库所有者（默认 Yezi26）
 *   GITEE_REPO          仓库名（默认 nrmm-tauri）
 *   GITEE_TAG           发布标签（默认取 GITEE_BRANCH 或 v<package.json version>）
 *
 * 用法：
 *   node scripts/gitee-release.mjs <产物目录>
 */

import { readdirSync, statSync, readFileSync, existsSync } from 'node:fs';
import { join, resolve, basename } from 'node:path';
import { request } from 'node:https';
import { request as httpRequest } from 'node:http';

const ROOT = resolve(process.cwd());
const ARTIFACT_DIR = process.argv[2] || 'dist';

const OWNER = process.env.GITEE_OWNER || 'Yezi26';
const REPO = process.env.GITEE_REPO || 'nrmm-tauri';
const TOKEN = process.env.GITEE_TOKEN || process.env.GITEE_ACCESS_TOKEN || '';
const pkg = JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8'));
const TAG =
  process.env.GITEE_TAG ||
  process.env.GITEE_BRANCH ||
  `v${pkg.version}`;

const API = 'https://gitee.com/api/v5';

function apiRequest(method, path, { token, json, multipart } = {}) {
  return new Promise((resolve, reject) => {
    const url = new URL(`${API}${path}`);
    if (token && !multipart) url.searchParams.set('access_token', token);
    const lib = url.protocol === 'http:' ? httpRequest : request;
    const headers = {};
    if (json) {
      headers['Content-Type'] = 'application/json';
      headers['Accept'] = 'application/json';
    }
    if (multipart) {
      headers['Content-Type'] = multipart.contentType;
    }
    const req = lib(
      url,
      { method, headers },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          const raw = Buffer.concat(chunks).toString('utf8');
          let body = null;
          try { body = raw ? JSON.parse(raw) : null; } catch { body = raw; }
          resolve({ status: res.statusCode, body });
        });
      }
    );
    req.on('error', reject);
    if (json) req.write(JSON.stringify(json));
    if (multipart) req.write(multipart.body);
    req.end();
  });
}

function collectFiles(dir) {
  if (!existsSync(dir)) return [];
  const out = [];
  for (const ent of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, ent.name);
    if (ent.isDirectory()) continue;
    const st = statSync(p);
    if (st.size > 2 * 1024 * 1024 * 1024) {
      console.warn(`  [SKIP] ${ent.name} 超过 2GB，跳过`);
      continue;
    }
    out.push(p);
  }
  return out.sort();
}

function buildMultipart(filePath, fieldName = 'file') {
  const data = readFileSync(filePath);
  const filename = basename(filePath);
  const boundary = `----nrmmtauri${Date.now()}`;
  const head = Buffer.from(
    `--${boundary}\r\n` +
      `Content-Disposition: form-data; name="${fieldName}"; filename="${filename}"\r\n` +
      'Content-Type: application/octet-stream\r\n\r\n',
    'utf8'
  );
  const tail = Buffer.from(`\r\n--${boundary}--\r\n`, 'utf8');
  return {
    body: Buffer.concat([head, data, tail]),
    contentType: `multipart/form-data; boundary=${boundary}`,
  };
}

async function findOrCreateRelease() {
  // 已存在则直接取 id
  const got = await apiRequest('GET', `/repos/${OWNER}/${REPO}/releases/tags/${encodeURIComponent(TAG)}`, { token: TOKEN });
  if (got.status === 200 && got.body && got.body.id) {
    console.log(`[gitee] 复用已存在 Release #${got.body.id} (tag=${TAG})`);
    return got.body.id;
  }
  const created = await apiRequest('POST', `/repos/${OWNER}/${REPO}/releases`, {
    token: TOKEN,
    json: {
      tag_name: TAG,
      name: TAG,
      body:
        `nrmm-tauri ${pkg.version}\n\n` +
        '自动发布（Gitee 流水线）。仅支持 x86_64，最低运行环境 glibc >= 2.35（Ubuntu 22.04 及以上）。\n\n' +
        '推荐：含 7z 的 portable tar.gz 免安装包或 deb/rpm。deb/rpm 需 sudo，作为环境异常兜底。\n' +
        '所有免安装包均内置 7z CLI（位于程序同级 7z/ 目录），请勿移除或改动；否则解压回退降级为仅支持 zip/7z/rar 并优先调用系统 7z，若系统亦未安装则进一步回退为内置纯 Rust 解压（格式更少、性能较弱）。',
      prerelease: false,
      target_commitish: 'master',
    },
  });
  if (created.status !== 201 && created.status !== 200) {
    throw new Error(`创建 Release 失败 (${created.status}): ${JSON.stringify(created.body)}`);
  }
  console.log(`[gitee] 创建 Release #${created.body.id} (tag=${TAG})`);
  return created.body.id;
}

async function uploadAttachment(releaseId, filePath) {
  const mp = buildMultipart(filePath);
  const url = new URL(
    `${API}/repos/${OWNER}/${REPO}/releases/${releaseId}/attachments?access_token=${encodeURIComponent(TOKEN)}&name=${encodeURIComponent(basename(filePath))}`
  );
  const lib = url.protocol === 'http:' ? httpRequest : request;
  return new Promise((resolve, reject) => {
    const req = lib(
      url,
      { method: 'POST', headers: { 'Content-Type': mp.contentType } },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () => {
          const raw = Buffer.concat(chunks).toString('utf8');
          let body = null;
          try { body = raw ? JSON.parse(raw) : null; } catch { body = raw; }
          resolve({ status: res.statusCode, body });
        });
      }
    );
    req.on('error', reject);
    req.write(mp.body);
    req.end();
  });
}

async function main() {
  const files = collectFiles(ARTIFACT_DIR);
  if (files.length === 0) {
    console.error(`[gitee] 产物目录为空: ${ARTIFACT_DIR}`);
    process.exit(1);
  }
  console.log(`[gitee] 待发布文件（${files.length}）:`);
  files.forEach((f) => console.log(`  - ${basename(f)} (${(statSync(f).size / 1024 / 1024).toFixed(2)} MB)`));

  if (!TOKEN) {
    console.warn('[gitee] 未设置 GITEE_TOKEN，跳过实际上传（请手动上传以上文件到 Gitee Release）。');
    process.exit(0);
  }

  const releaseId = await findOrCreateRelease();
  for (const f of files) {
    // latest.json 不重复作为附件上传（避免覆盖 updater manifest 语义）
    if (basename(f) === 'latest.json') {
      console.log(`  [SKIP] ${basename(f)}（manifest 由 Release 正文/updater 端点引用，非附件）`);
      continue;
    }
    const r = await uploadAttachment(releaseId, f);
    if (r.status === 201 || r.status === 200) {
      console.log(`  [OK] 上传 ${basename(f)}`);
    } else {
      console.error(`  [FAIL] 上传 ${basename(f)} 失败 (${r.status}): ${JSON.stringify(r.body)}`);
      process.exit(1);
    }
  }
  console.log(`[gitee] ✅ 发布完成: https://gitee.com/${OWNER}/${REPO}/releases/tag/${TAG}`);
}

main().catch((e) => {
  console.error('[gitee] ERROR:', e.message);
  process.exit(1);
});
