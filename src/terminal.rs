//! A small embedded PTY terminal used by plain terminal panels.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use gpui::{Context, FocusHandle, Focusable, KeyDownEvent, Render, Window, div, prelude::*, px};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::theme::Theme;

const ROWS: u16 = 40;
const COLS: u16 = 120;

pub struct TerminalPanel {
    focus: FocusHandle,
    parser: vt100::Parser,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    status: String,
    _poll: gpui::Task<()>,
}

impl TerminalPanel {
    pub fn new(working_dir: Option<String>, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let mut writer = None;
        let mut status = String::new();

        match native_pty_system().openpty(PtySize {
            rows: ROWS,
            cols: COLS,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                let mut command = CommandBuilder::new(shell);
                if let Some(dir) = working_dir {
                    command.cwd(dir);
                }
                match pair.slave.spawn_command(command) {
                    Ok(_child) => {
                        drop(pair.slave);
                        match (pair.master.try_clone_reader(), pair.master.take_writer()) {
                            (Ok(mut reader), Ok(pty_writer)) => {
                                writer = Some(Arc::new(Mutex::new(pty_writer)));
                                std::thread::Builder::new()
                                    .name("jcode-terminal-reader".into())
                                    .spawn(move || {
                                        let mut buf = [0u8; 8192];
                                        while let Ok(count) = reader.read(&mut buf) {
                                            if count == 0 {
                                                break;
                                            }
                                            if tx.send(buf[..count].to_vec()).is_err() {
                                                break;
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

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let bytes: Option<Vec<u8>> = if event.keystroke.modifiers.control {
            key.chars()
                .next()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| vec![(c.to_ascii_uppercase() as u8) - b'@'])
        } else {
            match key {
                "enter" => Some(vec![b'\r']),
                "backspace" => Some(vec![0x7f]),
                "tab" => Some(vec![b'\t']),
                "escape" => Some(vec![0x1b]),
                "up" => Some(b"\x1b[A".to_vec()),
                "down" => Some(b"\x1b[B".to_vec()),
                "right" => Some(b"\x1b[C".to_vec()),
                "left" => Some(b"\x1b[D".to_vec()),
                "home" => Some(b"\x1b[H".to_vec()),
                "end" => Some(b"\x1b[F".to_vec()),
                "delete" => Some(b"\x1b[3~".to_vec()),
                "pageup" => Some(b"\x1b[5~".to_vec()),
                "pagedown" => Some(b"\x1b[6~".to_vec()),
                _ => event
                    .keystroke
                    .key_char
                    .as_ref()
                    .map(|s| s.as_bytes().to_vec()),
            }
        };
        if let Some(bytes) = bytes {
            self.send(&bytes);
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut gpui::App) {
        window.focus(&self.focus, cx);
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.status.is_empty() {
            self.parser.screen().contents()
        } else {
            self.status.clone()
        };
        div()
            .id("plain-terminal")
            .debug_selector(|| "plain-terminal".into())
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .p_3()
            .bg(Theme::BG)
            .font_family(Theme::FONT_MONO)
            .text_size(px(13.0))
            .text_color(Theme::TEXT)
            .whitespace_nowrap()
            .child(content)
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
