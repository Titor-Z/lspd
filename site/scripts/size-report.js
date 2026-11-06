#!/usr/bin/env node

import { readFileSync, existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const distDir = join(__dirname, '..', 'dist');
const indexHtmlPath = join(distDir, 'index.html');

if (!existsSync(indexHtmlPath)) {
  console.log('dist/index.html not found, skipping size report.');
  process.exit(0);
}

console.log('Build complete. Site generated successfully.');
