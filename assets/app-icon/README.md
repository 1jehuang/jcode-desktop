# Jcode app icon

The canonical desktop logo is `../icons/jcode.svg`: the website's 76-cell
halftone donut, with 3,344 circle elements. The old traced-path SVG and
50-cell app image have been replaced, not retained as alternate logos.

`icon-1024.png` is a raster fallback of that same mark. Keep it for the Linux,
FreeBSD, Windows, and macOS packaging scripts, which require PNG input.
Provider logos and unrelated UI icons in `../icons/` are not Jcode variants.

To sync after regenerating the website's brand assets, from the desktop root:

```sh
cp ../jcode-website/public/brand/jcode-mark.svg assets/icons/jcode.svg
cp ../jcode-website/public/brand/jcode-mark-1024.png assets/app-icon/icon-1024.png
python3 -m unittest discover -s tests -p test_brand_assets.py
```

To regenerate the packaging PNG directly from the desktop SVG:

```sh
rsvg-convert -w 1024 -h 1024 -b none assets/icons/jcode.svg -o assets/app-icon/icon-1024.png
```
