from pathlib import Path

from PIL import Image

ASSETS = Path(__file__).resolve().parent
ICON_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
WINDOW_ICON_SIZE = 256


def main():
    master = Image.open(ASSETS / "chromafree.png").convert("RGBA")
    master.save(ASSETS / "chromafree.ico", sizes=[(size, size) for size in ICON_SIZES])
    master.resize((WINDOW_ICON_SIZE, WINDOW_ICON_SIZE), Image.Resampling.LANCZOS).save(
        ASSETS / "chromafree-256.png", optimize=True
    )


if __name__ == "__main__":
    main()
