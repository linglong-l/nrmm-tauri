#!/usr/bin/env node
/**
 * build-portable.mjs
 *
 * 生成免安装（portable）分发包（压缩包名按 nrmm-tauri_<版本>_x64-<系统> 规范派生）：
 *   - Windows: nrmm-tauri_<ver>_x64-Win.zip
 *   - Linux:   nrmm-tauri_<ver>_x64-Linux.tar.gz        （含内置 7z，官方发布形态）
 *
 * 所有免安装产物默认均内置 7z CLI（位于 exe 同级 7z/ 目录），以保证完整归档格式支持。
 * `--no-7z` 仅为使用者自行抉择的可选形态（不随官方发布流程产出）：不带内置 7z 时
 * 解压回退仅支持 zip/7z/rar 并优先调用系统 7z，由此引发的异常由使用者自行承担。
 *
 * 零第三方依赖，使用系统命令（Windows: PowerShell/Compress-Archive；Linux: tar）。
 *
 * 用法：
 *   node scripts/build-portable.mjs                # 自动检测平台，输出到 dist/
 *   node scripts/build-portable.mjs --target x86_64-pc-windows-msvc -o dist-win
 *   node scripts/build-portable.mjs --target x86_64-unknown-linux-gnu -o dist
 *   node scripts/build-portable.mjs --no-7z        # 可选：单程序版（不含内置 7z，自行承担回退限制）
 */

import { existsSync, readdirSync, statSync, mkdirSync, writeFileSync, rmSync, cpSync, readFileSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync, spawnSync } from 'node:child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const PROJECT_ROOT = resolve(__dirname, '..');

// ---------- CLI 参数 ----------
function parseArgs(argv) {
  const args = { target: '', outDir: '', no7z: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--target') args.target = argv[++i] || '';
    else if (a === '-o' || a === '--output') args.outDir = argv[++i] || '';
    else if (a === '--no-7z') args.no7z = true;
  }
  return args;
}
const cli = parseArgs(process.argv.slice(2));

function detectTarget() {
  const p = process.platform, a = process.arch;
  if (p === 'win32' && a === 'x64') return 'x86_64-pc-windows-msvc';
  if (p === 'linux' && a === 'x64') return 'x86_64-unknown-linux-gnu';
  console.error(`[portable] 当前仅支持 x86_64（${p}-${a}），请用 --target 指定`);
  process.exit(1);
}

const TARGET = cli.target || detectTarget();
const isWindows = TARGET.includes('windows-msvc');
const osToken = isWindows ? 'Win' : 'Linux';
const archToken = 'x64'; // 当前仅支持 x86_64
const OUT_DIR = cli.outDir
  ? resolve(PROJECT_ROOT, cli.outDir)
  : resolve(PROJECT_ROOT, 'dist');
// Release 目录探测（2026-08-18）：根 Cargo.toml 为 workspace，cargo 产物默认落在根 target/；
// 旧布局（无 workspace 或 CI 环境）落在 src-tauri/target/。按优先级探测，两处皆无才报错。
function detectReleaseDir() {
  const candidates = [
    resolve(PROJECT_ROOT, 'target', TARGET, 'release'),
    resolve(PROJECT_ROOT, 'src-tauri/target', TARGET, 'release'),
  ];
  for (const dir of candidates) {
    if (existsSync(dir)) return dir;
  }
  return candidates[0];
}
const RELEASE_DIR = detectReleaseDir();
const exeExt = isWindows ? '.exe' : '';
const mainExe = `nrmm-tauri${exeExt}`;

// 版本号（与 package.json 一致，作为命名规范的根基）
const pkgVersion = JSON.parse(
  readFileSync(resolve(PROJECT_ROOT, 'package.json'), 'utf8')
).version;

console.log(`[portable] Target:  ${TARGET}`);
console.log(`[portable] Release: ${RELEASE_DIR}`);
console.log(`[portable] Output:  ${OUT_DIR}`);

if (!existsSync(RELEASE_DIR)) {
  console.error(`[portable] ERROR: Release 目录不存在: ${RELEASE_DIR}`);
  // 注意：不得写 `npm run tauri build -- --target X`——npm 会把 `--target` 当本身配置吞掉。
  console.error(`  请先运行: npx tauri build --target ${TARGET}`);
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

// Windows DLL（vcruntime 等随 cargo build 落盘于 release 目录）
if (isWindows) {
  for (const ent of readdirSync(RELEASE_DIR, { withFileTypes: true })) {
    if (ent.isFile() && ent.name.toLowerCase().endsWith('.dll') && ent.name.toLowerCase() !== mainExe.toLowerCase()) {
      copyTo(join(RELEASE_DIR, ent.name), ent.name);
    }
  }
}

// ---------- 内置 7z（便携版落在 exe_dir/7z/，与 get_bundled_7z_path 探测一致）----------
// 资源由 Tauri bundler 注入到各安装包，但 portable 不走 bundle，需手动拷贝。
// 默认含 7z（推荐）；--no-7z 时跳过，产出单程序版（回退仅支持 zip/7z/rar，优先系统 7z）。
if (!cli.no7z) {
  console.log('[portable] 拷贝内置 7z 到 stage/7z/ ...');
  if (isWindows) {
    const srcRoot = resolve(PROJECT_ROOT, 'src-tauri/binaries/7z/Windows-x64');
    for (const f of ['7za.exe', '7za.dll', '7zxa.dll']) {
      copyTo(join(srcRoot, f), join('7z', f));
    }
  } else {
    const srcRoot = resolve(PROJECT_ROOT, 'src-tauri/binaries/7z/linux-x64');
    copyTo(join(srcRoot, '7zz'), join('7z', '7zz'));
  }
} else {
  console.log('[portable] --no-7z：跳过内置 7z（单程序版）');
}

// README（内置 7z 说明：免安装版一律内置 7z CLI，不得出现「允许删除」的表述）
const sevenZipNote = cli.no7z
  ? [
      '',
      '内置 7z：本版本未附带内置 7z（使用者自行抉择，非官方发布形态）。',
      '  解压回退仅支持 zip/7z/rar，且优先调用系统已安装的 7z；由此产生的异常由使用者自行承担。',
      '  如需完整归档格式支持，请改用内置 7z 的官方版本，或在系统 PATH 安装 7-Zip。',
      ''
    ]
  : [
      '',
      '内置 7z：本目录含 7z/ 子目录（Windows: 7za.exe + 7za.dll + 7zxa.dll；Linux: 7zz），为完整归档格式支持所必需。',
      '  程序启动后自动探测该目录；请勿移除或改动 7z/ 下的任何文件。',
      '  一旦缺失，解压回退将降级为仅支持 zip/7z/rar 并优先调用系统已安装的 7z；',
      '  若系统亦未安装 7-Zip，则进一步回退到内置纯 Rust 解压（支持格式更少、性能较弱）。',
      ''
    ];
const readmeContent = isWindows
  ? [
      'nrmm-tauri 免安装版（Portable）',
      '',
      '使用方法：',
      '  1. 解压到任意目录',
      '  2. 双击 nrmm-tauri.exe 运行',
      '  3. 配置文件保存在 %LOCALAPPDATA%\\nrmm-tauri\\',
      '',
      '注意：本版本不写入注册表、不创建开始菜单/桌面快捷方式。',
      '卸载方法：直接删除本文件夹即可。',
      ...sevenZipNote,
      ''
    ].join('\r\n')
  : [
      'nrmm-tauri Portable',
      '',
      'Usage:',
      '  1. Extract to any directory',
      '  2. chmod +x nrmm-tauri && ./nrmm-tauri',
      '  3. Config stored in ~/.local/share/nrmm-tauri/',
      '',
      'Note: This build does not install to system paths.',
      'To remove, simply delete this folder.',
      ...sevenZipNote,
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
const baseName = `nrmm-tauri_${pkgVersion}_${archToken}-${osToken}`;
const archiveName = isWindows
  ? `${baseName}.zip`
  : cli.no7z
    ? `${baseName}-no7z.tar.gz`
    : `${baseName}.tar.gz`;
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
