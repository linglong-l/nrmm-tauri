#!/usr/bin/env node
/**
 * Gitee Release 创建与附件上传脚本（CI + 本地双模式）。
 *
 * 环境变量：
 *   GITEE_TOKEN        — Gitee 私人令牌（必须，需 projects 权限）
 *   GITEE_OWNER        — 仓库所有者（默认 Yezi26）
 *   GITEE_REPO         — 仓库名（默认 nrmm-tauri）
 *   GITEE_TAG          — tag 名称（如 v0.3.0），可被 --tag 参数覆盖
 *   GITEE_COMMIT       — 对应的 commit SHA（仅创建时使用）
 *
 * CLI 参数：
 *   --tag <tag>        指定 tag 名（优先级高于环境变量）
 *   --upload-only      跳过 Release 创建，仅上传附件到已存在的 Release
 *   --force            跳过同名附件检查，强制重新上传
 *   --notes <file>     从文件读取 Release 说明（body）
 *   -o, --output <dir> artifacts 目录（位置参数亦可）
 *
 * 用法：
 *   # CI 模式（自动创建 Release + 上传）
 *   node scripts/gitee-release.mjs dist/
 *
 *   # 本地 Windows 构建后补传附件（Release 已由 CI 创建）
 *   $env:GITEE_TOKEN="your_token"; node scripts/gitee-release.mjs --upload-only --tag v0.3.0 dist/
 *
 *   # 完全本地模式（创建 Release + 上传）
 *   $env:GITEE_TOKEN="your_token"; node scripts/gitee-release.mjs --tag v0.3.0 dist/
 */

import { readFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';

const API_BASE = 'https://gitee.com/api/v5';

function parseArgs(argv) {
  const args = { uploadOnly: false, force: false, tag: '', notesFile: '', artifactsDir: '' };
  const rest = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--upload-only') args.uploadOnly = true;
    else if (a === '--force') args.force = true;
    else if (a === '--tag') args.tag = argv[++i] || '';
    else if (a === '--notes') args.notesFile = argv[++i] || '';
    else if (a === '-o' || a === '--output') args.artifactsDir = argv[++i] || '';
    else if (!a.startsWith('-')) rest.push(a);
  }
  if (!args.artifactsDir && rest.length > 0) args.artifactsDir = rest[0];
  return args;
}

const cli = parseArgs(process.argv.slice(2));

const token = process.env.GITEE_TOKEN;
const owner = process.env.GITEE_OWNER || 'Yezi26';
const repo = process.env.GITEE_REPO || 'nrmm-tauri';
const tag = cli.tag || process.env.GITEE_TAG || process.env.GITEE_BRANCH || '';
const commitSha = process.env.GITEE_COMMIT || '';
const artifactsDir = cli.artifactsDir ? resolve(cli.artifactsDir) : null;

if (!token) {
  console.error('[gitee-release] ERROR: 未设置 GITEE_TOKEN 环境变量。');
  console.error('');
  console.error('配置方法：');
  console.error('  PowerShell: $env:GITEE_TOKEN="你的私人令牌"');
  console.error('  CMD:        set GITEE_TOKEN=你的私人令牌');
  console.error('  Bash:       export GITEE_TOKEN=你的私人令牌');
  console.error('');
  console.error('获取私人令牌：https://gitee.com/profile/personal_access_tokens（需勾选 projects 权限）');
  process.exit(1);
}

if (!tag) {
  console.error('[gitee-release] ERROR: 无法确定 tag 名。请通过 --tag 参数或 GITEE_TAG 环境变量指定。');
  console.error('  示例: node scripts/gitee-release.mjs --tag v0.3.0 dist/');
  process.exit(1);
}

async function apiRequest(method, path, options = {}) {
  const url = `${API_BASE}/repos/${owner}/${repo}${path}`;
  const sep = url.includes('?') ? '&' : '?';
  const fullUrl = `${url}${sep}access_token=${encodeURIComponent(token)}`;
  const resp = await fetch(fullUrl, {
    method,
    headers: { 'User-Agent': 'nrmm-rust-ci', ...(options.headers || {}) },
    ...options,
  });
  const text = await resp.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    data = { raw: text };
  }
  return { ok: resp.ok, status: resp.status, data };
}

async function uploadFile(releaseId, filePath) {
  const fileName = basename(filePath);
  const stat = statSync(filePath);
  const fileBuffer = readFileSync(filePath);

  const boundary = `----giteeBoundary${Date.now()}${Math.random().toString(36).slice(2)}`;
  const CRLF = '\r\n';
  const pre = Buffer.from(
    `--${boundary}${CRLF}` +
      `Content-Disposition: form-data; name="file"; filename="${fileName}"${CRLF}` +
      `Content-Type: application/octet-stream${CRLF}${CRLF}`,
    'utf8'
  );
  const post = Buffer.from(`${CRLF}--${boundary}--${CRLF}`, 'utf8');
  const body = Buffer.concat([pre, fileBuffer, post]);

  const url = `${API_BASE}/repos/${owner}/${repo}/releases/${releaseId}/attach_files?access_token=${encodeURIComponent(token)}`;
  const resp = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': `multipart/form-data; boundary=${boundary}`,
      'Content-Length': body.length,
      'User-Agent': 'nrmm-rust-ci',
    },
    body,
  });
  const text = await resp.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    data = { raw: text };
  }
  return { ok: resp.ok, status: resp.status, data, size: stat.size, name: fileName };
}

async function deleteAsset(releaseId, assetId) {
  return apiRequest('DELETE', `/releases/${releaseId}/attach_files/${assetId}`);
}

async function listAssets(releaseId) {
  const r = await apiRequest('GET', `/releases/${releaseId}/attach_files?page=1&per_page=100`);
  if (!r.ok) return [];
  return Array.isArray(r.data) ? r.data : [];
}

function findFiles(dir) {
  if (!dir || !existsSync(dir)) return [];
  const out = [];
  const entries = readdirSync(dir, { withFileTypes: true });
  for (const ent of entries) {
    if (ent.isFile()) out.push(join(dir, ent.name));
  }
  return out;
}

function defaultReleaseBody(existingBody) {
  const defaults = [
    `## nrmm-rust ${tag}`,
    '',
    '自动构建发布（Gitee Go + 本地构建）。',
    '',
    '### Linux 产物',
    '- **AppImage**: `nrmm-rust-x86_64.AppImage` — 通用 Linux 包',
    '- **deb**: `nrmm-rust-x86_64.deb` — Debian/Ubuntu 系',
    '- **rpm**: `nrmm-rust-x86_64.rpm` — Fedora/RHEL 系',
    '',
    '### Windows 产物',
    '- **NSIS 安装包**: `nrmm-rust-setup-x86_64.exe` — Windows 安装程序',
    '',
    '### 自更新',
    '附件 `latest.json` 为 Tauri 官方 updater 提供更新元信息。',
  ].join('\n');
  if (existingBody && existingBody.trim().length > 20) return existingBody;
  return defaults;
}

async function main() {
  console.log(`[gitee-release] ==========================================`);
  console.log(`[gitee-release] Owner/Repo : ${owner}/${repo}`);
  console.log(`[gitee-release] Tag       : ${tag}`);
  console.log(`[gitee-release] Commit    : ${commitSha || '(n/a)'}`);
  console.log(`[gitee-release] 模式      : ${cli.uploadOnly ? '仅上传（Release 已存在）' : '创建/复用 Release + 上传'}`);
  console.log(`[gitee-release] Artifacts : ${artifactsDir || '(none)'}`);
  console.log(`[gitee-release] ==========================================`);

  let releaseId = null;
  let existingBody = '';

  if (cli.uploadOnly) {
    console.log('\n[gitee-release] 查询已存在的 Release...');
    const getRes = await apiRequest('GET', `/releases/tags/${encodeURIComponent(tag)}`);
    if (!getRes.ok || !getRes.data.id) {
      console.error(`[gitee-release] ERROR: 未找到 tag=${tag} 对应的 Release。请先创建 Release（去掉 --upload-only）。`);
      console.error(`  HTTP ${getRes.status}: ${JSON.stringify(getRes.data).slice(0, 300)}`);
      process.exit(1);
    }
    releaseId = getRes.data.id;
    existingBody = getRes.data.body || '';
    console.log(`[gitee-release] 已找到 Release，ID = ${releaseId}`);
  } else {
    console.log('\n[gitee-release] 尝试创建 Release...');
    let customBody = '';
    if (cli.notesFile && existsSync(cli.notesFile)) {
      customBody = readFileSync(cli.notesFile, 'utf8');
    }
    const createRes = await apiRequest('POST', '/releases', {
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        tag_name: tag,
        name: `nrmm-rust ${tag}`,
        body: customBody || defaultReleaseBody(''),
        target_commitish: commitSha || undefined,
        prerelease: false,
      }),
    });

    if (createRes.ok) {
      releaseId = createRes.data.id;
      console.log(`[gitee-release] Release 创建成功，ID = ${releaseId}`);
    } else {
      console.log(`[gitee-release] 创建返回 HTTP ${createRes.status}（可能已存在），尝试获取...`);
      const getRes = await apiRequest('GET', `/releases/tags/${encodeURIComponent(tag)}`);
      if (getRes.ok && getRes.data.id) {
        releaseId = getRes.data.id;
        existingBody = getRes.data.body || '';
        console.log(`[gitee-release] 已存在 Release，ID = ${releaseId}`);
      } else {
        console.error(`[gitee-release] ERROR: 无法创建或获取 Release。`);
        console.error(`  创建响应: ${JSON.stringify(createRes.data).slice(0, 500)}`);
        console.error(`  查询响应: ${JSON.stringify(getRes.data).slice(0, 300)}`);
        process.exit(1);
      }
    }
  }

  // 收集待上传文件
  const files = findFiles(artifactsDir);
  // CI 中可能存在最新的 latest.json（merge-updater-manifest 生成的）
  const cwdLatest = join(process.cwd(), 'latest.json');
  if (existsSync(cwdLatest) && !files.includes(cwdLatest)) {
    files.push(cwdLatest);
  }

  if (files.length === 0) {
    console.warn('[gitee-release] WARN: 未找到任何待上传文件。');
    console.log(`\n[gitee-release] Release 页面：https://gitee.com/${owner}/${repo}/releases/${tag}`);
    return;
  }

  // 查询已存在的附件，避免重复上传（除非 --force）
  let existingAssets = [];
  if (!cli.force) {
    existingAssets = await listAssets(releaseId);
    console.log(`[gitee-release] Release 当前已有 ${existingAssets.length} 个附件。`);
  }

  const existingNames = new Set(existingAssets.map((a) => a.name));

  let uploaded = 0;
  let skipped = 0;
  console.log(`\n[gitee-release] 准备上传 ${files.length} 个文件...`);
  for (const f of files) {
    const name = basename(f);
    if (!cli.force && existingNames.has(name)) {
      console.log(`  [SKIP] ${name}（已存在，使用 --force 可强制重传）`);
      skipped++;
      continue;
    }
    process.stdout.write(`  [..] ${name} ... `);
    try {
      // 若已存在，先删除旧的
      if (cli.force) {
        const old = existingAssets.find((a) => a.name === name);
        if (old) {
          await deleteAsset(releaseId, old.id);
        }
      }
      const r = await uploadFile(releaseId, f);
      if (r.ok) {
        console.log(`OK (${(r.size / 1024 / 1024).toFixed(2)} MB)`);
        uploaded++;
      } else {
        console.log(`FAIL (HTTP ${r.status})`);
        console.log(`       ${JSON.stringify(r.data).slice(0, 300)}`);
      }
    } catch (err) {
      console.log(`ERROR: ${err.message}`);
    }
  }

  console.log(`\n[gitee-release] 上传完成：${uploaded} 个成功，${skipped} 个跳过。`);
  console.log(`[gitee-release] Release 页面：https://gitee.com/${owner}/${repo}/releases/${tag}`);
}

main().catch((err) => {
  console.error('[gitee-release] FATAL:', err);
  process.exit(1);
});
