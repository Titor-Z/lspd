#!/usr/bin/env python3
"""解析 TSRS_REGION_LOG 的输出，统计 program 的创建与释放。

用法：
    TSRS_REGION_LOG=1 lspd ts 2>region.log
    scripts/region_stats.py region.log

日志每行形如：
    regions: program 0x7f8a: created (region 512 KiB, base 2048 KiB)
    regions: program 0x7f8a: freed (12 files, region 512 KiB)

输出四个数：创建次数、释放次数、两者之差、同时在世的 program 数峰值，
以及 region KiB 的分布——用来区分「program 在堆积」与「单个 program 在变大」。
"""

import argparse
import re
import statistics
import sys

# created 行带上 region 与 base 两个体积，freed 行只带 region
CREATED = re.compile(r"program (0x[0-9a-f]+): created \(region (\d+) KiB, base (\d+) KiB\)")
FREED = re.compile(r"program (0x[0-9a-f]+): freed \((\d+) files, region (\d+) KiB\)")


def parse(path):
    """逐行扫描日志，按出现顺序还原 program 的生死。"""
    created = []  # [(addr, region_kb, base_kb)]
    freed = []  # [(addr, files, region_kb)]
    live = set()
    peak_live = 0
    unknown_free = 0

    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            hit = CREATED.search(line)
            if hit:
                addr = hit.group(1)
                created.append((addr, int(hit.group(2)), int(hit.group(3))))
                live.add(addr)
                peak_live = max(peak_live, len(live))
                continue

            hit = FREED.search(line)
            if hit:
                addr = hit.group(1)
                freed.append((addr, int(hit.group(2)), int(hit.group(3))))
                if addr in live:
                    live.discard(addr)
                else:
                    unknown_free += 1

    return created, freed, peak_live, unknown_free


def spread(values):
    """给出 min / 中位 / max，空集返回 None。"""
    if not values:
        return None
    return (min(values), int(statistics.median(values)), max(values))


def main():
    parser = argparse.ArgumentParser(description="统计 region 日志里的 program 生死")
    parser.add_argument("log", help="TSRS_REGION_LOG 落盘的日志文件")
    parser.add_argument("--top", type=int, default=5, help="打印最大的几个 program")
    args = parser.parse_args()

    created, freed, peak_live, unknown_free = parse(args.log)

    print(f"文件        {args.log}")
    print(f"created     {len(created)}")
    print(f"freed       {len(freed)}")
    print(f"未释放      {len(created) - len(freed)}")
    print(f"同时存活峰值 {peak_live}")
    if unknown_free:
        print(f"孤立 freed  {unknown_free}  （说明日志被截断或跨进程拼接）")

    region_spread = spread([c[1] for c in created])
    base_spread = spread([c[2] for c in created])

    if region_spread:
        low, mid, high = region_spread
        print(f"region KiB  min {low} / 中位 {mid} / max {high}")
    if base_spread:
        low, mid, high = base_spread
        print(f"base   KiB  min {low} / 中位 {mid} / max {high}")

    if created:
        print("\n最大的几个 program（按 region 排序）：")
        for addr, region_kb, base_kb in sorted(created, key=lambda item: -item[1])[: args.top]:
            print(f"  {addr}  region {region_kb} KiB  base {base_kb} KiB")

    return 0


if __name__ == "__main__":
    sys.exit(main())
