#!/usr/bin/env node
/**
 * 构建后回填站点自身体积。
 *
 * 读取 dist 里实际产出的 CSS 与 JS，计算 gzip 后大小，写进 index.html 的页脚。
 * 这样页面上标注的体积是构建产物算出来的，不是手写的数字。
 */

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, '..', 'dist');
const assets = join(dist, 'assets');

/** gzip 后的大小，单位 KB，保留一位小数。 */
const gzipKb = (path) => (gzipSync(readFileSync(path)).length / 1024).toFixed(1);

let js = 0;
let css = 0;

for (const name of readdirSync(assets)) {
  const path = join(assets, name);
  if (!statSync(path).isFile()) continue;
  if (name.endsWith('.js')) js += Number(gzipKb(path));
  if (name.endsWith('.css')) css += Number(gzipKb(path));
}

const label = `本站 JS ${js.toFixed(1)} KB · CSS ${css.toFixed(1)} KB · 无框架 · 无网络字体`;

const htmlPath = join(dist, 'index.html');
const html = readFileSync(htmlPath, 'utf8');
const updated = html.replace(
  /(<span[^>]*\bdata-size\b[^>]*>)[^<]*(<\/span>)/,
  `$1${label}$2`,
);

writeFileSync(htmlPath, updated);
console.log(`size report: ${label}`);
