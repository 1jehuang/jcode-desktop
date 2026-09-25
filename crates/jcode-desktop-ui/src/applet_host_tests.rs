use super::*;
use jcode_applet_types::{Capability, PatchOp};
use serde_json::json;

fn register(host: &mut AppletHost, capabilities: &[Capability]) {
    host.apply(
        "demo",
        ProviderMessage::Register {
            manifest: serde_json::from_value(json!({
                "schema": jcode_applet_types::SCHEMA,
                "id": "demo",
                "title": "Demo",
                "capabilities": capabilities,
            }))
            .unwrap(),
        },
    )
    .unwrap();
}

fn mount(
    host: &mut AppletHost,
    id: &str,
    placement: serde_json::Value,
    lifetime: &str,
) -> Result<(), String> {
    host.apply(
        "demo",
        serde_json::from_value(json!({
            "type": "mount",
            "instance": id,
            "placement": placement,
            "lifetime": lifetime,
            "document": {
                "revision": 1,
                "title": "Demo",
                "state": {"q": ""},
                "view": {"type": "stack", "children": [
                    {"type": "text", "text": "hello"},
                    {"type": "button", "label": "Go", "on_press": {"action": "go"}}
                ]}
            }
        }))
        .unwrap(),
    )
}

#[test]
fn mounts_at_any_placement_without_a_tool_call() {
    let mut host = AppletHost::new();
    register(&mut host, &[]);
    mount(&mut host, "panel", json!({"kind": "panel"}), "session").unwrap();
    mount(&mut host, "hud", json!({"kind": "overlay"}), "session").unwrap();
    mount(&mut host, "card", json!({"kind": "inline", "session_id": "s1", "anchor": {"kind": "tool_call", "call_id": "c1"}}), "session").unwrap();
    mount(
        &mut host,
        "end",
        json!({"kind": "inline", "session_id": "s1", "anchor": {"kind": "end"}}),
        "session",
    )
    .unwrap();
    assert_eq!(host.at(|p| matches!(p, Placement::Panel { .. })).count(), 1);
    assert_eq!(
        host.at(|p| matches!(p, Placement::Overlay { .. })).count(),
        1
    );
    assert_eq!(host.tool_card("s1", "c1").unwrap().instance.id, "card");
    assert!(host.tool_card("s1", "c2").is_none());
    assert!(host.tool_card("s2", "c1").is_none());
}

#[test]
fn unregistered_or_foreign_providers_are_rejected() {
    let mut host = AppletHost::new();
    assert!(mount(&mut host, "x", json!({"kind": "panel"}), "session").is_err());
    register(&mut host, &[]);
    mount(&mut host, "x", json!({"kind": "panel"}), "session").unwrap();
    // Another provider cannot patch, move or close someone else's instance.
    let hijack = host.apply(
        "evil",
        ProviderMessage::Close {
            instance: "x".into(),
        },
    );
    assert!(hijack.is_err());
    assert!(host.instance("x").is_some());
    let spoof = host.apply(
        "evil",
        ProviderMessage::Register {
            manifest: host.manifest("demo").unwrap().clone(),
        },
    );
    assert!(spoof.is_err());
}

#[test]
fn bad_patches_keep_the_last_good_document_and_request_resync() {
    let mut host = AppletHost::new();
    register(&mut host, &[]);
    mount(&mut host, "x", json!({"kind": "panel"}), "session").unwrap();
    host.drain_outbox();

    host.apply(
        "demo",
        ProviderMessage::Patch {
            instance: "x".into(),
            base_revision: 1,
            ops: vec![PatchOp::Replace {
                path: "/view/children/0/text".into(),
                value: json!("world"),
            }],
            assets: vec![],
        },
    )
    .unwrap();
    assert_eq!(host.instance("x").unwrap().instance.document.revision, 2);

    // Stale revision: rejected, resync requested, document unchanged.
    let stale = host.apply(
        "demo",
        ProviderMessage::Patch {
            instance: "x".into(),
            base_revision: 1,
            ops: vec![PatchOp::Replace {
                path: "/view/children/0/text".into(),
                value: json!("stale"),
            }],
            assets: vec![],
        },
    );
    assert!(stale.is_err());
    let outbox = host.drain_outbox();
    assert_eq!(outbox[0].0, "demo");
    assert!(matches!(
        outbox[0].1,
        HostMessage::Resync { revision: 2, .. }
    ));
    let mounted = host.instance("x").unwrap();
    assert_eq!(mounted.instance.document.revision, 2);
    assert!(mounted.last_error.is_some());

    // Valid shape but capability violation: rejected after patching.
    let escalate = host.apply(
        "demo",
        ProviderMessage::Patch {
            instance: "x".into(),
            base_revision: 2,
            ops: vec![PatchOp::Replace {
                path: "/view/children/1/on_press".into(),
                value: json!({"action": "host.open_url", "args": {"url": "https://x.dev"}}),
            }],
            assets: vec![],
        },
    );
    assert!(escalate.is_err());
    assert_eq!(host.instance("x").unwrap().instance.document.revision, 2);
}

#[test]
fn actions_route_to_provider_with_state_and_host_actions_need_capabilities() {
    let mut host = AppletHost::new();
    register(&mut host, &[Capability::OpenUrl]);
    mount(&mut host, "x", json!({"kind": "panel"}), "session").unwrap();
    assert!(host.set_state("x", "q", json!("cats")));
    assert!(!host.set_state("x", "q", json!("cats")));

    let effect = host
        .dispatch("x", &Action::new("go"), Some("btn".into()))
        .unwrap();
    let Effect::Provider(HostMessage::Action {
        state,
        revision,
        source_key,
        ..
    }) = effect
    else {
        panic!("expected provider action");
    };
    assert_eq!(state["q"], "cats");
    assert_eq!(revision, 1);
    assert_eq!(source_key.as_deref(), Some("btn"));

    let open = Action {
        action: "host.open_url".into(),
        args: json!({"url": "https://x.dev"}),
    };
    assert_eq!(
        host.dispatch("x", &open, None),
        Some(Effect::OpenUrl("https://x.dev".into()))
    );
    let js = Action {
        action: "host.open_url".into(),
        args: json!({"url": "javascript:x"}),
    };
    assert_eq!(host.dispatch("x", &js, None), None);
    let copy = Action {
        action: "host.copy".into(),
        args: json!({"text": "x"}),
    };
    assert_eq!(
        host.dispatch("x", &copy, None),
        None,
        "clipboard not declared"
    );
    let local = Action {
        action: "host.set_state".into(),
        args: json!({"key": "tab", "value": "b"}),
    };
    assert_eq!(host.dispatch("x", &local, None), Some(Effect::Rerender));
    assert_eq!(host.state("x", "tab"), Some(&json!("b")));
    assert_eq!(
        host.dispatch("x", &Action::new("host.close"), None),
        Some(Effect::Closed)
    );
    assert!(host.instance("x").is_none());
    assert!(
        host.drain_outbox()
            .iter()
            .any(|(_, m)| matches!(m, HostMessage::Closed { .. }))
    );
}

#[test]
fn snapshots_respect_lifetimes_and_revalidate_on_restore() {
    let mut host = AppletHost::new();
    register(&mut host, &[]);
    mount(&mut host, "eph", json!({"kind": "sidebar"}), "ephemeral").unwrap();
    mount(&mut host, "ses", json!({"kind": "sidebar"}), "session").unwrap();
    mount(&mut host, "per", json!({"kind": "sidebar"}), "persistent").unwrap();

    let reload = host.snapshot(false);
    assert_eq!(reload.instances.len(), 2);
    let restart = host.snapshot(true);
    assert_eq!(restart.instances.len(), 1);

    let mut tampered = reload.clone();
    tampered.instances[0].instance.document.view = serde_json::from_value(json!({
        "type": "button", "label": "x", "on_press": {"action": "host.open_file", "args": {"path": "/etc/passwd"}}
    }))
    .unwrap();
    let json = serde_json::to_string(&tampered).unwrap();
    let mut restored = AppletHost::new();
    assert_eq!(restored.restore(serde_json::from_str(&json).unwrap()), 1);
    assert_eq!(restored.instances().count(), 1);

    host.provider_disconnected("demo");
    assert!(host.instance("eph").is_none());
    assert!(host.instance("per").is_some());
}

#[test]
fn asset_cache_sniffs_bytes_and_rejects_mislabeled_images() {
    use base64::Engine;
    let png = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/previews/image-preview.png"
    ))
    .unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(&png);
    let mut cache = AssetCache::default();
    let first = cache.image("image/png", &data).unwrap();
    let second = cache.image("image/png", &data).unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "decoded once by content"
    );
    let mut fresh = AssetCache::default();
    assert!(
        fresh.image("image/jpeg", &data).is_none(),
        "label must match bytes"
    );
    assert!(fresh.image("image/png", "bm90IGFuIGltYWdl").is_none());
    assert!(
        fresh
            .data_uri(&format!("data:image/png;base64,{data}"))
            .is_some()
    );
    let svg = base64::engine::general_purpose::STANDARD
        .encode("<svg xmlns='http://www.w3.org/2000/svg'/>");
    assert!(fresh.image("image/svg+xml", &svg).is_some());
}
