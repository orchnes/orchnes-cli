import test from 'node:test';
import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {readFileSync} from 'node:fs';

const require = createRequire(import.meta.url);
const {targetFor, assetName, cacheDir, TARGETS} = require('../bin/orchnes.js');
const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const workflow = readFileSync(new URL('../../.github/workflows/publish.yml', import.meta.url), 'utf8');
const launcher = readFileSync(new URL('../bin/orchnes.js', import.meta.url), 'utf8');

test('every supported platform maps to a target that the release workflow builds', () => {
  for (const target of Object.values(TARGETS)) assert.ok(workflow.includes(`target: ${target}`), target);
  assert.equal(targetFor('linux', 'x64'), 'x86_64-unknown-linux-musl');
  assert.equal(targetFor('darwin', 'arm64'), 'aarch64-apple-darwin');
  assert.equal(targetFor('win32', 'x64'), 'x86_64-pc-windows-msvc');
  assert.equal(targetFor('freebsd', 'x64'), null);
});

test('asset names match what the workflow uploads', () => {
  assert.equal(assetName('x86_64-pc-windows-msvc'), 'orchnes-x86_64-pc-windows-msvc.exe');
  assert.equal(assetName('aarch64-apple-darwin'), 'orchnes-aarch64-apple-darwin');
  assert.ok(workflow.includes('orchnes-${{ matrix.target }}${{ matrix.ext }}'));
});

test('cache location respects overrides and platform conventions', () => {
  assert.equal(cacheDir({ORCHNES_CACHE_DIR: '/tmp/x'}, 'linux', '/home/a'), '/tmp/x');
  assert.match(cacheDir({}, 'linux', '/home/a'), /\.cache[\\/]orchnes[\\/]bin$/);
  assert.match(cacheDir({XDG_CACHE_HOME: '/xdg'}, 'linux', '/home/a'), /^[\\/]xdg[\\/]orchnes[\\/]bin$/);
  assert.match(cacheDir({}, 'darwin', '/Users/a'), /Library[\\/]Caches[\\/]orchnes[\\/]bin$/);
});

test('package is a dependency-free launcher without install scripts, verified by checksum', () => {
  assert.equal(pkg.name, 'orchnes');
  assert.deepEqual(pkg.bin, {orchnes: 'bin/orchnes.js'});
  assert.equal(pkg.dependencies, undefined);
  assert.equal(pkg.scripts, undefined);
  assert.deepEqual(pkg.files.sort(), ['README.md', 'bin/orchnes.js', 'checksums.json']);
  assert.match(launcher, /checksum mismatch/);
  assert.match(launcher, /releases\/download\/v\$\{pkg\.version\}/);
});
