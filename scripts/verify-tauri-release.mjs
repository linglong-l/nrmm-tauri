#!/usr/bin/env node
/**
 * verify-tauri-release.mjs
 *
 * Tauri 发布前/后校验脚本（零依赖，仅使用 Node.js 内置模块）。
 *
 * 用法：
 *   node scripts/verify-tauri-release.mjs pre           # 构建前校验（版本号一致、配置正确）
 *   node scripts/verify-tauri-release.mjs post <path>   # 构建后校验（latest.json 完整性）
 */

import { readFileSync, existsSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const PROJECT_ROOT = resolve(__dirname, '..');

const RED = '\x1b[31m';
const GREEN = '\x1b[32m';
const YELLOW = '\x1b[33m';
const RESET = '\x1b[0m';

function pass(msg) {
  console.log(`${GREEN}  ✅ ${msg}${RESET}`);
}

function fail(msg) {
  console.error(`${RED}  ❌ ${msg}${RESET}`);
  process.exitCode = 1;
}

function warn(msg) {
  console.log(`${YELLOW}  ⚠️  ${msg}${RESET}`);
}

function readJSON(relPath) {
  const abs = resolve(PROJECT_ROOT, relPath);
  if (!existsSync(abs)) {
    fail(`文件不存在: ${relPath}`);
    return null;
  }
  try {
    return JSON.parse(readFileSync(abs, 'utf-8'));
  } catch (e) {
    fail(`解析 JSON 失败 ${relPath}: ${e.message}`);
    return null;
  }
}

function readCargoVersion(relPath) {
  const abs = resolve(PROJECT_ROOT, relPath);
  if (!existsSync(abs)) {
    fail(`文件不存在: ${relPath}`);
    return null;
  }
  const content = readFileSync(abs, 'utf-8');
  const match = content.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    fail(`Cargo.toml 中未找到 [package] 下的 version 字段: ${relPath}`);
    return null;
  }
  return match[1];
}

const EXPECTED_PUBKEY_PREFIX = 'dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6';
const EXPECTED_TARGETS = ['nsis', 'msi', 'deb', 'appimage', 'rpm'];

function verifyPre() {
  console.log('\n🔍 Pre-build verification:');
  let hasError = false;

  const pkg = readJSON('package.json');
  const tauriConf = readJSON('src-tauri/tauri.conf.json');
  const cargoVersion = readCargoVersion('src-tauri/Cargo.toml');

  if (!pkg || !tauriConf || !cargoVersion) return false;

  // Check 1: version consistency
  const pkgVersion = pkg.version;
  const tauriVersion = tauriConf.version;

  if (pkgVersion === tauriVersion && tauriVersion === cargoVersion) {
    pass(`三处版本号一致: v${pkgVersion}`);
  } else {
    fail(`版本号不一致！package.json=${pkgVersion}, Cargo.toml=${cargoVersion}, tauri.conf.json=${tauriVersion}`);
    hasError = true;
  }

  // Check 2: pubkey
  const pubkey = tauriConf.plugins?.updater?.pubkey;
  if (pubkey && typeof pubkey === 'string' && pubkey.startsWith(EXPECTED_PUBKEY_PREFIX)) {
    pass(`pubkey 配置正确 (长度: ${pubkey.length})`);
  } else {
    fail(`pubkey 缺失或格式错误`);
    hasError = true;
  }

  // Check 3: endpoints (optional — custom update logic may omit this)
  const endpoints = tauriConf.plugins?.updater?.endpoints;
  if (endpoints === undefined || endpoints === null) {
    pass(`updater endpoints 未配置（使用自定义更新逻辑，符合预期）`);
  } else if (Array.isArray(endpoints) && endpoints.length > 0 && endpoints.every(e => typeof e === 'string' && e.startsWith('http'))) {
    pass(`updater endpoints 配置正确 (${endpoints.length} 个端点)`);
  } else {
    fail(`updater endpoints 配置无效: ${JSON.stringify(endpoints)}`);
    hasError = true;
  }

  // Check 4: bundle targets
  const targets = tauriConf.bundle?.targets;
  if (Array.isArray(targets)) {
    const missing = EXPECTED_TARGETS.filter(t => !targets.includes(t));
    if (missing.length === 0) {
      pass(`bundle targets 包含全部平台: ${EXPECTED_TARGETS.join(', ')}`);
    } else {
      warn(`bundle targets 缺少平台: ${missing.join(', ')} (当前: ${targets.join(', ')})`);
    }
  } else {
    fail(`bundle targets 未配置`);
    hasError = true;
  }

  return !hasError;
}

function verifyPost(latestJsonPath) {
  console.log(`\n🔍 Post-build verification (${latestJsonPath}):`);
  let hasError = false;

  const tauriConf = readJSON('src-tauri/tauri.conf.json');
  const latest = readJSON(latestJsonPath);

  if (!tauriConf || !latest) return false;

  const expectedVersion = tauriConf.version;

  // Check 1: version matches
  if (latest.version === expectedVersion) {
    pass(`latest.json version 匹配: v${latest.version}`);
  } else {
    fail(`latest.json version=${latest.version} 与 tauri.conf.json version=${expectedVersion} 不一致`);
    hasError = true;
  }

  // Check 2: platforms
  const platforms = latest.platforms;
  if (!platforms || typeof platforms !== 'object') {
    fail(`latest.json platforms 缺失或格式错误`);
    return false;
  }

  const platformKeys = Object.keys(platforms);
  if (platformKeys.length === 0) {
    fail(`latest.json platforms 为空`);
    hasError = true;
  } else {
    pass(`platforms 包含 ${platformKeys.length} 个平台: ${platformKeys.join(', ')}`);
    for (const key of platformKeys) {
      const p = platforms[key];
      if (p.url && p.signature) {
        pass(`  ${key}: url=${p.url.substring(0, 60)}..., signature=${p.signature.substring(0, 30)}...`);
      } else {
        fail(`  ${key}: url 或 signature 缺失`);
        hasError = true;
      }
    }
  }

  return !hasError;
}

// Main
const mode = process.argv[2];

if (!mode || (mode !== 'pre' && mode !== 'post')) {
  console.error('用法: node verify-tauri-release.mjs <pre|post> [latest.json path]');
  console.error('  pre  - 构建前校验（版本号、pubkey、endpoints、targets）');
  console.error('  post - 构建后校验（latest.json 完整性）');
  process.exit(1);
}

if (mode === 'pre') {
  const ok = verifyPre();
  if (ok) {
    console.log(`\n${GREEN}🎉 Pre-build verification passed!${RESET}\n`);
  } else {
    console.log(`\n${RED}💥 Pre-build verification failed!${RESET}\n`);
    process.exit(1);
  }
} else {
  const latestPath = process.argv[3];
  if (!latestPath) {
    console.error('post 模式需要指定 latest.json 路径');
    process.exit(1);
  }
  const ok = verifyPost(resolve(PROJECT_ROOT, latestPath));
  if (ok) {
    console.log(`\n${GREEN}🎉 Post-build verification passed!${RESET}\n`);
  } else {
    console.log(`\n${RED}💥 Post-build verification failed!${RESET}\n`);
    process.exit(1);
  }
}
