#!/usr/bin/env node
/**
 * 构建后回填站点页脚的版本号，让官网版本与 Release 一致。
 *
 * 版本来源优先级：
 * 1. 环境变量 SITE_VERSION（发布工作流调用本站构建时传入本次 tag）
 * 2. 仓库最新 Release 的 tag（日常推 main 触发构建时）
 * 3. 两者都取不到时保留 index.html 里的原值，不因网络问题让构建失败
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const target = join(here, '..', 'dist', 'index.html');
const repo = 'Titor-Z/lspd';

/** 去掉 tag 的 v 前缀。 */
const normalize = (tag) => tag?.trim().replace(/^v/, '') || undefined;

/** 读最新 Release 的 tag；任何失败都返回 undefined。 */
async function latestReleaseTag() {
  try {
    const res = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
      headers: { accept: 'application/vnd.github+json' },
    });
    return res.ok ? normalize((await res.json()).tag_name) : undefined;
  } catch {
    return undefined;
  }
}

const version = normalize(process.env.SITE_VERSION) || (await latestReleaseTag());

if (!version) {
  console.log('site version: 未取到发布版本，保留原文');
} else {
  const html = readFileSync(target, 'utf8');
  writeFileSync(
    target,
    html.replace(/<span data-version>[^<]*<\/span>/, `<span data-version>${version}</span>`),
  );
  console.log(`site version: ${version}`);
}
