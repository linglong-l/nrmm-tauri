#!/usr/bin/env node
/**
 * merge-updater-manifest.mjs
 *
 * 多平台 latest.json 合并脚本（零依赖，仅使用 Node.js 内置模块）。
 *
 * 用法：
 *   node scripts/merge-updater-manifest.mjs <file1> <file2> ... [-o output.json]
 *
 * 功能：
 *   - 读取多个平台构建产出的 latest.json 文件
 *   - 合并 platforms 键（Windows 的 nsis/windows-x86_64 与 Linux 的 appimage/deb/rpm/linux-x86_64 互不覆盖）
 *   - version/notes/pub_date 取最新版本（按语义版本号比较）
 *   - 输出合并后的 JSON 到 stdout 或指定输出文件
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const args = process.argv.slice(2);

let outputPath = null;
const inputPaths = [];

for (let i = 0; i < args.length; i++) {
  if (args[i] === '-o' || args[i] === '--output') {
    outputPath = args[++i];
  } else if (!args[i].startsWith('-')) {
    inputPaths.push(args[i]);
  }
}

if (inputPaths.length < 1) {
  console.error('用法: node merge-updater-manifest.mjs <file1> [file2 ...] [-o output.json]');
  console.error('  至少需要一个 latest.json 文件作为输入');
  process.exit(1);
}

function parseVersion(v) {
  const m = String(v).match(/^v?(\d+)\.(\d+)\.(\d+)/);
  if (!m) return [0, 0, 0];
  return [parseInt(m[1]), parseInt(m[2]), parseInt(m[3])];
}

function compareVersions(a, b) {
  const va = parseVersion(a);
  const vb = parseVersion(b);
  for (let i = 0; i < 3; i++) {
    if (va[i] !== vb[i]) return va[i] - vb[i];
  }
  return 0;
}

let merged = {
  version: '0.0.0',
  notes: '',
  pub_date: '',
  platforms: {}
};

for (const inputPath of inputPaths) {
  const abs = resolve(process.cwd(), inputPath);
  let data;
  try {
    data = JSON.parse(readFileSync(abs, 'utf-8'));
  } catch (e) {
    console.error(`读取或解析失败: ${inputPath} - ${e.message}`);
    process.exit(1);
  }

  // Merge platforms
  if (data.platforms && typeof data.platforms === 'object') {
    for (const [key, value] of Object.entries(data.platforms)) {
      if (merged.platforms[key]) {
        console.warn(`警告: 平台 ${key} 在多个文件中出现，以最后一个为准`);
      }
      merged.platforms[key] = value;
    }
  }

  // Take latest version
  if (data.version && compareVersions(data.version, merged.version) > 0) {
    merged.version = data.version;
    merged.notes = data.notes || '';
    merged.pub_date = data.pub_date || '';
  } else if (data.version && compareVersions(data.version, merged.version) === 0) {
    // Same version: prefer non-empty notes/pub_date
    if (!merged.notes && data.notes) merged.notes = data.notes;
    if (!merged.pub_date && data.pub_date) merged.pub_date = data.pub_date;
  }
}

// Validate merged result
const platformKeys = Object.keys(merged.platforms);
if (platformKeys.length === 0) {
  console.error('错误: 合并后 platforms 为空，请检查输入文件');
  process.exit(1);
}

// Check required signature fields
for (const [key, plat] of Object.entries(merged.platforms)) {
  if (!plat.url) {
    console.error(`错误: 平台 ${key} 缺少 url 字段`);
    process.exit(1);
  }
  if (!plat.signature) {
    console.error(`错误: 平台 ${key} 缺少 signature 字段`);
    process.exit(1);
  }
}

const output = JSON.stringify(merged, null, 2) + '\n';

if (outputPath) {
  writeFileSync(resolve(process.cwd(), outputPath), output, 'utf-8');
  console.error(`✅ 已合并 ${inputPaths.length} 个文件 -> ${outputPath}`);
  console.error(`   版本: v${merged.version}, 平台数: ${platformKeys.length}`);
} else {
  process.stdout.write(output);
}
