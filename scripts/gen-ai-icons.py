"""Gera os icones usados pela barra de topo do NeuralIA.

Marcas geometricas simplificadas nas cores de cada servico -- desenhadas a 8x e
reduzidas com Lanczos, porque o GDI nao tem anti-aliasing: o unico suavizado que
a barra de topo tem vem destes PNGs. Nada e descarregado e nenhuma arte de marca
registada e embutida.

    python scripts/gen-ai-icons.py
"""

from __future__ import annotations

import math
import os

from PIL import Image, ImageDraw

SIZE = 256
SS = 8  # supersampling
N = SIZE * SS
CENTER = N / 2.0

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "assets", "ai")


def finish(mask: Image.Image, paint: Image.Image, name: str) -> None:
    """Aplica `mask` como alfa de `paint`, reduz para 256 e grava."""
    out = paint.copy()
    out.putalpha(mask)
    out = out.resize((SIZE, SIZE), Image.LANCZOS)
    path = os.path.join(OUT_DIR, name)
    out.save(path)
    print(f"{path}  {out.size[0]}x{out.size[1]}")


def solid(color: tuple[int, int, int]) -> Image.Image:
    return Image.new("RGB", (N, N), color)


def quad_bezier(p0, c, p1, steps=160):
    points = []
    for i in range(steps + 1):
        t = i / steps
        u = 1.0 - t
        points.append(
            (
                u * u * p0[0] + 2 * u * t * c[0] + t * t * p1[0],
                u * u * p0[1] + 2 * u * t * c[1] + t * t * p1[1],
            )
        )
    return points


def sample_gradient(stops, t):
    t = min(max(t, 0.0), 1.0)
    for i in range(len(stops) - 1):
        t0, c0 = stops[i]
        t1, c1 = stops[i + 1]
        if t0 <= t <= t1:
            k = (t - t0) / (t1 - t0) if t1 > t0 else 0.0
            return tuple(round(c0[j] + (c1[j] - c0[j]) * k) for j in range(3))
    return stops[-1][1]


def stroked(draw: ImageDraw.ImageDraw, a, b, stroke: float) -> None:
    """Segmento com pontas redondas -- o PIL nao tem line caps."""
    draw.line([a, b], fill=255, width=int(stroke))
    r = stroke / 2.0
    for p in (a, b):
        draw.ellipse([p[0] - r, p[1] - r, p[0] + r, p[1] + r], fill=255)


# --------------------------------------------------------------------- Gemini
def gemini() -> None:
    """Faisca de quatro pontas: quatro Beziers quadraticas com o controlo no centro."""
    ry = N * 0.500
    rx = N * 0.400
    tips = [
        (CENTER, CENTER - ry),
        (CENTER + rx, CENTER),
        (CENTER, CENTER + ry),
        (CENTER - rx, CENTER),
    ]

    path = []
    for i in range(4):
        path.extend(quad_bezier(tips[i], (CENTER, CENTER), tips[(i + 1) % 4]))

    mask = Image.new("L", (N, N), 0)
    ImageDraw.Draw(mask).polygon(path, fill=255)

    # Gradiente diagonal azul -> roxo -> rosa: desenhado pequeno e ampliado,
    # que e suave e evita percorrer 4 milhoes de pixeis em Python.
    stops = [(0.0, (66, 133, 244)), (0.55, (155, 114, 203)), (1.0, (217, 101, 112))]
    k = 64
    small = Image.new("RGB", (k, k))
    for y in range(k):
        for x in range(k):
            small.putpixel((x, y), sample_gradient(stops, (x + y) / (2 * (k - 1))))
    paint = small.resize((N, N), Image.BICUBIC)

    finish(mask, paint, "gemini.png")


# -------------------------------------------------------------------- ChatGPT
def chatgpt() -> None:
    """No hexagonal: anel de seis lados com um prolongamento tangente em cada vertice."""
    radius = N * 0.345
    stroke = N * 0.078
    stub = radius * 0.44

    vertices = []
    for k in range(6):
        angle = math.radians(90 + 60 * k)
        vertices.append((CENTER + radius * math.cos(angle), CENTER - radius * math.sin(angle)))

    mask = Image.new("L", (N, N), 0)
    draw = ImageDraw.Draw(mask)

    for k in range(6):
        a = vertices[k]
        stroked(draw, a, vertices[(k + 1) % 6], stroke)

        # Prolongamento: continua a aresta que chega ao vertice, para la dele.
        prev = vertices[(k - 1) % 6]
        dx, dy = a[0] - prev[0], a[1] - prev[1]
        length = math.hypot(dx, dy) or 1.0
        stroked(draw, a, (a[0] + dx / length * stub, a[1] + dy / length * stub), stroke)

    finish(mask, solid((16, 163, 127)), "chatgpt.png")


# --------------------------------------------------------------------- Claude
def claude() -> None:
    """Explosao de raios finos em fuso, convergindo num ponto central."""
    rays = 11
    base = N * 0.44
    lengths = [1.0, 0.82, 0.95, 0.74, 0.92, 0.86, 1.0, 0.78, 0.9, 0.84, 0.97]
    width_ratio = 0.058

    mask = Image.new("L", (N, N), 0)
    draw = ImageDraw.Draw(mask)

    for i in range(rays):
        angle = math.radians(-90 + 360.0 * i / rays)
        length = base * lengths[i % len(lengths)]
        half = length * width_ratio

        upper, lower = [], []
        steps = 96
        for step in range(steps + 1):
            t = step / steps
            # Fuso: ponta fina nas duas extremidades, mais largo a meio.
            w = half * math.sin(math.pi * t) ** 1.15
            x = t * length
            upper.append((x, -w))
            lower.append((x, w))

        cos_a, sin_a = math.cos(angle), math.sin(angle)
        polygon = [
            (CENTER + px * cos_a - py * sin_a, CENTER + px * sin_a + py * cos_a)
            for px, py in upper + lower[::-1]
        ]
        draw.polygon(polygon, fill=255)

    finish(mask, solid((217, 119, 87)), "claude.png")


# ----------------------------------------------------------------------- Home
def home() -> None:
    """Casa em contorno, branca: a barra pinta-a com a cor do tema."""
    stroke = N * 0.072
    apex = (CENTER, N * 0.215)
    eave_l = (N * 0.155, N * 0.495)
    eave_r = (N * 0.845, N * 0.495)
    wall_l = N * 0.265
    wall_r = N * 0.735
    wall_top = N * 0.415
    floor = N * 0.785

    mask = Image.new("L", (N, N), 0)
    draw = ImageDraw.Draw(mask)

    stroked(draw, eave_l, apex, stroke)
    stroked(draw, apex, eave_r, stroke)
    stroked(draw, (wall_l, wall_top), (wall_l, floor), stroke)
    stroked(draw, (wall_r, wall_top), (wall_r, floor), stroke)
    stroked(draw, (wall_l, floor), (wall_r, floor), stroke)

    finish(mask, solid((255, 255, 255)), "home.png")


def main() -> None:
    os.makedirs(OUT_DIR, exist_ok=True)
    gemini()
    chatgpt()
    claude()
    home()


if __name__ == "__main__":
    main()
