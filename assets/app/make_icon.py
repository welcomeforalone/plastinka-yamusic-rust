"""Рисует иконку Plastinka: plastinka.png (256 px) и plastinka.ico (16–256 px).

Геометрия та же, что в plastinka.svg. Нужен Pillow:
    python -I assets/app/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
# Рисуем крупно и уменьшаем: так края сглажены.
SCALE = 8
SIZE = 256
C = SIZE / 2

DISC = (17, 17, 17, 255)
RIM = (46, 46, 46, 255)
GROOVE = (38, 38, 38, 255)
SHEEN = (255, 255, 255, 22)
LABEL = (250, 45, 72, 255)  # акцент «Красный» (0xFA2D48)
LABEL_RING = (214, 30, 58, 255)


def box(r: float) -> list[float]:
    return [(C - r) * SCALE, (C - r) * SCALE, (C + r) * SCALE, (C + r) * SCALE]


def render() -> Image.Image:
    big = SIZE * SCALE
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    # Диск с ободком.
    draw.ellipse(box(120), fill=DISC, outline=RIM, width=3 * SCALE)
    # Дорожки.
    for r in range(56, 113, 8):
        draw.ellipse(box(r), outline=GROOVE, width=int(1.5 * SCALE))
    # Блики: два сектора напротив друг друга.
    sheen = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    sdraw = ImageDraw.Draw(sheen)
    sdraw.pieslice(box(117), -65, -25, fill=SHEEN)
    sdraw.pieslice(box(117), 115, 155, fill=SHEEN)
    img = Image.alpha_composite(img, sheen)
    draw = ImageDraw.Draw(img)
    # Наклейка.
    draw.ellipse(box(44), fill=LABEL)
    draw.ellipse(box(30), outline=LABEL_RING, width=2 * SCALE)
    # Отверстие — прозрачное.
    hole = Image.new("L", (big, big), 255)
    ImageDraw.Draw(hole).ellipse(box(7), fill=0)
    alpha = Image.composite(img.getchannel("A"), Image.new("L", (big, big), 0), hole)
    img.putalpha(alpha)
    return img.resize((SIZE, SIZE), Image.LANCZOS)


def main() -> None:
    icon = render()
    icon.save(HERE / "plastinka.png")
    icon.save(
        HERE / "plastinka.ico",
        sizes=[(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)],
    )


if __name__ == "__main__":
    main()
