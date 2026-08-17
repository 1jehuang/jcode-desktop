//! A small embedded PTY terminal used by plain terminal panels.

use std::io::{Read, Write};
use std::ops::Range;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use gpui::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, KeyBinding,
    KeyDownEvent, Pixels, Render, UTF16Selection, Window, actions, canvas, div, prelude::*, px,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::theme::Theme;

const ROWS: u16 = 40;
const COLS: u16 = 120;

actions!(
    terminal,
    [
        Enter, Backspace, Tab, Escape, Up, Down, Left, Right, Home, End, Delete
    ]
);

pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        KeyBinding::new("enter", Enter, Some("Terminal")),
        KeyBinding::new("backspace", Backspace, Some("Terminal")),
        KeyBinding::new("tab", Tab, Some("Terminal")),
        KeyBinding::new("escape", Escape, Some("Terminal")),
        KeyBinding::new("up", Up, Some("Terminal")),
        KeyBinding::new("down", Down, Some("Terminal")),
        KeyBinding::new("left", Left, Some("Terminal")),
        KeyBinding::new("right", Right, Some("Terminal")),
        KeyBinding::new("home", Home, Some("Terminal")),
        KeyBinding::new("end", End, Some("Terminal")),
        KeyBinding::new("delete", Delete, Some("Terminal")),
    ]);
}

pub struct TerminalPanel {
    focus: FocusHandle,
    parser: vt100::Parser,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    // Keep the process handle alive for the lifetime of the panel. Some PTY
    // backends terminate or reap the child when this handle is dropped.
    _child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    status: String,
    _poll: gpui::Task<()>,
}

impl TerminalPanel {
    pub fn new(working_dir: Option<String>, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let mut writer = None;
        let mut child = None;
        let mut status = String::new();

        match native_pty_system().openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => {
                // Desktop terminals should be predictable rather than inheriting
                // whichever login shell happened to launch the GUI. Fish is the
                // default, with conservative fallbacks for systems without it.
                let shell = default_shell();
                let is_fish = shell.ends_with("/fish") || shell == "fish";
                let mut command = CommandBuilder::new(shell);
                if is_fish {
                    command.arg("--interactive");
                }
                // vt100 parses display output but does not answer terminal capability
                // queries. Advertising xterm makes fish wait 10 seconds for replies;
                // dumb starts immediately and remains fully interactive.
                command.env("TERM", "dumb");
                if let Some(dir) = working_dir {
                    command.cwd(dir);
                }
                match pair.slave.spawn_command(command) {
                    Ok(pty_child) => {
                        child = Some(pty_child);
                        drop(pair.slave);
                        match (pair.master.try_clone_reader(), pair.master.take_writer()) {
                            (Ok(mut reader), Ok(pty_writer)) => {
                                writer = Some(Arc::new(Mutex::new(pty_writer)));
                                std::thread::Builder::new()
                                    .name("jcode-terminal-reader".into())
                                    .spawn(move || {
                                        let mut buf = [0u8; 8192];
                                        loop {
                                            match reader.read(&mut buf) {
                                                Ok(0) => break,
                                                Ok(count) => {
                                                    if tx.send(buf[..count].to_vec()).is_err() {
                                                        break;
                                                    }
                                                }
                                                Err(error)
                                                    if error.kind()
                                                        == std::io::ErrorKind::Interrupted => {}
                                                // Linux PTY masters can briefly return EIO
                                                // between spawn and the child opening the slave.
                                                Err(error) if error.raw_os_error() == Some(5) => {
                                                    std::thread::sleep(Duration::from_millis(10));
                                                }
                                                Err(_) => break,
                                            }
                                        }
                                    })
                                    .ok();
                            }
                            _ => status = "could not connect to terminal PTY".into(),
                        }
                    }
                    Err(error) => status = format!("could not start shell: {error}"),
                }
            }
            Err(error) => status = format!("could not open PTY: {error}"),
        }

        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let chunks = rx.try_iter().collect::<Vec<_>>();
                if chunks.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |this, cx| {
                        for chunk in chunks {
                            this.parser.process(&chunk);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            focus: cx.focus_handle(),
            parser: vt100::Parser::new(ROWS, COLS, 10_000),
            writer,
            _child: child,
            status,
            _poll: poll,
        }
    }

    fn send(&self, bytes: &[u8]) {
        if let Some(writer) = &self.writer
            && let Ok(mut writer) = writer.lock()
        {
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    fn send_text(&self, text: &str) {
        if text.contains('\n') || text.contains('\r') {
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            self.send(normalized.as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        // Super combinations belong to the workspace. Do not turn Super+O/N/Q
        // into shell input or stop their action from bubbling.
        if modifiers.platform || modifiers.function {
            return;
        }
        let key = event.keystroke.key.as_str();
        if modifiers.control && !modifiers.alt {
            if let Some(ch) = key.chars().next().filter(|ch| ch.is_ascii_alphabetic()) {
                self.send(&[(ch.to_ascii_uppercase() as u8) - b'@']);
            }
        } else if !modifiers.alt && event.keystroke.key_char.is_none() && key.chars().count() == 1 {
            // Synthetic/raw keyboard sources may not generate an IME commit.
            // Normal text still comes through EntityInputHandler, avoiding
            // duplicate characters on Wayland.
            self.send(key.as_bytes());
        }
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\r");
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, _: &mut Context<Self>) {
        self.send(&[0x7f]);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\t");
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, _: &mut Context<Self>) {
        self.send(&[0x1b]);
    }
    fn up(&mut self, _: &Up, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[A");
    }
    fn down(&mut self, _: &Down, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[B");
    }
    fn left(&mut self, _: &Left, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[D");
    }
    fn right(&mut self, _: &Right, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[C");
    }
    fn home(&mut self, _: &Home, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[H");
    }
    fn end(&mut self, _: &End, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[F");
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[3~");
    }

    pub fn focus(&self, window: &mut Window, cx: &mut gpui::App) {
        window.focus(&self.focus, cx);
    }

    #[cfg(test)]
    pub fn screen_contents(&self) -> String {
        self.parser.screen().contents()
    }
}

// Printable text on Wayland/macOS is delivered through GPUI's text-input/IME
// path, not reliably through KeyDownEvent. Registering this handler is what
// makes normal typing reach the PTY. Workspace shortcuts remain actions and
// continue bubbling through the terminal to Workspace.
impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        actual_range.replace(0..0);
        Some(String::new())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.send_text(new_text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.send_text(new_text);
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(bounds.origin, gpui::size(px(1.0), px(18.0))))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

fn default_shell() -> String {
    ["/usr/bin/fish", "/bin/fish"]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .map(str::to_owned)
        .or_else(|| {
            std::env::var("SHELL")
                .ok()
                .filter(|shell| !shell.is_empty())
        })
        .unwrap_or_else(|| "/bin/sh".into())
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.status.is_empty() {
            self.parser.screen().contents()
        } else {
            self.status.clone()
        };
        let input = cx.entity();
        div()
            .id("plain-terminal")
            .debug_selector(|| "plain-terminal".into())
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .p_3()
            .bg(Theme::BG)
            .font_family(Theme::FONT_MONO)
            .text_size(px(13.0))
            .text_color(Theme::TEXT)
            .whitespace_nowrap()
            // One element per terminal row preserves line boundaries. A single
            // text child is laid out like prose and can collapse terminal output.
            .children(
                content
                    .split('\n')
                    .map(|line| div().h(px(18.0)).child(line.to_owned())),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let focus = input.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, input.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::delete))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus(window, cx)),
            )
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fish_is_the_default_shell_when_installed() {
        let shell = super::default_shell();
        if std::path::Path::new("/usr/bin/fish").is_file() {
            assert_eq!(shell, "/usr/bin/fish");
        }
    }
}
