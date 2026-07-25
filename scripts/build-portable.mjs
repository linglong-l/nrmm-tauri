#!/usr/bin/env node
/**
 * build-portable.mjs
 *
 * 生成免安装（portable）分发包：
 *   - Windows: nrmm-rust-portable-x86_64.zip
 *   - Linux:   nrmm-rust-portable-x86_64.tar.gz
 *
 * 零第三方依赖，使用系统命令（Windows: PowerShell/Compress-Archive；Linux: tar）。
 *
 * 用法：
 *   node scripts/build-portable.mjs                # 自动检测平台，输出到 dist/
 *   node scripts/build-portable.mjs --target x86_64-pc-windows-msvc -o dist-win
 *   node scripts/build-portable.mjs --target x86_64-unknown-linux-gnu -o dist
 */

import { existsSync, readdirSync, statSync, mkdirSync, writeFileSync, rmSync, cpSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync, spawnSync } from 'node:child_process';
import { Buffer } from 'node:buffer';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const PROJECT_ROOT = resolve(__dirname, '..');

// ---------- CLI 参数 ----------
function parseArgs(argv) {
  const args = { target: '', outDir: '' };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--target') args.target = argv[++i] || '';
    else if (a === '-o' || a === '--output') args.outDir = argv[++i] || '';
  }
  return args;
}
const cli = parseArgs(process.argv.slice(2));

function detectTarget() {
  const p = process.platform, a = process.arch;
  if (p === 'win32' && a === 'x64') return 'x86_64-pc-windows-msvc';
  if (p === 'linux' && a === 'x64') return 'x86_64-unknown-linux-gnu';
  if (p === 'darwin' && a === 'x64') return 'x86_64-apple-darwin';
  if (p === 'darwin' && a === 'arm64') return 'aarch64-apple-darwin';
  console.error(`[portable] 无法自动检测平台 ${p}-${a}，请用 --target 指定`);
  process.exit(1);
}

const TARGET = cli.target || detectTarget();
const OUT_DIR = cli.outDir
  ? resolve(PROJECT_ROOT, cli.outDir)
  : resolve(PROJECT_ROOT, 'dist');
const RELEASE_DIR = resolve(PROJECT_ROOT, 'src-tauri/target', TARGET, 'release');
const isWindows = TARGET.includes('windows-msvc');
const exeExt = isWindows ? '.exe' : '';
const mainExe = `nrmm-rust${exeExt}`;

console.log(`[portable] Target:  ${TARGET}`);
console.log(`[portable] Release: ${RELEASE_DIR}`);
console.log(`[portable] Output:  ${OUT_DIR}`);

if (!existsSync(RELEASE_DIR)) {
  console.error(`[portable] ERROR: Release 目录不存在: ${RELEASE_DIR}`);
  console.error(`  请先运行: npm run tauri build -- --target ${TARGET}`);
  process.exit(1);
}
mkdirSync(OUT_DIR, { recursive: true });

// ---------- 1. 构建临时 staging 目录 ----------
const stageDir = resolve(OUT_DIR, `.portable-stage-${Date.now()}`);
mkdirSync(stageDir, { recursive: true });

let fileCount = 0;
function copyTo(src, destRel) {
  if (!existsSync(src)) { console.log(`  [SKIP] ${destRel} (not found)`); return; }
  const dest = join(stageDir, destRel);
  mkdirSync(dirname(dest), { recursive: true });
  cpSync(src, dest, { recursive: true });
  const s = statSync(src);
  console.log(`  [ADD]  ${destRel} (${(s.size / 1024 / 1024).toFixed(2)} MB)`);
  fileCount++;
}
function addDirContents(srcDir, destBase) {
  if (!existsSync(srcDir)) return;
  for (const ent of readdirSync(srcDir, { withFileTypes: true })) {
    const sp = join(srcDir, ent.name);
    const dp = join(destBase, ent.name);
    if (ent.isFile()) copyTo(sp, dp);
    else if (ent.isDirectory()) addDirContents(sp, dp);
  }
}

console.log('[portable] 收集文件...');
copyTo(join(RELEASE_DIR, mainExe), mainExe);

// Windows DLL
if (isWindows) {
  for (const ent of readdirSync(RELEASE_DIR, { withFileTypes: true })) {
    if (ent.isFile() && ent.name.toLowerCase().endsWith('.dll') && ent.name.toLowerCase() !== mainExe.toLowerCase()) {
      copyTo(join(RELEASE_DIR, ent.name), ent.name);
    }
  }
}

// Tauri resources 目录（如果有 sidecar/resources 配置）
const resDir = join(RELEASE_DIR, 'resources');
if (existsSync(resDir)) addDirContents(resDir, 'resources');

// README
const readmeContent = isWindows
  ? [
      'nrmm-rust 免安装版（Portable）',
      '',
      '使用方法：',
      '  1. 解压到任意目录',
      '  2. 双击 nrmm-rust.exe 运行',
      '  3. 配置文件保存在 %LOCALAPPDATA%\\nrmm-rust\\',
      '',
      '注意：本版本不写入注册表、不创建开始菜单/桌面快捷方式。',
      '卸载方法：直接删除本文件夹即可。',
      ''
    ].join('\r\n')
  : [
      'nrmm-rust Portable',
      '',
      'Usage:',
      '  1. Extract to any directory',
      '  2. chmod +x nrmm-rust && ./nrmm-rust',
      '  3. Config stored in ~/.local/share/nrmm-rust/',
      '',
      'Note: This build does not install to system paths.',
      'To remove, simply delete this folder.',
      ''
    ].join('\n');
writeFileSync(join(stageDir, 'README.txt'), readmeContent, 'utf8');
console.log(`  [ADD]  README.txt`);
fileCount++;

if (fileCount === 0) {
  console.error('[portable] ERROR: 没有可打包的文件');
  process.exit(1);
}

// ---------- 2. 压缩 ----------
const archiveName = isWindows
  ? 'nrmm-rust-portable-x86_64.zip'
  : 'nrmm-rust-portable-x86_64.tar.gz';
const archivePath = resolve(OUT_DIR, archiveName);

console.log(`[portable] 创建压缩包: ${archiveName}`);
if (existsSync(archivePath)) rmSync(archivePath);

if (isWindows) {
  function psEscapeQuote(str) {
    return str.replace(/'/g, "''");
  }
  const psCmd = [
    `Add-Type -AssemblyName System.IO.Compression.FileSystem`,
    `$s = '${psEscapeQuote(stageDir)}'`,
    `$a = '${psEscapeQuote(archivePath)}'`,
    `$l = [System.IO.Compression.CompressionLevel]::Optimal`,
    `[System.IO.Compression.ZipFile]::CreateFromDirectory($s, $a, $l, $false)`,
  ].join('; ');
  const r = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', psCmd], {
    stdio: 'inherit',
    windowsVerbatimArguments: false,
  });
  if (r.status !== 0) {
    console.error(`[portable] ERROR: PowerShell 压缩失败 (exit code: ${r.status})`);
    process.exit(r.status || 1);
  }
} else {
  const r = spawnSync('tar', ['-czf', archivePath, '-C', stageDir, '.'], {
    stdio: 'inherit',
  });
  if (r.status !== 0) {
    console.error(`[portable] ERROR: tar 压缩失败 (exit code: ${r.status})`);
    process.exit(r.status || 1);
  }
}

// ---------- 3. 清理 staging ----------
rmSync(stageDir, { recursive: true, force: true });

const size = (statSync(archivePath).size / 1024 / 1024).toFixed(2);
console.log(`[portable] ✅ 完成: ${archivePath} (${size} MB)`);
