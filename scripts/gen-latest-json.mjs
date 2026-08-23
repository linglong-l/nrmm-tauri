#!/usr/bin/env node
/**
 * gen-latest-json.mjs
 *
 * 生成 Tauri v2 更新清单 latest.json（updater manifest）。
 *
 * Tauri 的 `tauri build` 本身【不会】写出 latest.json —— 它只会在配置 signing 密钥后
 * 为安装包附带 .sig 签名文件；latest.json 需自行生成（或借助 Tauri Action）。
 * 本脚本从 dist-win/ 中已签名的安装包推导 manifest：
 *   1. 读取版本号（package.json）与产品名；
 *   2. 定位 Windows 安装包（默认 NSIS setup.exe，updater 的目标安装器）；
 *   3. 若缺失 .sig 则调用 `tauri signer sign` 现场签名（复用环境内的签名密钥）；
 *   4. 按 Tauri v2 规范写出 latest.json（及 latest-windows.json 兼容副本）。
 *
 * 托管约定（与 build-windows.ps1 一致）：产物上传到 Gitee Release，tag=v<VERSION>。
 * 可通过环境变量覆盖：
 *   UPDATE_BASE_URL  产物基础 URL（默认由 gitee remote 推导）
 *   UPDATE_NOTES     更新说明（默认简短占位）
 *   DIST_DIR         产物目录（默认 dist-win）
 *   ARCH_SUFFIX      目标架构后缀（默认 x86_64；i686 构建时传 i686，决定安装包名与平台键）
 *
 * 用法：
 *   node scripts/gen-latest-json.mjs
 */

import {
  existsSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
} from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawnSync } from 'node:child_process';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');

// ---------- 版本 / 产品名 ----------
const pkg = JSON.parse(readFileSync(join(ROOT, 'package.json'), 'utf8'));
const VERSION = String(pkg.version);
if (!/^\d+\.\d+\.\d+/.test(VERSION)) {
  console.error(`[latest] ERROR: 非法版本号 "${VERSION}"`);
  process.exit(1);
}
const PRODUCT = pkg.name || 'nrmm-rust';
// 目标架构后缀（由 build-windows.ps1 透传）；默认 x86_64，i686 构建时传 i686
const archSuffix = process.env.ARCH_SUFFIX || 'x86_64';

// ---------- 产物目录 ----------
const DIST = resolve(ROOT, process.env.DIST_DIR || 'dist-win');
if (!existsSync(DIST)) {
  console.error(`[latest] ERROR: 产物目录不存在: ${DIST}`);
  process.exit(1);
}

// ---------- 托管基础 URL ----------
function deriveBase() {
  try {
    const url = execFileSync('git', ['remote', 'get-url', 'gitee'], {
      cwd: ROOT,
      encoding: 'utf8',
    })
      .trim()
      .replace(/\.git$/, '');
    const m = url.match(/[:/]([^/]+)\/([^/]+?)$/);
    if (m) {
      const host = /gitee/i.test(url) ? 'gitee.com' : /github/i.test(url) ? 'github.com' : 'gitee.com';
      return `https://${host}/${m[1]}/${m[2]}/releases/download/v${VERSION}`;
    }
  } catch {
    /* ignore */
  }
  return `https://gitee.com/Yezi26/nrmm-tauri/releases/download/v${VERSION}`;
}
const BASE = (process.env.UPDATE_BASE_URL || deriveBase()).replace(/\/+$/, '');

const NOTES = process.env.UPDATE_NOTES || `NRMM ${VERSION} 更新。`;
const PUB_DATE = new Date().toISOString();

// ---------- Windows 安装包（NSIS setup.exe 是 updater 目标） ----------
// 仅引用一个 windows-${archSuffix} 条目；NSIS 被动安装（installMode: passive）即基于此 setup.exe。
const PRIMARY = `nrmm-rust-${archSuffix}-setup.exe`;
const artifactPath = join(DIST, PRIMARY);
if (!existsSync(artifactPath)) {
  console.error(`[latest] ERROR: 找不到 Windows 安装包: ${artifactPath}`);
  console.error('  请先构建并收集产物到 dist-win/（npm run build:windows）');
  process.exit(1);
}

// ---------- 确保 .sig 存在（缺失则现场签名） ----------
function ensureSig(artifact) {
  const sig = `${artifact}.sig`;
  if (existsSync(sig)) {
    console.log(`[latest] 已有签名: ${PRODUCT} -> ${sig.split(/[\\/]/).pop()}`);
    return sig;
  }
  console.log(`[latest] 签名: ${artifact.split(/[\\/]/).pop()}`);
  const env = { ...process.env };
  // `tauri signer sign` 子命令下 PRIVATE_KEY 与 PRIVATE_KEY_PATH 互斥，二选一：
  if (env.TAURI_SIGNING_PRIVATE_KEY && env.TAURI_SIGNING_PRIVATE_KEY_PATH) {
    delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
  }
  // 通过系统 shell 调用 npx（Windows 仅有 npx.cmd，无 npx.exe，需 shell 解析）
  const r = spawnSync(`npx --no-install tauri signer sign "${artifact}"`, {
    cwd: ROOT,
    env,
    stdio: 'inherit',
    shell: true,
  });
  if (r.status !== 0 || !existsSync(sig)) {
    console.error('[latest] ERROR: 签名失败，请确认 TAURI_SIGNING_PRIVATE_KEY / _PASSWORD 已配置');
    process.exit(r.status || 1);
  }
  return sig;
}

const sigPath = ensureSig(artifactPath);
const signature = readFileSync(sigPath, 'utf8').trim();
if (!signature) {
  console.error('[latest] ERROR: 签名文件为空');
  process.exit(1);
}

// ---------- 组装 manifest ----------
const manifest = {
  version: VERSION,
  notes: NOTES,
  pub_date: PUB_DATE,
  platforms: {
    [`windows-${archSuffix}`]: {
      signature,
      url: `${BASE}/${PRIMARY}`,
    },
  },
};

const json = JSON.stringify(manifest, null, 2) + '\n';
const outMain = join(DIST, 'latest.json');
const outWin = join(DIST, 'latest-windows.json');
writeFileSync(outMain, json);
writeFileSync(outWin, json);
const platformKey = `windows-${archSuffix}`;
console.log(`[latest] ✅ 已生成: ${outMain}`);
console.log(`[latest] ✅ 已生成: ${outWin}`);
console.log('[latest] 平台条目:');
console.log(`  ${platformKey} -> ${manifest.platforms[platformKey].url}`);
console.log('');
console.log('[latest] ⚠️ 发布前请确认：');
console.log('  1. 将 dist-win/ 全部内容（含 *.sig 与 latest.json）上传到 Gitee Release tag v' + VERSION);
console.log('  2. 在 tauri.*.conf.json 的 plugins.updater.endpoints 填入 latest.json 的托管 URL');
console.log('     （例如 ' + BASE + '/latest.json ）');
