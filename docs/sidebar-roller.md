# Sidebar folder roller

Folder layout uses a cylindrical carousel for the top-left sidebar navigation.
The live-session tabs above the conversation canvas are independent and unchanged
by this feature. Normal layout retains its horizontal navigation strip.

- The centered folder is full size. Neighboring folders recede symmetrically in
  height, width, and vertical position. Far folders paint behind nearer ones.
- Wheel/trackpad input and the two chevrons browse all eleven folders cyclically.
  Browsing does not select a page or launch an action. Click a folder to activate it.
- Tab labels stay inside each exposed face. Tooltips provide the full labels of
  compressed tabs. The centered active folder joins its sidebar page.
- Motion uses the existing Focus transition and reduced-motion policy. Hidden
  rollers do not keep scheduling animation frames. Reload restores the centered
  folder from the existing sidebar-view snapshot without changing its schema.
- Each occluding folder face handles wheel events itself and stops propagation,
  so real native wheel input works on the tabs, not just their backdrop.

## Checks

```sh
cargo test -p jcode-desktop-ui --lib sidebar_roller -- --include-ignored --test-threads=1
python3 scripts/screenshot.py target/roller-review.png
python3 scripts/accept-roller.py target/roller-native
```

The native acceptance runner requires the current desktop binary, Xvfb, Openbox,
xdotool, ImageMagick, Pillow, Tesseract, and Mesa lavapipe. It uses an isolated HOME,
configuration, runtime directory, and offline fixture on a private display. It
does not interact with the user's desktop or account credentials.
