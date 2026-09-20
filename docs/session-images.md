# Session image pane

Click **Images** in the chat footer to move image previews into a dedicated pane
for that session. Inline display remains the default. The count includes images
read by the model and images attached to your messages, including loaded history.

- The pane initially follows the latest image. Click a thumbnail or an image link
  in the transcript to inspect an earlier image, then **Follow latest** to resume.
- Click the main image to use the existing full-window zoom and pan viewer.
  Escape returns to the pane without changing your draft.
- Click **Inline ×** or **Images** again to return to inline previews.
- Each session remembers its own display mode across UI reload and workspace
  recovery. Reload resumes following the latest image.
- On narrow panels the image pane stacks below the chat instead of squeezing it.

Pane mode leaves compact, clickable image references at their original transcript
positions, preserving the relationship between tool reads and model responses.
Images with unsupported or unavailable previews remain listed with a fallback.

## Verification

```sh
cargo test -p jcode-desktop-ui image_pane
python3 scripts/screenshot.py target/session-images.png --transcript image --image-pane-interact
```

The native acceptance check runs the real application on a private Xvfb display
with offline data, never in the active desktop session.
