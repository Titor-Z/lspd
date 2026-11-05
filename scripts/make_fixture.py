#!/usr/bin/env python3
"""生成基准用的确定性测试工程。

产物只依赖参数，不依赖时间与随机种子，保证不同轮次的基准可以直接对比：
- HTML 文件：多层嵌套、属性、内联 style/script、id/class
- CSS 文件：规则、自定义属性、媒体查询
- TS 文件：跨文件 import、接口、类、函数
"""

import argparse
import os

# HTML 片段模板：交替使用语义标签，保证有足够的嵌套与属性供补全和诊断使用
HTML_BLOCK = """\
  <section id="block-{index}" class="card card--{index}" data-role="block">
    <header class="card__header">
      <h2 class="card__title">区块 {index}</h2>
      <p class="card__desc">用于基准测试的静态片段，不含外部依赖。</p>
    </header>
    <div class="card__body">
      <ul class="list">
        <li class="list__item"><a href="#block-{index}" title="跳转">条目一</a></li>
        <li class="list__item"><a href="#block-{index}" title="跳转">条目二</a></li>
      </ul>
      <button type="button" class="btn btn--primary" aria-label="提交">提交</button>
    </div>
  </section>
"""

# CSS 规则模板：包含自定义属性、组合选择器与媒体查询
CSS_RULE = """\
.page-{index} .card__title {{
  color: var(--color-text-{index}, #1f2328);
  font-size: 1.125rem;
  line-height: 1.5;
}}

.page-{index} .card__body > .list__item:first-child {{
  margin-block-end: 0.5rem;
  padding-inline: calc(0.5rem + {index}px);
}}

@media (min-width: {width}px) {{
  .page-{index} .card {{
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  }}
}}
"""

# TS 文件模板：跨文件依赖，制造真实的项目图而非孤立文件
TS_MODULE = """\
export interface Record{index} {{
  id: number;
  name: string;
  tags: string[];
}}

export class Store{index} {{
  private readonly items: Map<number, Record{index}> = new Map();

  add(item: Record{index}): void {{
    this.items.set(item.id, item);
  }}

  find(id: number): Record{index} | undefined {{
    return this.items.get(id);
  }}
}}

export function build{index}(name: string): Record{index} {{
  return {{ id: {index}, name, tags: [] }};
}}
"""


def write_html(directory, count, size):
    """按目标体积重复写入区块，生成指定数量的 HTML 文件。"""
    for index in range(count):
        blocks = []
        length = 0
        repeated = 0
        while length < size:
            block = HTML_BLOCK.format(index=f"{index}-{repeated}")
            blocks.append(block)
            length += len(block)
            repeated += 1
        body = "".join(blocks)
        document = (
            "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n"
            "  <meta charset=\"utf-8\">\n"
            f"  <title>页面 {index}</title>\n"
            "  <link rel=\"stylesheet\" href=\"style-%d.css\">\n" % index
            + "</head>\n<body class=\"page-%d\">\n" % index
            + body
            + "</body>\n</html>\n"
        )
        with open(os.path.join(directory, "page-%d.html" % index), "w", encoding="utf-8") as handle:
            handle.write(document)


def write_css(directory, count, size):
    """按目标体积重复写入规则，生成指定数量的 CSS 文件。"""
    for index in range(count):
        rules = []
        length = 0
        repeated = 0
        while length < size:
            rule = CSS_RULE.format(index=f"{index}-{repeated}", width=640 + repeated * 40)
            rules.append(rule)
            length += len(rule)
            repeated += 1
        with open(os.path.join(directory, "style-%d.css" % index), "w", encoding="utf-8") as handle:
            handle.write(":root {\n  --color-text: #1f2328;\n}\n\n" + "".join(rules))


def write_ts(directory, count):
    """生成带跨文件引用的 TS 模块，用于测量语言服务器的项目加载成本。"""
    for index in range(count):
        header = "import { build0 } from \"./module-0\";\n\n" if index else ""
        footer = "\nexport const entry%d = build0(\"entry\");\n" % index if index else ""
        with open(os.path.join(directory, "module-%d.ts" % index), "w", encoding="utf-8") as handle:
            handle.write(header + TS_MODULE.format(index=index) + footer)


def main():
    parser = argparse.ArgumentParser(description="生成确定性的基准测试工程")
    parser.add_argument("--out", required=True, help="输出目录")
    parser.add_argument("--html-files", type=int, default=16)
    parser.add_argument("--html-size", type=int, default=65536)
    parser.add_argument("--css-files", type=int, default=16)
    parser.add_argument("--css-size", type=int, default=12800)
    parser.add_argument("--ts-files", type=int, default=50)
    args = parser.parse_args()

    html_dir = os.path.join(args.out, "html")
    css_dir = os.path.join(args.out, "css")
    ts_dir = os.path.join(args.out, "ts")
    for path in (html_dir, css_dir, ts_dir):
        os.makedirs(path, exist_ok=True)

    write_html(html_dir, args.html_files, args.html_size)
    write_css(css_dir, args.css_files, args.css_size)
    write_ts(ts_dir, args.ts_files)

    print("已生成：%s（html %d 个 / css %d 个 / ts %d 个）"
          % (args.out, args.html_files, args.css_files, args.ts_files))


if __name__ == "__main__":
    main()
