#!/usr/bin/env python3
"""生成纯 JS 测试工程：无 tsconfig，同目录，文件之间互相 import。

用途是复现「打开一个涨一个」的场景。刻意不写 tsconfig.json 与 jsconfig.json，
让语言服务走 inferred project —— root files 只包含当前打开的文件，
依赖通过 import 链按需加载。

用法：
    scripts/make_js_fixture.py --out /tmp/jsproj --files 60
"""

import argparse
import os


TEMPLATE = """// module {index}
import {{ util{left} }} from './module-{left}.js';
import {{ util{right} }} from './module-{right}.js';

export function util{index}(value) {{
  return value + {index};
}}

export function entry{index}(input) {{
  const a = util{left}(input);
  const b = util{right}(a);
  return {{ total: b, local: util{index}(1) }};
}}

export const config{index} = {{
  name: 'module-{index}',
  index: {index},
  nested: {{ enabled: true, weight: {index} * 2 }},
}};
"""


def main():
    parser = argparse.ArgumentParser(description="生成纯 JS 测试工程")
    parser.add_argument("--out", required=True, help="输出目录")
    parser.add_argument("--files", type=int, default=60, help="生成的 .js 文件数")
    args = parser.parse_args()

    os.makedirs(args.out, exist_ok=True)

    for index in range(args.files):
        # 环形引用：每个文件依赖前后各一个，形成真实依赖网而不是孤岛
        left = (index - 1) % args.files
        right = (index + 1) % args.files
        path = os.path.join(args.out, "module-%d.js" % index)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(TEMPLATE.format(index=index, left=left, right=right))

    # 故意不放 tsconfig.json / jsconfig.json
    print("已生成 %d 个 .js 到 %s（无 tsconfig）" % (args.files, args.out))


if __name__ == "__main__":
    main()
