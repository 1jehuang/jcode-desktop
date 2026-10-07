//! Files dragged in from the system file manager.
//!
//! Images attach to the prompt exactly like a pasted screenshot, so the model
//! sees them. Every other path (source files, folders, PDFs) is inserted into
//! the prompt text, where the agent reads it with its own file tools. That
//! avoids uploading large or binary files the model cannot use directly.
use super::*;
use std::path::{Path, PathBuf};

/// Largest image attached inline. Bigger files are referenced by path.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Default, PartialEq)]
pub(crate) struct DropPlan {
    pub images: Vec<PathBuf>,
    pub paths: Vec<PathBuf>,
}

pub(crate) fn image_media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => return None,
    })
}

/// Decide which dropped paths become image attachments.
pub(crate) fn plan(paths: &[PathBuf], size_of: impl Fn(&Path) -> Option<u64>) -> DropPlan {
    let mut plan = DropPlan::default();
    for path in paths {
        let small_image = image_media_type(path).is_some()
            && size_of(path).is_some_and(|size| size <= MAX_IMAGE_BYTES);
        if small_image {
            plan.images.push(path.clone());
        } else {
            plan.paths.push(path.clone());
        }
    }
    plan
}

/// Prompt text for referenced paths. Paths with spaces are quoted.
pub(crate) fn path_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| {
            let text = path.to_string_lossy();
            if text.chars().any(char::is_whitespace) {
                format!("\"{text}\"")
            } else {
                text.into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl PromptInput {
    /// Handle a platform file drop on the prompt or its panel.
    pub fn drop_files(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let plan = plan(paths, |path| std::fs::metadata(path).ok().map(|m| m.len()));
        let mut failed = Vec::new();
        for path in &plan.images {
            let image = std::fs::read(path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    crate::clipboard_image::from_bytes(
                        image_media_type(path).unwrap_or("image/png").into(),
                        bytes,
                    )
                });
            match image {
                Ok(image) => self.attach_image(image, cx),
                Err(_) => failed.push(path.clone()),
            }
        }
        // Unreadable images still reach the agent as paths.
        let referenced: Vec<PathBuf> = plan.paths.into_iter().chain(failed).collect();
        if !referenced.is_empty() {
            let mut text = path_text(&referenced);
            let before = self.content[..self.cursor_offset()].chars().next_back();
            if before.is_some_and(|c| !c.is_whitespace()) {
                text.insert(0, ' ');
            }
            text.push(' ');
            self.replace_text_in_range(None, &text, window, cx);
            if plan.images.is_empty() {
                self.attachment_notice = Some(match referenced.len() {
                    1 => "file path added".into(),
                    count => format!("{count} file paths added").into(),
                });
            }
        }
        window.focus(&self.focus_handle.clone(), cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_attach_and_everything_else_is_referenced() {
        let paths = [
            PathBuf::from("/tmp/shot.PNG"),
            PathBuf::from("/tmp/photo.jpeg"),
            PathBuf::from("/tmp/huge.png"),
            PathBuf::from("/tmp/notes.md"),
            PathBuf::from("/tmp/project"),
        ];
        let plan = plan(&paths, |path| {
            Some(if path.ends_with("huge.png") {
                MAX_IMAGE_BYTES + 1
            } else {
                1024
            })
        });
        assert_eq!(
            plan.images,
            vec![
                PathBuf::from("/tmp/shot.PNG"),
                PathBuf::from("/tmp/photo.jpeg")
            ]
        );
        assert_eq!(
            plan.paths,
            vec![
                PathBuf::from("/tmp/huge.png"),
                PathBuf::from("/tmp/notes.md"),
                PathBuf::from("/tmp/project")
            ]
        );
    }

    #[test]
    fn missing_images_are_referenced_not_dropped() {
        let plan = plan(&[PathBuf::from("/gone/a.png")], |_| None);
        assert_eq!(plan.paths, vec![PathBuf::from("/gone/a.png")]);
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        assert_eq!(
            path_text(&[PathBuf::from("/a/b.rs"), PathBuf::from("/my docs/c d.txt")]),
            "/a/b.rs \"/my docs/c d.txt\""
        );
    }
}
