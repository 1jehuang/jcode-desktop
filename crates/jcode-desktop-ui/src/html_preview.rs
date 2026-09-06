//! Sandboxed, opt-in HTML artifacts. GPUI owns layout/clipping and input while
//! an isolated WebKit process paints at 2x resolution. Never execute chat HTML
//! in the host app, and never grant the document an app-command bridge.
use crate::theme::Theme;
use base64::Engine;
use gpui::{
    App, Bounds, Context, FocusHandle, ImageSource, IntoElement, MouseButton, Pixels, Render,
    RenderOnce, SharedString, Task, Window, canvas, div, img, prelude::*, px,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_SOURCE: usize = 262_144;
const MAX_OUTPUT: u64 = 12 * 1024 * 1024;
const RUNTIME: &str = include_str!("html_preview_runtime.py");
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

#[derive(IntoElement)]
pub(crate) struct HtmlPreview {
    key: SharedString,
    source: String,
}
impl HtmlPreview {
    pub fn new(source: String, row: usize, block: usize) -> Self {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hash);
        Self {
            key: format!("html-{row}-{block}-{:x}", hash.finish()).into(),
            source,
        }
    }
}
impl RenderOnce for HtmlPreview {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        window.use_keyed_state(self.key, cx, |_, cx| Preview::new(self.source, cx))
    }
}

struct Worker {
    commands: mpsc::SyncSender<Value>,
}
enum Update {
    Frame(Arc<gpui::RenderImage>),
    Error(String),
}
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Worker {
    fn start(
        source: String,
        ids: crate::image_cache::ImageIds,
    ) -> Result<(Self, async_channel::Receiver<Update>), String> {
        if source.len() > MAX_SOURCE {
            return Err("Preview exceeds the 256 KiB limit.".into());
        }
        if !cfg!(target_os = "linux") {
            return Err("Interactive HTML previews currently require Linux with WebKitGTK 4.1. Source is still available.".into());
        }
        if ACTIVE
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 3).then_some(n + 1)
            })
            .is_err()
        {
            return Err(
                "Three previews are already active. Pause another preview, then retry.".into(),
            );
        }
        let slot = Slot;
        let (commands, receiver) = mpsc::sync_channel::<Value>(128);
        let (updates, output) = async_channel::bounded(2);
        std::thread::spawn(move || {
            let _slot = slot;
            let result = run_worker(source, receiver, updates.clone(), ids);
            if let Err(error) = result {
                let _ = updates.send_blocking(Update::Error(error));
            }
        });
        Ok((Self { commands }, output))
    }
    fn send(&self, command: Value) {
        let _ = self.commands.try_send(command);
    }
}

fn run_worker(
    source: String,
    commands: mpsc::Receiver<Value>,
    updates: async_channel::Sender<Update>,
    ids: crate::image_cache::ImageIds,
) -> Result<(), String> {
    let mut command = Command::new("/usr/bin/python3");
    command
        .args(["-I", "-u", "-c", RUNTIME])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| {
        format!("Cannot start HTML runtime: {e}. Install Python GI, Cairo, WebKitGTK 4.1 and Xvfb.")
    })?;
    let mut stdin = child.stdin.take().ok_or("HTML runtime stdin unavailable")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("HTML runtime stdout unavailable")?;
    let first_frame = Arc::new(AtomicUsize::new(0));
    let seen = first_frame.clone();
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.by_ref().take(MAX_OUTPUT + 1).read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if line.len() as u64 > MAX_OUTPUT => {
                    let _ = updates
                        .send_blocking(Update::Error("Preview output exceeded its limit.".into()));
                    break;
                }
                _ => {}
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let update = match value["type"].as_str() {
                Some("frame") => {
                    seen.store(1, Ordering::Relaxed);
                    let Some(encoded) = value["png"].as_str() else {
                        continue;
                    };
                    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded)
                    else {
                        continue;
                    };
                    let mut decoder = image::ImageReader::with_format(
                        std::io::Cursor::new(bytes),
                        image::ImageFormat::Png,
                    );
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(3200);
                    limits.max_image_height = Some(1800);
                    decoder.limits(limits);
                    let Ok(decoded) = decoder.decode() else {
                        continue;
                    };
                    let mut rgba = decoded.to_rgba8();
                    // GPUI textures use BGRA. Decode off the UI thread, then
                    // hand it a ready frame instead of accumulating asset jobs.
                    for pixel in rgba.pixels_mut() {
                        pixel.0.swap(0, 2);
                    }
                    Update::Frame(ids.render(vec![image::Frame::new(rgba)]))
                }
                Some("error") => Update::Error(
                    value["message"]
                        .as_str()
                        .unwrap_or("Preview failed")
                        .to_owned(),
                ),
                _ => continue,
            };
            if updates.send_blocking(update).is_err() {
                break;
            }
        }
    });
    let write_command =
        |stdin: &mut std::process::ChildStdin, value: &Value| -> std::io::Result<()> {
            serde_json::to_writer(&mut *stdin, value)?;
            stdin.write_all(b"\n")?;
            stdin.flush()
        };
    let started = Instant::now();
    let result = (|| {
        write_command(&mut stdin, &json!({"type":"load", "html":source}))
            .map_err(|e| e.to_string())?;
        loop {
            match commands.recv_timeout(Duration::from_millis(100)) {
                Ok(value) => write_command(&mut stdin, &value).map_err(|e| e.to_string())?,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "HTML runtime exited ({status}). Install python-gobject, python-cairo, webkit2gtk-4.1 and xorg-server-xvfb."
                ));
            }
            if first_frame.load(Ordering::Relaxed) == 0
                && started.elapsed() > Duration::from_secs(20)
            {
                return Err("HTML preview timed out. Check the document or retry.".into());
            }
        }
        Ok(())
    })();
    let _ = write_command(&mut stdin, &json!({"type":"quit"}));
    drop(stdin);
    let deadline = Instant::now() + Duration::from_millis(700);
    while Instant::now() < deadline && child.try_wait().ok().flatten().is_none() {
        std::thread::sleep(Duration::from_millis(20));
    }
    #[cfg(unix)]
    unsafe {
        // The helper, private Xvfb, and WebKit children belong to this new group.
        // Cleanup never targets the user's browser or desktop display.
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    result
}

#[cfg(test)]
#[derive(Default)]
struct TestPreviewInstances(std::collections::HashMap<String, Vec<gpui::EntityId>>);
#[cfg(test)]
impl gpui::Global for TestPreviewInstances {}

/// Test-only creation history distinguishes a retained Preview from an identical
/// new card, without depending on WebKit timing or requiring a native display.
#[cfg(test)]
pub(crate) fn test_instance_ids(source: &str, cx: &App) -> Vec<gpui::EntityId> {
    if !cx.has_global::<TestPreviewInstances>() {
        return Vec::new();
    }
    cx.global::<TestPreviewInstances>()
        .0
        .get(source.trim())
        .cloned()
        .unwrap_or_default()
}

struct Preview {
    source: String,
    image: Option<Arc<gpui::RenderImage>>,
    error: Option<String>,
    worker: Option<Worker>,
    task: Option<Task<()>>,
    show_source: bool,
    expanded: bool,
    focus: FocusHandle,
    bounds: Rc<RefCell<Bounds<Pixels>>>,
    viewport: (u32, u32),
    dragging: bool,
}
impl Preview {
    fn new(source: String, cx: &mut Context<Self>) -> Self {
        #[cfg(test)]
        {
            let id = cx.entity().entity_id();
            cx.default_global::<TestPreviewInstances>()
                .0
                .entry(source.trim().to_owned())
                .or_default()
                .push(id);
        }
        let mut this = Self {
            source,
            image: None,
            error: None,
            worker: None,
            task: None,
            show_source: false,
            expanded: false,
            focus: cx.focus_handle(),
            bounds: Rc::new(RefCell::new(Bounds::default())),
            viewport: (800, 420),
            dragging: false,
        };
        cx.on_release(|this, cx| {
            if let Some(image) = this.image.take() {
                cx.defer(move |cx| cx.drop_image(image, None));
            }
        })
        .detach();
        this.start(cx);
        this
    }
    fn start(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        match Worker::start(self.source.clone(), crate::image_cache::ImageIds::get(cx)) {
            Ok((worker, updates)) => {
                self.worker = Some(worker);
                self.task = Some(cx.spawn(async move |view, cx| {
                    while let Ok(update) = updates.recv().await {
                        if view
                            .update(cx, |view, cx| {
                                match update {
                                    Update::Frame(image) => {
                                        if let Some(old) = view.image.replace(image) {
                                            cx.drop_image(old, None);
                                        }
                                        view.error = None;
                                    }
                                    Update::Error(error) => {
                                        view.error = Some(error);
                                        view.worker = None;
                                    }
                                }
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
                self.send(
                    json!({"type":"resize", "width":self.viewport.0, "height":self.viewport.1}),
                );
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn send(&self, value: Value) {
        if let Some(worker) = &self.worker {
            worker.send(value);
        }
    }
    fn position(&self, position: gpui::Point<Pixels>) -> (f32, f32) {
        let bounds = self.bounds.borrow();
        browser_position(
            (
                f32::from(position.x - bounds.origin.x),
                f32::from(position.y - bounds.origin.y),
            ),
            (f32::from(bounds.size.width), f32::from(bounds.size.height)),
            self.viewport,
        )
    }
    fn pointer(&self, kind: &str, position: gpui::Point<Pixels>) {
        let (x, y) = self.position(position);
        self.send(json!({"type":kind, "x":x, "y":y}));
    }
}
impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global();
        let copy = self.source.clone();
        let mut root = div()
            .id("html-preview-card")
            .debug_selector(|| "html-preview-card".into())
            .w_full()
            .min_w_0()
            .my_2()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_1()
            .border_color(theme.CODE_BORDER)
            .rounded_md()
            .bg(theme.CODE_BG)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(theme.TEXT_DIM)
                    .bg(theme.CODE_HEADER_BG)
                    .child("HTML preview · isolated · offline")
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .id("html-source")
                                    .cursor_pointer()
                                    .child(if self.show_source {
                                        "Preview"
                                    } else {
                                        "Source"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_source = !this.show_source;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("html-copy")
                                    .cursor_pointer()
                                    .child("Copy")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .id("html-expand")
                                    .cursor_pointer()
                                    .child(if self.expanded { "Collapse" } else { "Expand" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.expanded = !this.expanded;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("html-pause")
                                    .cursor_pointer()
                                    .child(if self.worker.is_some() {
                                        "Pause"
                                    } else {
                                        "Retry"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.worker.is_some() {
                                            this.worker = None;
                                            this.task = None;
                                        } else {
                                            this.start(cx);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        if self.show_source {
            return root.child(crate::markdown::code_block("html", &self.source, window));
        }
        if let Some(error) = &self.error {
            return root.child(
                div()
                    .p_3()
                    .text_size(px(12.0))
                    .text_color(theme.TEXT_DIM)
                    .child(error.clone()),
            );
        }
        let height = if self.expanded { 700.0 } else { 420.0 };
        let bounds = self.bounds.clone();
        let entity = cx.entity().downgrade();
        let surface = div().id("html-preview-surface").debug_selector(|| "html-preview-surface".into())
            .relative().w_full().h(px(height)).overflow_hidden().bg(gpui::rgb(0xffffff))
            .track_focus(&self.focus).key_context("HtmlPreview")
            // Parent transcript/workspace routers listen in capture phase.
            // A focused surface must occlude their hitboxes before dispatch.
            .when(self.focus.is_focused(window), |el| el.occlude())
            .when_some(self.image.clone(), |el, image| el.child(img(ImageSource::Render(image)).w_full().h_full().object_fit(gpui::ObjectFit::Fill)))
            .when(self.image.is_none(), |el| el.child(div().p_4().text_color(gpui::rgb(0x666666)).child("Rendering HTML…")))
            .child(canvas(move |area, _, cx| {
                *bounds.borrow_mut() = area;
                let viewport = (f32::from(area.size.width).round().clamp(240.0, 1600.0) as u32, height as u32);
                let _ = entity.update(cx, |this, _| {
                    if this.viewport != viewport {
                        this.viewport = viewport;
                        this.send(json!({"type":"resize", "width":viewport.0, "height":viewport.1}));
                    }
                });
            }, |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                this.focus.focus(window, cx); this.dragging = true;
                this.pointer("down", event.position); cx.stop_propagation();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                this.dragging = false; this.pointer("up", event.position); cx.stop_propagation();
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, event: &gpui::MouseUpEvent, _, _| {
                if this.dragging { this.dragging = false; this.pointer("up", event.position); }
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, _| {
                if this.dragging { this.pointer("move", event.position); }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    window.blur();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.modifiers.control || event.keystroke.modifiers.platform || event.keystroke.modifiers.alt { return; }
                this.send(json!({"type":"key", "key":event.keystroke.key, "shift":event.keystroke.modifiers.shift}));
                cx.stop_propagation();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, window, cx| {
                // The transcript keeps its normal scroll until the user focuses
                // the preview. Escape/outer click returns focus to the app.
                if !this.focus.is_focused(window) { return; }
                let delta = event.delta.pixel_delta(px(20.0));
                let (x, y) = this.position(event.position);
                this.send(json!({"type":"scroll", "x":x, "y":y,
                    "dx":-f32::from(delta.x)/40.0, "dy":-f32::from(delta.y)/40.0}));
                cx.stop_propagation();
            }));
        root = root.child(surface);
        root.child(
            div()
                .px_3()
                .py_1()
                .text_size(px(10.0))
                .text_color(theme.TEXT_FAINT)
                .child(if self.worker.is_some() {
                    "Click to interact · local fonts · no network, files, or app access"
                } else {
                    "Paused · Retry restarts the document"
                }),
        )
    }
}

// The browser viewport is bounded while GPUI may paint wider/narrower cards.
fn browser_position(position: (f32, f32), size: (f32, f32), viewport: (u32, u32)) -> (f32, f32) {
    (
        position.0 * viewport.0 as f32 / size.0.max(1.0),
        position.1 * viewport.1 as f32 / size.1.max(1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_tracks_scaled_browser_surface() {
        assert_eq!(
            browser_position((1200.0, 210.0), (2400.0, 420.0), (1600, 420)),
            (800.0, 210.0)
        );
        assert_eq!(
            browser_position((100.0, 210.0), (200.0, 420.0), (240, 420)),
            (120.0, 210.0)
        );
    }
    #[test]
    fn rejects_large_documents_before_starting_a_process() {
        assert!(
            Worker::start(
                "x".repeat(MAX_SOURCE + 1),
                crate::image_cache::ImageIds::default()
            )
            .is_err()
        );
    }
    #[test]
    fn preview_key_separates_content_and_position() {
        assert_ne!(
            HtmlPreview::new("a".into(), 0, 0).key,
            HtmlPreview::new("b".into(), 0, 0).key
        );
        assert_ne!(
            HtmlPreview::new("a".into(), 0, 0).key,
            HtmlPreview::new("a".into(), 1, 0).key
        );
    }
}
