#!/usr/bin/env node
/**
 * Gitee Release 创建与附件上传脚本（用于 Gitee Go CI/CD）。
 *
 * 环境变量：
 *   GITEE_TOKEN        — Gitee 私人令牌（必须，需 projects 权限）
 *   GITEE_OWNER        — 仓库所有者（默认 Yezi26）
 *   GITEE_REPO         — 仓库名（默认 nrmm-tauri）
 *   GITEE_TAG          — tag 名称（如 v0.3.0）
 *   GITEE_COMMIT       — 对应的 commit SHA
 *
 * 用法（CI 中）：
 *   node scripts/gitee-release.mjs <artifacts_dir>
 *
 * 产物目录中应包含待上传文件。脚本会上传所有非 .json 文件作为安装包，
 * 若存在 latest.json 则一并上传供 Tauri updater 使用。
 */

import { readFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';

const API_BASE = 'https://gitee.com/api/v5';

const token = process.env.GITEE_TOKEN;
const owner = process.env.GITEE_OWNER || 'Yezi26';
const repo = process.env.GITEE_REPO || 'nrmm-tauri';
const tag = process.env.GITEE_TAG || process.env.GITEE_BRANCH || '';
const commitSha = process.env.GITEE_COMMIT || '';

const artifactsDir = process.argv[2] ? resolve(process.argv[2]) : null;

if (!token) {
  console.error('[gitee-release] ERROR: GITEE_TOKEN 未配置，跳过 Release 创建。');
  console.error('[gitee-release] 构建产物已保存在 Gitee Go 制品库中。');
  process.exit(0);
}

if (!tag) {
  console.error('[gitee-release] ERROR: 无法确定 tag 名（GITEE_TAG / GITEE_BRANCH 均为空）。');
  process.exit(1);
}

async function apiRequest(method, path, options = {}) {
  const url = `${API_BASE}/repos/${owner}/${repo}${path}`;
  const opts = {
    method,
    headers: { 'User-Agent': 'nrmm-rust-ci' },
    ...options,
  };
  const urlWithToken = () => {
    const sep = url.includes('?') ? '&' : '?';
    return `${url}${sep}access_token=${encodeURIComponent(token)}`;
  };
  const fullUrl = urlWithToken();
  const resp = await fetch(fullUrl, opts);
  const text = await resp.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    data = { raw: text };
  }
  if (!resp.ok) {
    return { ok: false, status: resp.status, data };
  }
  return { ok: true, status: resp.status, data };
}

async function uploadFile(releaseId, filePath) {
  const fileName = basename(filePath);
  const stat = statSync(filePath);
  const fileBuffer = readFileSync(filePath);

  const boundary = `----giteeBoundary${Date.now()}`;
  const CRLF = '\r\n';
  const parts = [];
  parts.push(
    `--${boundary}${CRLF}` +
      `Content-Disposition: form-data; name="file"; filename="${fileName}"${CRLF}` +
      `Content-Type: application/octet-stream${CRLF}${CRLF}`
  );
  const pre = Buffer.from(parts.join(''), 'utf8');
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
  return { ok: resp.ok, status: resp.status, data, size: stat.size };
}

async function findFiles(dir) {
  if (!dir || !existsSync(dir)) return [];
  const out = [];
  const entries = readdirSync(dir, { withFileTypes: true });
  for (const ent of entries) {
    const full = join(dir, ent.name);
    if (ent.isFile()) out.push(full);
  }
  return out;
}

async function main() {
  console.log(`[gitee-release] Owner/Repo : ${owner}/${repo}`);
  console.log(`[gitee-release] Tag       : ${tag}`);
  console.log(`[gitee-release] Commit    : ${commitSha || '(unknown)'}`);
  console.log(`[gitee-release] Artifacts : ${artifactsDir || '(none)'}`);

  const releaseBody = [
    `## nrmm-rust ${tag}`,
    '',
    '自动构建发布（Gitee Go 流水线）。',
    '',
    '### Linux 产物',
    '- **AppImage**: `nrmm-rust-x86_64.AppImage` — 通用 Linux 包，下载后 `chmod +x` 即可运行',
    '- **deb**: `nrmm-rust-x86_64.deb` — Debian/Ubuntu 系安装包（`sudo dpkg -i` 安装）',
    '- **rpm**: `nrmm-rust-x86_64.rpm` — Fedora/RHEL 系安装包（`sudo rpm -i` 安装）',
    '',
    '### Windows 产物',
    'Windows 安装包需在 Windows 环境本地构建（需 MSVC + WebView2 SDK）：',
    '```bash',
    'npm ci',
    'npm run tauri build -- --target x86_64-pc-windows-msvc',
    '```',
    '',
    '### 自更新',
    '若附件包含 `latest.json`，Tauri updater 插件可据此检测版本更新。',
  ].join('\n');

  let releaseId = null;
  console.log('\n[gitee-release] 尝试创建 Release...');
  const createRes = await apiRequest('POST', '/releases', {
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      tag_name: tag,
      name: `nrmm-rust ${tag}`,
      body: releaseBody,
      target_commitish: commitSha || undefined,
      prerelease: false,
    }),
  });

  if (createRes.ok) {
    releaseId = createRes.data.id;
    console.log(`[gitee-release] Release 创建成功，ID = ${releaseId}`);
  } else {
    console.log(`[gitee-release] 创建失败（HTTP ${createRes.status}），尝试获取已存在的 Release...`);
    const getRes = await apiRequest('GET', `/releases/tags/${encodeURIComponent(tag)}`);
    if (getRes.ok && getRes.data.id) {
      releaseId = getRes.data.id;
      console.log(`[gitee-release] 已存在 Release，ID = ${releaseId}`);
    } else {
      console.error(`[gitee-release] ERROR: 无法获取 Release，终止上传。`);
      console.error(`  创建响应: ${JSON.stringify(createRes.data).slice(0, 500)}`);
      process.exit(1);
    }
  }

  const files = await findFiles(artifactsDir);
  if (artifactsDir && existsSync(join(process.cwd(), 'latest.json'))) {
    files.push(join(process.cwd(), 'latest.json'));
  }
  // 还包含当前目录下的 latest.json
  const localLatest = join(process.cwd(), 'latest.json');
  if (!files.includes(localLatest) && existsSync(localLatest)) {
    files.push(localLatest);
  }

  if (files.length === 0) {
    console.warn('[gitee-release] WARN: 未找到任何待上传文件。');
  } else {
    console.log(`\n[gitee-release] 上传 ${files.length} 个附件...`);
    for (const f of files) {
      const name = basename(f);
      process.stdout.write(`  [..] ${name} ... `);
      try {
        const r = await uploadFile(releaseId, f);
        if (r.ok) {
          console.log(`OK (${r.size} bytes)`);
        } else {
          console.log(`FAIL (HTTP ${r.status})`);
          console.log(`       ${JSON.stringify(r.data).slice(0, 200)}`);
        }
      } catch (err) {
        console.log(`ERROR: ${err.message}`);
      }
    }
  }

  console.log(`\n[gitee-release] 完成！Release 页面：`);
  console.log(`  https://gitee.com/${owner}/${repo}/releases/${tag}`);
}

main().catch((err) => {
  console.error('[gitee-release] FATAL:', err);
  process.exit(1);
});
