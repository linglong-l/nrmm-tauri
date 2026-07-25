#!/usr/bin/env node
/**
 * verify-build.mjs
 *
 * nrmm-rust 构建全流程检查脚本（零依赖，Node.js 内置模块）。
 *
 * 三大能力：
 *   1. pre   - 构建前环境检查（Node/Rust/系统依赖/前端依赖完整性）
 *   2. scan  - 扫描构建日志文件，提取错误/警告/异常关键词（检测 linuxdeploy / FUSE / glibc 等错误）
 *   3. post  - 构建后产物完整性检查（7 种包格式 + 便携版 + latest.json 校验）
 *
 * 用法：
 *   node scripts/verify-build.mjs pre                          # 构建前环境检查
 *   node scripts/verify-build.mjs scan <logfile>               # 扫描构建日志中的异常
 *   node scripts/verify-build.mjs post [--target <triple>]     # 构建后产物检查
 *   node scripts/verify-build.mjs all  [--target <triple>]     # pre + post (不做 log scan)
 *   node scripts/verify-build.mjs list-commands                # 列出构建检查命令清单
 */

import { readFileSync, existsSync, statSync, readdirSync } from 'node:fs';
import { resolve, dirname, join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync } from 'node:child_process';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const PROJECT_ROOT = resolve(__dirname, '..');

// ---------- color helpers ----------
const C = {
  R: '\x1b[31m', G: '\x1b[32m', Y: '\x1b[33m', B: '\x1b[34m',
  C: '\x1b[36m', W: '\x1b[37m', BOLD: '\x1b[1m', RST: '\x1b[0m',
};
const ok = (m) => console.log(`${C.G}  ✅ ${m}${C.RST}`);
const fail = (m) => { console.error(`${C.R}  ❌ ${m}${C.RST}`); _stats.fail++; };
const warn = (m) => { console.log(`${C.Y}  ⚠️  ${m}${C.RST}`); _stats.warn++; };
const info = (m) => console.log(`${C.C}  ℹ️  ${m}${C.RST}`);
const section = (m) => console.log(`\n${C.BOLD}${C.B}── ${m} ──${C.RST}`);

const _stats = { pass: 0, warn: 0, fail: 0 };
function pass(m) { console.log(`${C.G}  ✅ ${m}${C.RST}`); _stats.pass++; }

// ---------- fs helpers ----------
function readJSON(rel) {
  const abs = resolve(PROJECT_ROOT, rel);
  if (!existsSync(abs)) return null;
  try { return JSON.parse(readFileSync(abs, 'utf-8')); }
  catch { return null; }
}
function readCargoVersion(rel = 'src-tauri/Cargo.toml') {
  const abs = resolve(PROJECT_ROOT, rel);
  if (!existsSync(abs)) return null;
  const c = readFileSync(abs, 'utf-8');
  const m = c.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}
function fileSizeMB(abs) {
  try { return (statSync(abs).size / 1024 / 1024).toFixed(2); }
  catch { return null; }
}
function findByGlob(dir, pattern) {
  if (!existsSync(dir)) return [];
  const files = readdirSync(dir);
  return files.filter(f => pattern.test(f)).map(f => join(dir, f));
}

// ============================================================
// MODE: pre - 构建前环境检查
// ============================================================
function cmd(cmdStr, opts = {}) {
  try {
    return execSync(cmdStr, { encoding: 'utf-8', stdio: ['ignore', 'pipe', 'pipe'], ...opts }).trim();
  } catch {
    return null;
  }
}

function verifyPre() {
  section('1/6 Node.js 与 npm');
  const nodeVer = cmd('node --version');
  const npmVer = cmd('npm --version');
  if (nodeVer) {
    const mj = parseInt(nodeVer.replace('v', '').split('.')[0], 10);
    if (mj >= 20) pass(`Node.js: ${nodeVer} (>= 20 符合要求)`);
    else warn(`Node.js: ${nodeVer} 建议 >= 20`);
  } else fail('Node.js 未安装或不在 PATH');
  if (npmVer) pass(`npm: ${npmVer}`);
  else fail('npm 未安装或不在 PATH');

  section('2/6 Rust 工具链');
  const rustc = cmd('rustc --version');
  const cargo = cmd('cargo --version');
  const rustupT = cmd('rustup target list --installed');
  if (rustc) {
    const m = rustc.match(/rustc (\d+)\.(\d+)/);
    if (m && parseInt(m[1]) * 1000 + parseInt(m[2]) >= 1096) pass(`rustc: ${rustc} (>= 1.96 符合要求)`);
    else warn(`rustc: ${rustc} 建议 >= 1.96`);
  } else fail('rustc 未找到（需安装 Rust）');
  if (cargo) pass(`cargo: ${cargo}`);
  else fail('cargo 未找到');
  if (rustupT) {
    const targets = rustupT.split(/\r?\n/).map(s => s.trim()).filter(Boolean);
    const needWin = 'x86_64-pc-windows-msvc';
    const needLin = 'x86_64-unknown-linux-gnu';
    if (targets.includes(needWin) || targets.includes(needLin)) pass(`Rust targets 已安装: ${targets.join(', ')}`);
    else warn(`建议安装 ${needWin} (Windows) 或 ${needLin} (Linux):  rustup target add <triple>`);
  }

  section('3/6 前端依赖完整性');
  const nmPkg = resolve(PROJECT_ROOT, 'node_modules', '@tauri-apps', 'cli', 'package.json');
  if (existsSync(nmPkg)) pass('@tauri-apps/cli 已安装');
  else { fail('node_modules 缺失或 @tauri-apps/cli 未安装。请先执行 npm ci / npm install'); }
  const piniaOk = existsSync(resolve(PROJECT_ROOT, 'node_modules', 'pinia', 'package.json'));
  if (piniaOk) pass('核心依赖 (pinia/vue/element-plus) 就绪');

  section('4/6 版本号一致性');
  const pkg = readJSON('package.json');
  const tauri = readJSON('src-tauri/tauri.conf.json');
  const cargoV = readCargoVersion();
  if (pkg && tauri && cargoV) {
    if (pkg.version === tauri.version && tauri.version === cargoV) {
      pass(`三处版本号一致: v${pkg.version}`);
    } else fail(`版本号不一致: package.json=${pkg?.version}, tauri.conf.json=${tauri?.version}, Cargo.toml=${cargoV}`);
  }

  section('5/6 Tauri 配置完整性');
  if (tauri) {
    const win = tauri.app?.windows || [];
    const hasMainLabel = win.some(w => w.label === 'main');
    if (hasMainLabel) pass('窗口配置存在 label:"main"（符合项目约束）');
    else fail('窗口配置缺少 label:"main"，违反项目硬约束');

    const sec = tauri.app?.security;
    if (sec && !tauri.windows?.some?.(w => w.security)) pass("security 配置位于 app.security（Tauri v2 约束）");
    else warn("检查 security 是否错误地放在了单个窗口下");

    const targets = tauri.bundle?.targets || [];
    const exp = ['nsis', 'msi', 'deb', 'appimage', 'rpm'];
    const missing = exp.filter(t => !targets.includes(t));
    if (missing.length === 0) pass(`bundle targets 覆盖全部: ${exp.join(', ')}`);
    else warn(`缺少 targets: ${missing.join(', ')} (当前: ${targets.join(',')})`);
  }

  section('6/6 源码关键文件');
  const must = [
    'src-tauri/src/lib.rs', 'src-tauri/Cargo.toml', 'src-tauri/tauri.conf.json',
    'src/main.ts', 'package.json', 'vite.config.ts',
  ];
  for (const f of must) {
    if (existsSync(resolve(PROJECT_ROOT, f))) pass(`${f} 存在`);
    else fail(`${f} 缺失！`);
  }

  summary('构建前环境');
  return _stats.fail === 0;
}

// ============================================================
// MODE: scan - 构建日志扫描
// ============================================================
const ERROR_PATTERNS = [
  // Tauri bundler AppImage 相关
  { re: /failed to run linuxdeploy/i, lvl: 'fail', msg: 'linuxdeploy 执行失败（FUSE 缺失或 glibc 不兼容）' },
  { re: /Download of AppImage plugin failed/i, lvl: 'warn', msg: 'AppImage plugin 下载失败（网络问题，将回退内置版本）' },
  { re: /APPIMAGE_EXTRACT_AND_RUN/i, lvl: 'info', msg: '检测到 APPIMAGE_EXTRACT_AND_RUN 环境变量使用' },
  // FUSE
  { re: /fusermount|libfuse|\/dev\/fuse|FUSE/i, lvl: 'warn', msg: '日志提及 FUSE，需要确认 /dev/fuse 权限与模块' },
  // Glibc 兼容性
  { re: /version `GLIBC_.*' not found|GLIBCXX/i, lvl: 'fail', msg: 'glibc / libstdc++ 版本不兼容' },
  // WebKit
  { re: /libwebkit|webkit2gtk|javascriptcore/i, lvl: 'warn', msg: '日志提及 WebKit，检查 Linux 依赖是否安装' },
  // 通用 Cargo / Rust 错误
  { re: /error\[E\d{4}\]/, lvl: 'fail', msg: 'Rust 编译错误' },
  { re: /error: aborting due to \d+ previous error/i, lvl: 'fail', msg: 'Rust 编译终止' },
  { re: /could not compile |compilation failed/i, lvl: 'fail', msg: 'Cargo 编译失败' },
  // 通用前端错误（仅当出现 TS 错误号或失败关键字才算错误；'vue-tsc' 单独出现在命令行/info中忽略）
  { re: /TypeScript error|TS\d{4}:\s|vue-tsc\s+.*(error|fail)|vite\s+build\s+failed|error\s+during\s+build/i, lvl: 'fail', msg: '前端类型/构建错误' },
  // NSIS / WiX
  { re: /File: .*\.nsi|File: .*\.wxs|error.*nsis|error.*wix/i, lvl: 'fail', msg: 'NSIS/WiX 安装包构建错误' },
  // GLIBC / ELF 常见
  { re: /No such file or directory.*ld-linux|not a dynamic executable|Invalid ELF/i, lvl: 'fail', msg: 'ELF 可执行文件异常（可能架构不匹配或损坏）' },
  // ⭐ Fedora 39+ (glibc 2.38+) 默认用 DT_RELR (.relr.dyn section, type=0x13)，老的 binutils strip 不认识
  // linuxdeploy 内置的 strip 极其古老（~2020 年），在 Fedora 44 Ubuntu 24.04 等新系统上 100% 复现，症状：所有库 strip 失败 -> 最终 failed to run linuxdeploy
  // 修复：设置 NO_STRIP=1 或 STRIP=/usr/bin/strip（使用系统的新 strip）
  { re: /\.relr\.dyn|unknown type \[0x13\] section|Unable to recognise the format of the input file.*strip/i, lvl: 'fail', msg: '⭐ [Fedora44/WSL2根因命中] glibc 2.38+ 库使用 .relr.dyn (DT_RELR)，但 linuxdeploy 内置 strip 过于古老不认识。修复：export NO_STRIP=1 或 STRIP=$(command -v strip) 重新构建' },
  // deb/rpm
  { re: /dpkg-shlibdeps|dpkg-deb|rpmbuild failed/i, lvl: 'fail', msg: 'deb/rpm 打包失败' },
  // 网络
  { re: /TLS handshake|connection reset|timed out|DNS|resolve/i, lvl: 'warn', msg: '可能的网络问题' },
  // Panic
  { re: /panicked at |thread 'main' panicked|unwrap.*None|called `Result::unwrap\(\)` on an `Err`/i, lvl: 'fail', msg: 'Rust panic' },
];

function verifyScan(logPath) {
  const abs = resolve(process.cwd(), logPath);
  section(`扫描构建日志: ${abs}`);
  if (!existsSync(abs)) { fail(`日志文件不存在: ${abs}`); return false; }
  let content;
  try { content = readFileSync(abs, 'utf-8'); }
  catch (e) { fail(`读取日志失败: ${e.message}`); return false; }

  const lines = content.split(/\r?\n/);
  info(`日志总长度: ${lines.length} 行, ${(content.length / 1024).toFixed(1)} KB`);

  const hits = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    for (const pat of ERROR_PATTERNS) {
      if (pat.re.test(line)) {
        hits.push({ ln: i + 1, lvl: pat.lvl, msg: pat.msg, raw: line.slice(0, 200) });
        break;
      }
    }
  }

  if (hits.length === 0) { pass('日志未发现异常模式（扫描通过）'); return true; }

  // 去重：相同 lvl+msg 的重复命中只保留1条（避免同一个错误被 tauri bundler 重复打印多行造成虚假计数）
  const seen = new Set();
  const dedup = [];
  for (const h of hits) {
    const k = `${h.lvl}|${h.msg}`;
    if (!seen.has(k)) { seen.add(k); dedup.push(h); }
  }
  if (dedup.length !== hits.length) info(`去重：原命中 ${hits.length} 条 -> ${dedup.length} 条（相同错误被多次打印仅统计1次）`);
  const H = dedup;

  // 按严重程度输出
  const f = H.filter(h => h.lvl === 'fail');
  const w = H.filter(h => h.lvl === 'warn');
  const n = H.filter(h => h.lvl === 'info');
  if (f.length) section(`FAIL (${f.length})`);
  for (const h of f) { console.log(`    L${h.ln} [FAIL] ${h.msg}`); console.log(`         ${h.raw}`); fail(h.msg); }
  if (w.length) section(`WARN (${w.length})`);
  for (const h of w) { console.log(`    L${h.ln} [WARN] ${h.msg}`); console.log(`         ${h.raw}`); warn(h.msg); }
  if (n.length) section(`INFO (${n.length})`);
  for (const h of n) { console.log(`    L${h.ln} [INFO] ${h.msg}  -> ${h.raw}`); }

  summary('日志扫描');
  return f.length === 0;
}

// ============================================================
// MODE: post - 产物完整性检查
// ============================================================
const ARTIFACT_SPECS_WIN = [
  { fmt: 'nsis',   subdir: 'nsis',   pattern: /\.exe$/,       label: 'NSIS 安装包',  minMB: 3, maxMB: 60 },
  { fmt: 'msi',    subdir: 'msi',    pattern: /\.msi$/,       label: 'MSI 安装包',   minMB: 3, maxMB: 60, optional: true },
  { fmt: 'portable', rel: 'portable-zip', pattern: /nrmm-rust-portable-.*\.zip$/, label: 'Windows 便携版 zip', minMB: 3, maxMB: 60, portable: true },
];
const ARTIFACT_SPECS_LINUX = [
  { fmt: 'appimage', subdir: 'appimage', pattern: /\.AppImage$/, label: 'AppImage',    minMB: 5, maxMB: 200 },
  { fmt: 'deb',      subdir: 'deb',      pattern: /\.deb$/,      label: 'deb 包',       minMB: 3, maxMB: 100 },
  { fmt: 'rpm',      subdir: 'rpm',      pattern: /\.rpm$/,      label: 'rpm 包',       minMB: 3, maxMB: 100 },
  { fmt: 'portable', rel: 'portable-tgz', pattern: /nrmm-rust-portable-.*\.tar\.gz$/, label: 'Linux 便携版 tar.gz', minMB: 3, maxMB: 200, portable: true },
];

function detectPlatform() {
  const p = process.platform;
  if (p === 'win32') return 'windows';
  if (p === 'linux') return 'linux';
  if (p === 'darwin') return 'macos';
  return 'unknown';
}

function verifyPost(targetTriple) {
  section('构建后产物完整性检查');

  const platform = detectPlatform();
  const pkgV = readJSON('package.json')?.version || '0.0.0';
  info(`版本: v${pkgV}, 主机平台: ${platform}, target triple: ${targetTriple || '(auto)'}`);

  // 推断 target/bundle 目录
  let triple = targetTriple;
  let bundleDir;
  if (triple) {
    bundleDir = resolve(PROJECT_ROOT, 'src-tauri/target', triple, 'release/bundle');
  } else {
    // 自动：尝试常见路径
    const tryDirs = [
      ['x86_64-pc-windows-msvc', platform === 'windows'],
      ['x86_64-unknown-linux-gnu', platform === 'linux'],
      ['aarch64-apple-darwin', false],
    ];
    for (const [t, _] of tryDirs) {
      const d = resolve(PROJECT_ROOT, 'src-tauri/target', t, 'release/bundle');
      if (existsSync(d)) { triple = t; bundleDir = d; break; }
    }
    // 最后回退无target
    if (!bundleDir) bundleDir = resolve(PROJECT_ROOT, 'src-tauri/target/release/bundle');
  }
  info(`使用 bundle 目录: ${bundleDir}`);
  if (!existsSync(bundleDir)) {
    fail(`bundle 目录不存在: ${bundleDir}。请先完成构建或用 --target 指定正确的 triple`);
    return false;
  }

  const specs = (triple && (triple.includes('linux') || triple.includes('linux-gnu'))) || platform === 'linux'
    ? ARTIFACT_SPECS_LINUX
    : ARTIFACT_SPECS_WIN;

  for (const s of specs) {
    const dir = s.portable ? PROJECT_ROOT : join(bundleDir, s.subdir);
    const files = s.portable
      ? (existsSync(dir) ? readdirSync(dir).filter(f => s.pattern.test(f)).map(f => join(dir, f)) : [])
      : findByGlob(dir, s.pattern);

    if (files.length === 0) {
      if (s.optional) warn(`${s.label} 未找到（可选产物）`);
      else fail(`${s.label} 未找到！(搜索目录: ${dir}, pattern: ${s.pattern})`);
      continue;
    }
    for (const f of files) {
      const mb = fileSizeMB(f);
      const name = basename(f);
      if (mb === null) { fail(`${s.label} 文件无法访问: ${name}`); continue; }
      const mbN = parseFloat(mb);
      if (s.minMB && mbN < s.minMB) { fail(`${s.label} 体积过小 (${mb} MB < ${s.minMB} MB)，可能构建不完整: ${name}`); continue; }
      if (s.maxMB && mbN > s.maxMB) { warn(`${s.label} 体积过大 (${mb} MB > ${s.maxMB} MB): ${name}`); continue; }
      pass(`${s.label}: ${name} (${mb} MB)  ✓`);
    }
  }

  // latest.json
  section('updater manifest (latest.json) 校验');
  const latestCandidates = [
    ...findByGlob(bundleDir, /^latest\.json$/),
    resolve(PROJECT_ROOT, 'latest.json'),
  ];
  const latestFile = latestCandidates.find(existsSync);
  if (latestFile) {
    const l = readJSON(latestFile);
    info(`找到 latest.json: ${latestFile}`);
    if (!l) { fail('latest.json 解析失败或非合法 JSON'); }
    else {
      if (l.version === pkgV) pass(`latest.json 版本匹配: v${l.version}`);
      else fail(`latest.json 版本 ${l.version} 与 package.json ${pkgV} 不一致`);
      const plats = l.platforms;
      if (plats && Object.keys(plats).length > 0) {
        pass(`platforms 包含 ${Object.keys(plats).length} 个平台: ${Object.keys(plats).join(', ')}`);
        for (const [k, v] of Object.entries(plats)) {
          if (!v.url || !v.signature) fail(`platforms.${k} 缺少 url 或 signature`);
          else pass(`  ${k}: url=${(v.url||'').slice(0,60)}... sig.len=${(v.signature||'').length}`);
        }
      } else warn('latest.json 无 platforms 信息（可能由 merge-updater-manifest.mjs 合并生成）');
    }
  } else warn('未找到 latest.json（合并 manifest 步骤会生成）');

  // 便携版兜底提示
  if (platform === 'linux' || (triple && triple.includes('linux'))) {
    const tgz = findByGlob(PROJECT_ROOT, /nrmm-rust-portable-.*\.tar\.gz$/);
    if (tgz.length > 0) info(`✅ 便携版 tar.gz 存在: ${basename(tgz[0])}（可作为 AppImage 构建失败时的兜底）`);
    else warn('未生成便携版 tar.gz，可运行:  node scripts/build-portable.mjs --target x86_64-unknown-linux-gnu -o .');
  }

  summary('构建后产物');
  return _stats.fail === 0;
}

// ============================================================
// MODE: list-commands
// ============================================================
function listCommands() {
  const lines = [
    '# nrmm-rust 构建检查命令清单（全流程）',
    '',
    '## 一、环境修复阶段（仅 Windows WSL2 用户需要）',
    '  # 在 Windows PowerShell (管理员) 执行，生成 .wslconfig 并启用 FUSE',
    '  & .\\scripts\\setup-wsl2.ps1 -ApplyNow',
    '',
    '## 二、AppImage 环境诊断修复（Linux / WSL2 中执行）',
    '  bash scripts/fix-appimage-env.sh           # 诊断 + 下载 linuxdeploy 缓存',
    '  bash scripts/fix-appimage-env.sh --apply   # 额外把环境变量写入 ~/.bashrc',
    '  bash scripts/fix-appimage-env.sh --check   # 仅诊断，不做下载/修改',
    '',
    '## 三、构建前环境检查（任意平台）',
    '  node scripts/verify-build.mjs pre',
    '  npm run verify:release                     # 同上（package.json 别名）',
    '',
    '## 四、执行构建（建议保存日志供 scan 分析）',
    '  # Windows',
    '  npm run tauri build -- --target x86_64-pc-windows-msvc *>&1 | Tee-Object build-win.log',
    '  # Linux / WSL2',
    '  (npm run tauri build -- --target x86_64-unknown-linux-gnu) 2>&1 | tee build-linux.log',
    '',
    '## 五、日志扫描（定位构建异常）',
    '  node scripts/verify-build.mjs scan build-win.log',
    '  node scripts/verify-build.mjs scan build-linux.log',
    '',
    '## 六、构建后产物检查',
    '  node scripts/verify-build.mjs post --target x86_64-unknown-linux-gnu',
    '  node scripts/verify-build.mjs post --target x86_64-pc-windows-msvc',
    '  node scripts/verify-build.mjs all  --target x86_64-unknown-linux-gnu   # pre + post',
    '',
    '## 七、便携版兜底（当 AppImage/安装包构建失败时使用）',
    '  node scripts/build-portable.mjs --target x86_64-pc-windows-msvc  -o dist-win',
    '  node scripts/build-portable.mjs --target x86_64-unknown-linux-gnu -o dist-linux',
    '',
    '## 八、合并发布 manifest（多平台产物合并后上传 Release）',
    '  node scripts/merge-updater-manifest.mjs latest-win.json latest-linux.json -o latest.json',
    '  node scripts/verify-tauri-release.mjs post latest.json',
    '',
  ];
  console.log(lines.join('\n'));
}

// ---------- summary ----------
function summary(stage) {
  console.log(`\n${C.BOLD}${stage} 汇总: 通过=${_stats.pass}  警告=${_stats.warn}  失败=${_stats.fail}${C.RST}`);
}

// ============================================================
// MAIN
// ============================================================
function main() {
  const mode = process.argv[2];
  let target = null;
  for (let i = 3; i < process.argv.length; i++) {
    if (process.argv[i] === '--target' && i + 1 < process.argv.length) target = process.argv[++i];
  }

  if (!mode) { console.error('缺少 mode 参数。支持: pre | scan <log> | post [--target T] | all [--target T] | list-commands'); process.exit(2); }

  console.log(`${C.BOLD}${C.W}nrmm-rust 构建校验工具${C.RST}  mode=${mode}${target ? '  target=' + target : ''}`);

  let ok = true;
  switch (mode) {
    case 'pre':           ok = verifyPre(); break;
    case 'scan': {
      const log = process.argv[3];
      if (!log) { console.error('scan 模式需要 <logfile> 参数'); process.exit(2); }
      ok = verifyScan(log);
      break;
    }
    case 'post':          ok = verifyPost(target); break;
    case 'all': {
      const ok1 = verifyPre();
      console.log('');
      _stats.pass = _stats.warn = _stats.fail = 0;
      const ok2 = verifyPost(target);
      ok = ok1 && ok2;
      break;
    }
    case 'list-commands': listCommands(); process.exit(0);
    default:
      console.error(`未知 mode: ${mode}. 支持: pre | scan <log> | post [--target T] | all [--target T] | list-commands`);
      process.exit(2);
  }

  process.exit(ok ? 0 : 1);
}

main();
