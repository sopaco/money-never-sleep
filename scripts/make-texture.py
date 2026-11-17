#!/usr/bin/env python3
"""生成管家窗口的背景纹理（assets/butler-texture.png）。

为什么是「生成脚本 + PNG」而不是运行时画：GPUI 没有平铺/噪点填充，纹理只能
来自图片；而把噪声写成代码每次启动重算不值得。脚本保证可复现——想换纹理改
参数重跑即可，不用手工做图。

设计约束：
- **透明底**，只在 alpha 上做文章。纹理叠在渐变背景上，若用近白实底，半透明
  叠加会把渐变冲淡成一片灰白；透明 PNG 只压暗/提亮局部，底色保持不变。
- 低频为主：远看是「纸的云斑」，近看才有一点纤维。高频颗粒一多，PNG 体积
  涨十倍，暗色区域还会出现脏点。
- 单色（暖灰）低对比：管家窗口是文字界面，纹理的职责是让白卡片旁边那层底
  看起来有材质，不是抢注意力。

用法：
    python3 scripts/make-texture.py            # 写入 assets/butler-texture.png
    python3 scripts/make-texture.py --check     # 只打印尺寸与体积
"""

from __future__ import annotations

import argparse
import math
import random
import struct
import zlib
from pathlib import Path

WIDTH = 768
HEIGHT = 640
SEED = 20261117

# 云斑的强度（alpha，0-255）。半透明叠加后约等于 1-2% 的明度变化。
MOTTLE_ALPHA = 30
# 纤维纹理的最大强度，比云斑更弱一档。
FIBER_ALPHA = 12
# 左上暖光、右下冷影的渐变强度。
LIGHT_ALPHA = 20
SHADE_ALPHA = 16

# 单色暖灰：纹理不承担配色，色相只为了让暖光/冷影看起来是「光」而不是灰。
WARM = (134, 120, 96)
DAWN = (255, 248, 236)
DUSK = (228, 236, 252)


def light_falloff(u: float, v: float) -> float:
    """左上角的晨光，按椭圆距离平方衰减。"""
    d = math.hypot(u / 1.05, v / 1.25)
    return max(0.0, 1.0 - d) ** 2


def shade_falloff(u: float, v: float) -> float:
    """右下角的雾影，与晨光对称。"""
    d = math.hypot((1.0 - u) / 1.05, (1.0 - v) / 1.25)
    return max(0.0, 1.0 - d) ** 2


def value_noise(width: int, height: int, cells: int, rng: random.Random) -> list[float]:
    """A tiny value-noise lattice, bilinearly interpolated and smoothstepped."""
    grid_w = max(2, cells)
    grid_h = max(2, int(cells * height / max(1, width)))
    lattice = [[rng.random() for _ in range(grid_w + 1)] for _ in range(grid_h + 1)]
    out = [0.0] * (width * height)
    for y in range(height):
        fy = y / max(1, height - 1) * grid_h
        y0 = min(int(fy), grid_h - 1)
        ty = fy - y0
        sy = ty * ty * (3 - 2 * ty)
        row0 = lattice[y0]
        row1 = lattice[y0 + 1]
        for x in range(width):
            fx = x / max(1, width - 1) * grid_w
            x0 = min(int(fx), grid_w - 1)
            tx = fx - x0
            sx = tx * tx * (3 - 2 * tx)
            top = row0[x0] + (row0[x0 + 1] - row0[x0]) * sx
            bottom = row1[x0] + (row1[x0 + 1] - row1[x0]) * sx
            out[y * width + x] = top + (bottom - top) * sy
    return out


def fbm(width: int, height: int, octaves: list[int], rng: random.Random) -> list[float]:
    """Weighted sum of value-noise octaves, normalised to 0..1."""
    total = [0.0] * (width * height)
    weight_sum = 0.0
    for index, cells in enumerate(octaves):
        weight = 1.0 / (index + 1)
        layer = value_noise(width, height, cells, rng)
        for i, value in enumerate(layer):
            total[i] += value * weight
        weight_sum += weight
    return [value / weight_sum for value in total]


def build_rows() -> list[bytes]:
    rng = random.Random(SEED)
    # 三层低频：大云斑 + 中斑 + 细一点的纤维带。
    mottle = fbm(WIDTH, HEIGHT, [3, 7, 15], rng)
    fiber = fbm(WIDTH, HEIGHT, [40, 90], rng)

    rows: list[bytes] = []
    for y in range(HEIGHT):
        row = bytearray()
        v = y / max(1, HEIGHT - 1)
        for x in range(WIDTH):
            u = x / max(1, WIDTH - 1)
            index = y * WIDTH + x

            blotch = mottle[index]
            grain = fiber[index]

            # 四层按 alpha 加权混色。分别「先算 alpha 再二选一配色」会在
            # 层与层交接处留下一条硬边（第一版就是这样），所以这里始终做
            # 加权平均：权重降到 0 的那一层颜色自然退出，没有分界线。
            weights = [
                (MOTTLE_ALPHA * (0.30 + 0.70 * blotch), WARM),
                (FIBER_ALPHA * abs(grain - 0.5) * 2, WARM),
                (LIGHT_ALPHA * light_falloff(u, v), DAWN),
                (SHADE_ALPHA * shade_falloff(u, v), DUSK),
            ]
            total = sum(weight for weight, _ in weights)
            if total <= 0.0:
                row += bytes((0, 0, 0, 0))
                continue
            r = int(round(sum(weight * color[0] for weight, color in weights) / total))
            g = int(round(sum(weight * color[1] for weight, color in weights) / total))
            b = int(round(sum(weight * color[2] for weight, color in weights) / total))
            alpha = int(round(min(255.0, total)))
            row += bytes((r, g, b, alpha))
        rows.append(bytes(row))
    return rows


def write_png(path: Path, rows: list[bytes]) -> None:
    def chunk(tag: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + tag
            + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
        )

    # 每行挑一个 PNG 过滤器（None/Sub/Up）。纹理是大片平滑渐变，逐行取
    # 「差值绝对值之和最小」的那个，能把体积压到不做过滤的四分之一左右。
    stride = WIDTH * 4
    filtered: list[bytes] = []
    previous = bytes(stride)
    for row in rows:
        candidates = [bytes([0]) + row]

        sub = bytes((row[i] - (row[i - 4] if i >= 4 else 0)) & 0xFF for i in range(stride))
        candidates.append(bytes([1]) + sub)

        up = bytes((row[i] - previous[i]) & 0xFF for i in range(stride))
        candidates.append(bytes([2]) + up)

        def cost(candidate: bytes) -> int:
            return sum(min(b, 256 - b) for b in candidate[1:])

        filtered.append(min(candidates, key=cost))
        previous = row

    raw = b"".join(filtered)
    header = struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="只打印产物信息")
    parser.add_argument(
        "--out",
        default=str(Path(__file__).resolve().parent.parent / "assets" / "butler-texture.png"),
    )
    args = parser.parse_args()

    out = Path(args.out)
    if args.check:
        if not out.exists():
            raise SystemExit(f"{out} 不存在")
        print(f"{out} {WIDTH}x{HEIGHT} {out.stat().st_size / 1024:.1f} KiB")
        return

    write_png(out, build_rows())
    print(f"wrote {out} {WIDTH}x{HEIGHT} {out.stat().st_size / 1024:.1f} KiB")


if __name__ == "__main__":
    main()
