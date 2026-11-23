#!/usr/bin/env node
// npm launcher for the orchnes CLI (a Rust binary). No dependencies, no install script.
// On first run it downloads the prebuilt binary for this platform from the matching
// GitHub Release, verifies its SHA-256 against checksums.json shipped in this package,
// caches it, and then runs it with the given arguments.
'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const https = require('node:https');
const crypto = require('node:crypto');
const {spawnSync} = require('node:child_process');

const pkg = require('../package.json');
const REPO = 'orchnesai/orchnes-cli';

const TARGETS = {
  'linux-x64': 'x86_64-unknown-linux-musl',
  'linux-arm64': 'aarch64-unknown-linux-musl',
  'darwin-x64': 'x86_64-apple-darwin',
  'darwin-arm64': 'aarch64-apple-darwin',
  'win32-x64': 'x86_64-pc-windows-msvc'
};

function targetFor(platform, arch) {
  return TARGETS[`${platform}-${arch}`] || null;
}

function assetName(target) {
  return `orchnes-${target}${target.includes('windows') ? '.exe' : ''}`;
}

function cacheDir(env, platform, home) {
  if (env.ORCHNES_CACHE_DIR) return env.ORCHNES_CACHE_DIR;
  if (platform === 'win32') return path.join(env.LOCALAPPDATA || path.join(home, 'AppData', 'Local'), 'orchnes', 'bin');
  if (platform === 'darwin') return path.join(home, 'Library', 'Caches', 'orchnes', 'bin');
  return path.join(env.XDG_CACHE_HOME || path.join(home, '.cache'), 'orchnes', 'bin');
}

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function download(url, dest, redirects = 5) {
  return new Promise((resolve, reject) => {
    const req = https.get(url, {headers: {'User-Agent': `orchnes-npm/${pkg.version}`}}, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode) && res.headers.location && redirects > 0) {
        res.resume();
        resolve(download(new URL(res.headers.location, url).href, dest, redirects - 1));
        return;
      }
      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(`download failed (${res.statusCode}) for ${url}`));
        return;
      }
      const out = fs.createWriteStream(dest, {mode: 0o755});
      res.pipe(out);
      out.on('finish', () => out.close(resolve));
      out.on('error', reject);
    });
    req.on('error', reject);
    req.setTimeout(60000, () => req.destroy(new Error('download timed out')));
  });
}

async function ensureBinary() {
  const target = targetFor(process.platform, process.arch);
  if (!target) {
    throw new Error(`no prebuilt orchnes binary for ${process.platform}-${process.arch}; install with: cargo install orchnes`);
  }
  const name = assetName(target);
  const checksums = require('../checksums.json');
  const expected = checksums[name];
  if (!expected) throw new Error(`checksums.json has no entry for ${name}`);
  const dir = path.join(cacheDir(process.env, process.platform, os.homedir()), pkg.version);
  const bin = path.join(dir, name);
  if (fs.existsSync(bin) && sha256(bin) === expected) return bin;
  fs.mkdirSync(dir, {recursive: true});
  const tmp = `${bin}.${process.pid}.download`;
  const url = `https://github.com/${REPO}/releases/download/v${pkg.version}/${name}`;
  try {
    await download(url, tmp);
    const actual = sha256(tmp);
    if (actual !== expected) throw new Error(`checksum mismatch for ${name}: expected ${expected}, got ${actual}`);
    fs.chmodSync(tmp, 0o755);
    fs.renameSync(tmp, bin);
  } finally {
    if (fs.existsSync(tmp)) fs.rmSync(tmp, {force: true});
  }
  return bin;
}

async function main() {
  const bin = await ensureBinary();
  const result = spawnSync(bin, process.argv.slice(2), {stdio: 'inherit'});
  if (result.error) throw result.error;
  process.exit(result.status === null ? 1 : result.status);
}

if (require.main === module) {
  main().catch((err) => {
    process.stderr.write(`orchnes: ${err.message}\n`);
    process.exit(1);
  });
}

module.exports = {targetFor, assetName, cacheDir, TARGETS};
