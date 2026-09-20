//! Embedded, monochrome icons for tool-call headers.
//! No asset loader registration or filesystem access is required.

use crate::theme::Theme;
use gpui::{Div, div, prelude::*, px};

/// A stable-sized icon that never compresses the tool-call label.
pub(crate) fn render(name: &str) -> Div {
    div()
        .debug_selector(|| "tool-type-icon".into())
        .size(px(14.0))
        .flex_shrink_0()
        .child(
            gpui::svg()
                .data(icon_data(name))
                .size(px(14.0))
                .text_color(Theme::global().TEXT_FAINT),
        )
}

/// Strip only recognized wrappers. Do not mistake an arbitrary MCP tool named
/// `read` for the built-in file reader. All third-party MCP tools use the plug.
fn normalized_name(name: &str) -> &str {
    let name = name
        .trim()
        .strip_prefix("functions.")
        .unwrap_or(name.trim());
    if let Some(rest) = name.strip_prefix("mcp__") {
        return match rest.split_once("__") {
            Some(("node_repl" | "node-repl", tool)) if !tool.is_empty() => "node_repl",
            Some((server, tool)) if !server.is_empty() && !tool.is_empty() => "mcp",
            _ => name,
        };
    }
    // MCP display titles use mcp:server:tool instead of the dispatch spelling.
    if let Some(rest) = name.strip_prefix("mcp:") {
        return match rest.split_once(':') {
            Some(("node_repl" | "node-repl", tool)) if !tool.is_empty() => "node_repl",
            Some((server, tool)) if !server.is_empty() && !tool.is_empty() => "mcp",
            _ => name,
        };
    }
    if name
        .strip_prefix("node_repl.")
        .is_some_and(|tool| !tool.is_empty())
    {
        return "node_repl";
    }
    name
}

fn icon_data(name: &str) -> &'static [u8] {
    macro_rules! icon {
        ($name:literal) => {
            include_bytes!(concat!("../../../assets/icons/tools/", $name, ".svg"))
                as &'static [u8]
        };
    }
    match normalized_name(name) {
        "imagegen" | "image_gen" | "image_gen.imagegen" => icon!("imagegen"),
        "read" => icon!("read"),
        "write" => icon!("write"),
        "edit" => icon!("edit"),
        "multiedit" => icon!("multiedit"),
        "patch" | "apply_patch" => icon!("patch"),
        "bash" => icon!("bash"),
        "ls" => icon!("ls"),
        "agentgrep" => icon!("agentgrep"),
        "glob" => icon!("glob"),
        "grep" => icon!("grep"),
        "find" => icon!("find"),
        "websearch" => icon!("websearch"),
        "webfetch" => icon!("webfetch"),
        "browser" => icon!("browser"),
        "gmail" => icon!("gmail"),
        "memory" => icon!("memory"),
        "todo" => icon!("todo"),
        "schedule" | "schedule_ambient" => icon!("schedule"),
        "swarm" => icon!("swarm"),
        "batch" => icon!("batch"),
        "bg" => icon!("bg"),
        "panel" | "side_panel" => icon!("panel"),
        "open" => icon!("open"),
        "compile_remote" => icon!("compile_remote"),
        "integration_tools" => icon!("integration_tools"),
        "mcp" | "mcp_search" | "mcp_call" => icon!("mcp"),
        "node_repl" => icon!("node_repl"),
        "jcode_docs" => icon!("jcode_docs"),
        "conversation_search" => icon!("conversation_search"),
        "session_search" => icon!("session_search"),
        "skill_manage" => icon!("skill_manage"),
        "maintainer_feedback" => icon!("maintainer_feedback"),
        "invalid" => icon!("invalid"),
        "selfdev" => icon!("selfdev"),
        "desktop_selfdev" => icon!("desktop_selfdev"),
        "debug_socket" => icon!("debug_socket"),
        "computer" | "macos_computer_use" => icon!("computer"),
        "initiative" => icon!("initiative"),
        "end_ambient_cycle" => icon!("end_ambient_cycle"),
        "request_permission" => icon!("request_permission"),
        "send_message" => icon!("send_message"),
        _ => icon!("generic"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Snapshot of production names in jcode-app-core/src/tool (including
    // conditional, ambient, MCP management and self-development tools), plus
    // compatibility names exposed by the functions API.
    const BUILT_INS: &[&str] = &[
        "imagegen",
        "image_gen",
        "image_gen.imagegen",
        "read",
        "write",
        "edit",
        "multiedit",
        "patch",
        "bash",
        "ls",
        "agentgrep",
        "glob",
        "grep",
        "find",
        "websearch",
        "webfetch",
        "browser",
        "gmail",
        "memory",
        "todo",
        "schedule",
        "swarm",
        "batch",
        "bg",
        "panel",
        "open",
        "compile_remote",
        "integration_tools",
        "mcp",
        "node_repl",
        "jcode_docs",
        "conversation_search",
        "session_search",
        "skill_manage",
        "maintainer_feedback",
        "invalid",
        "selfdev",
        "desktop_selfdev",
        "debug_socket",
        "computer",
        "initiative",
        "end_ambient_cycle",
        "request_permission",
        "send_message",
        "apply_patch",
        "side_panel",
        "schedule_ambient",
        "mcp_search",
        "mcp_call",
        "macos_computer_use",
    ];

    #[test]
    fn every_builtin_has_an_embedded_icon() {
        let fallback = icon_data("unknown_tool");
        for name in BUILT_INS {
            let data = icon_data(name);
            assert_ne!(data, fallback, "missing icon for {name}");
            let svg = std::str::from_utf8(data).expect("UTF-8 SVG");
            assert!(svg.starts_with("<svg "));
            assert!(svg.contains("viewBox=\"0 0 24 24\""));
            assert!(svg.contains("stroke=\"currentColor\""));
            assert!(svg.ends_with("</svg>\n"));
        }
    }

    #[test]
    fn function_namespace_preserves_every_mapping() {
        for name in BUILT_INS {
            assert_eq!(icon_data(name), icon_data(&format!("functions.{name}")));
        }
        assert_eq!(icon_data("read"), icon_data("  functions.read  "));
    }

    #[test]
    fn related_aliases_share_icons() {
        for (name, alias) in [
            ("imagegen", "image_gen.imagegen"),
            ("imagegen", "image_gen"),
            ("patch", "apply_patch"),
            ("panel", "side_panel"),
            ("schedule", "schedule_ambient"),
            ("mcp", "mcp_search"),
            ("mcp", "mcp_call"),
            ("computer", "macos_computer_use"),
        ] {
            assert_eq!(icon_data(name), icon_data(alias));
        }
    }

    #[test]
    fn mcp_names_keep_the_server_identity() {
        for name in [
            "mcp__node_repl__js",
            "functions.mcp__node_repl__js_reset",
            "mcp__node_repl__js_add_node_module_dir",
            "mcp__node_repl__turn_ended",
            "mcp:node-repl:js",
            "node_repl.js",
            "functions.node_repl.js",
        ] {
            assert_eq!(icon_data(name), icon_data("node_repl"), "{name}");
        }
        for name in [
            "mcp__files__read",
            "functions.mcp__service__action",
            "mcp:service:action",
        ] {
            assert_eq!(icon_data(name), icon_data("mcp"), "{name}");
        }
        assert_ne!(icon_data("mcp__files__read"), icon_data("read"));
    }

    #[test]
    fn unknown_and_malformed_names_have_a_generic_fallback() {
        for name in [
            "",
            " ",
            "unknown",
            "functions.unknown",
            "other.read",
            "READ",
            "mcp__",
            "mcp____read",
            "mcp__node_repl__",
            "mcp:node_repl:",
            "node_repl.",
            "js",
        ] {
            assert_eq!(icon_data(name), icon_data("unknown_tool"), "{name}");
        }
    }

    #[test]
    fn different_tool_types_are_visually_distinct() {
        let types = [
            "imagegen",
            "read",
            "write",
            "edit",
            "multiedit",
            "patch",
            "bash",
            "ls",
            "agentgrep",
            "glob",
            "grep",
            "find",
            "websearch",
            "webfetch",
            "browser",
            "gmail",
            "memory",
            "todo",
            "schedule",
            "swarm",
            "batch",
            "bg",
            "panel",
            "open",
            "compile_remote",
            "integration_tools",
            "mcp",
            "node_repl",
            "jcode_docs",
            "conversation_search",
            "session_search",
            "skill_manage",
            "maintainer_feedback",
            "invalid",
            "selfdev",
            "desktop_selfdev",
            "debug_socket",
            "computer",
            "initiative",
            "end_ambient_cycle",
            "request_permission",
            "send_message",
        ];
        for (index, name) in types.iter().enumerate() {
            for other in &types[index + 1..] {
                assert_ne!(icon_data(name), icon_data(other), "{name} and {other}");
            }
        }
    }
}
