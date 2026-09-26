//! Applet host: the GPUI-free core that owns registered applets and mounted
//! instances. It validates every document before it can render, applies
//! revisioned patches atomically, routes actions (host-handled versus provider),
//! decodes image assets once by content hash, and snapshots instances for hot
//! reload. Rendering lives in `applet_view`, placement in the panel and
//! workspace.
//!
//! Instances are independent of tool calls: a tool card is one placement
//! (`Inline` anchored to a call id) among panels, sidebar sections, overlays,
//! composer strips, and inline cards anchored to messages or the transcript end.
use jcode_applet_types::{
    Capability, HostMessage, Instance, Limits, Manifest, Placement, ProviderMessage, apply_patch,
    manifest::{Launcher, Trigger},
    placement::{Anchor, Lifetime, Scope},
    validate::{validate_document, validate_manifest},
    view::{Action, host_action},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The user's capability decisions, keyed by applet id. An applet may use a
/// capability only when its manifest declares it AND the user granted it.
/// Recorded once per applet and revocable from settings.
pub(crate) type Grants = BTreeMap<String, BTreeSet<Capability>>;

/// Human wording for a capability in consent prompts and settings.
pub(crate) fn capability_label(capability: Capability) -> &'static str {
    match capability {
        Capability::OpenUrl => "open links",
        Capability::Clipboard => "copy to the clipboard",
        Capability::StartChat => "start new chats",
        Capability::SendPrompt => "send prompts to sessions",
        Capability::ReadFiles => "read local files",
        Capability::RemoteImages => "load remote images",
        Capability::Notifications => "show notifications",
        Capability::Html => "run sandboxed web content",
    }
}

/// What the host must do after the core accepts an action.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Forward to the provider.
    Provider(HostMessage),
    OpenUrl(String),
    Copy(String),
    StartChat {
        prompt: String,
    },
    SendPrompt {
        session_id: String,
        prompt: String,
    },
    OpenFile(String),
    /// Local state changed. Re-render only.
    Rerender,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Mounted {
    pub instance: Instance,
    /// Provider signalled work in progress for the latest action.
    #[serde(default)]
    pub busy: bool,
    /// Last rejected message, shown instead of silently dropping it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Persisted instances. The runtime is an App global, so Ctrl+R keeps it
/// without a snapshot. This is the format for `persistent` instances across
/// app restarts, written to `~/.jcode/applets/instances.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct HostSnapshot {
    pub manifests: Vec<Manifest>,
    pub instances: Vec<Mounted>,
}

#[derive(Default)]
pub(crate) struct AppletHost {
    manifests: BTreeMap<String, Manifest>,
    instances: BTreeMap<String, Mounted>,
    limits: Limits,
    /// Messages for providers, drained by the transport.
    outbox: Vec<(String, HostMessage)>,
    /// Built-in applets (showcase, agent) whose capabilities need no consent.
    trusted: BTreeSet<String>,
    /// Capability decisions. An applet absent here has not been asked yet.
    grants: Grants,
}

impl AppletHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn manifest(&self, applet: &str) -> Option<&Manifest> {
        self.manifests.get(applet)
    }

    pub fn manifests(&self) -> impl Iterator<Item = &Manifest> {
        self.manifests.values()
    }

    /// Treat an applet as built in: its declared capabilities need no consent.
    pub fn trust(&mut self, applet: &str) {
        self.trusted.insert(applet.to_owned());
    }

    pub fn grants(&self) -> &Grants {
        &self.grants
    }

    pub fn set_grants(&mut self, grants: Grants) {
        self.grants = grants;
    }

    /// Record the user's decision. Denying records an empty set, so the
    /// prompt is not shown again until the applet asks for something new.
    pub fn decide(&mut self, applet: &str, granted: BTreeSet<Capability>) {
        self.grants.insert(applet.to_owned(), granted);
    }

    /// Forget every decision for an applet, revoking its capabilities.
    pub fn revoke(&mut self, applet: &str) {
        self.grants.remove(applet);
    }

    /// Whether an applet may use a capability right now.
    pub fn allowed(&self, applet: &str, capability: Capability) -> bool {
        let Some(manifest) = self.manifests.get(applet) else {
            return false;
        };
        manifest.allows(capability)
            && (self.trusted.contains(applet)
                || self
                    .grants
                    .get(applet)
                    .is_some_and(|granted| granted.contains(&capability)))
    }

    /// Effective capabilities of an applet, for the renderer.
    pub fn effective(&self, applet: &str) -> Vec<Capability> {
        self.manifests
            .get(applet)
            .map(|manifest| {
                manifest
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|capability| self.allowed(applet, *capability))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Applets that declared capabilities the user has not decided on yet,
    /// with the undecided capabilities.
    pub fn pending_consent(&self) -> Vec<(Manifest, Vec<Capability>)> {
        self.manifests
            .values()
            .filter(|manifest| !self.trusted.contains(&manifest.id))
            .filter_map(|manifest| {
                let decided = self.grants.get(&manifest.id);
                let asked: Vec<Capability> = manifest
                    .capabilities
                    .iter()
                    .copied()
                    .filter(|capability| {
                        decided.is_none_or(|decided| !decided.contains(capability))
                    })
                    .collect();
                // An applet that was already asked is only prompted again for
                // capabilities it newly requests after the decision.
                let fresh = match decided {
                    None => asked,
                    Some(_) => Vec::new(),
                };
                (!fresh.is_empty()).then(|| (manifest.clone(), fresh))
            })
            .collect()
    }

    /// Every launcher across registered applets: `(applet, index, launcher)`.
    pub fn launchers(&self) -> Vec<(String, usize, Launcher)> {
        self.manifests
            .values()
            .flat_map(|manifest| {
                manifest
                    .launchers
                    .iter()
                    .enumerate()
                    .map(|(index, launcher)| (manifest.id.clone(), index, launcher.clone()))
            })
            .collect()
    }

    /// Fire a launcher. A singleton launcher whose instance is still mounted
    /// returns that instance for the caller to focus. Otherwise the provider
    /// is asked to mount `instance` and `None` is returned.
    pub fn launch(&mut self, applet: &str, index: usize) -> Option<String> {
        let launcher = self.manifests.get(applet)?.launchers.get(index)?.clone();
        let instance = format!("{applet}#launch{index}");
        if launcher.singleton && self.instances.contains_key(&instance) {
            return Some(instance);
        }
        self.outbox.push((
            applet.to_owned(),
            HostMessage::Launch {
                applet: applet.to_owned(),
                launcher: index,
                instance,
            },
        ));
        None
    }

    /// Launchers that mount automatically when their provider registers.
    pub fn startup_launchers(&self, applet: &str) -> Vec<usize> {
        self.manifests
            .get(applet)
            .map(|manifest| {
                manifest
                    .launchers
                    .iter()
                    .enumerate()
                    .filter(|(_, launcher)| matches!(launcher.trigger, Trigger::Startup))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Applets whose manifest claims this tool call as a card.
    pub fn tool_card_claims(&self, tool: &str, action: Option<&str>) -> Vec<String> {
        self.manifests
            .values()
            .filter(|manifest| {
                manifest
                    .tool_cards
                    .iter()
                    .any(|claim| claim.matches(tool, action))
            })
            .map(|manifest| manifest.id.clone())
            .collect()
    }

    /// Tell claiming providers that a tool call started or finished.
    pub fn notify_tool_call(&mut self, message: HostMessage) -> usize {
        let HostMessage::ToolCall { tool, input, .. } = &message else {
            return 0;
        };
        let action = input.get("action").and_then(Value::as_str);
        let claims = self.tool_card_claims(tool, action);
        for applet in &claims {
            self.outbox.push((applet.clone(), message.clone()));
        }
        claims.len()
    }

    /// Mount or replace an instance on behalf of a provider the host itself
    /// speaks for (the agent), without echoing anything back to it.
    pub fn upsert(&mut self, instance: Instance) -> Result<(), String> {
        let manifest = self
            .manifests
            .get(&instance.applet)
            .ok_or_else(|| format!("applet {} is not registered", instance.applet))?;
        validate_document(&instance.document, manifest, &self.limits)
            .map_err(|error| error.to_string())?;
        let busy = self
            .instances
            .get(&instance.id)
            .is_some_and(|mounted| mounted.busy);
        self.instances.insert(
            instance.id.clone(),
            Mounted {
                instance,
                busy,
                last_error: None,
            },
        );
        Ok(())
    }

    /// Remove an instance without notifying its provider, because the
    /// provider is the one that removed it.
    pub fn remove_silently(&mut self, instance: &str) -> bool {
        self.instances.remove(instance).is_some()
    }

    /// Visible inline, composer and sidebar instances for a session.
    pub fn inline_for<'a>(&'a self, session_id: &'a str) -> impl Iterator<Item = &'a Mounted> + 'a {
        self.instances.values().filter(move |mounted| {
            matches!(&mounted.instance.placement,
                Placement::Inline { session_id: s, .. } if s == session_id)
        })
    }

    pub fn composer_for<'a>(
        &'a self,
        session_id: &'a str,
    ) -> impl Iterator<Item = &'a Mounted> + 'a {
        self.instances.values().filter(move |mounted| {
            matches!(&mounted.instance.placement,
                Placement::Composer { session_id: s } if s == session_id)
        })
    }

    pub fn instance(&self, id: &str) -> Option<&Mounted> {
        self.instances.get(id)
    }

    pub fn instances(&self) -> impl Iterator<Item = &Mounted> {
        self.instances.values()
    }

    /// Instances at a placement kind, in stable id order.
    pub fn at<'a>(
        &'a self,
        filter: impl Fn(&Placement) -> bool + 'a,
    ) -> impl Iterator<Item = &'a Mounted> + 'a {
        self.instances
            .values()
            .filter(move |mounted| filter(&mounted.instance.placement))
    }

    /// The inline instance that replaces a tool call's generic row.
    pub fn tool_card(&self, session_id: &str, call_id: &str) -> Option<&Mounted> {
        self.instances.values().find(|mounted| {
            matches!(
                &mounted.instance.placement,
                Placement::Inline { session_id: s, anchor: Anchor::ToolCall { call_id: c } }
                    if s == session_id && c == call_id
            )
        })
    }

    /// Pending messages, each addressed to the applet (provider) it concerns.
    pub fn drain_outbox(&mut self) -> Vec<(String, HostMessage)> {
        std::mem::take(&mut self.outbox)
    }

    fn reject(
        &mut self,
        applet: &str,
        instance: Option<&str>,
        reason: String,
    ) -> Result<(), String> {
        if let Some(id) = instance
            && let Some(mounted) = self.instances.get_mut(id)
        {
            mounted.last_error = Some(reason.clone());
        }
        self.outbox.push((
            applet.to_owned(),
            HostMessage::Rejected {
                instance: instance.map(str::to_owned),
                reason: reason.clone(),
            },
        ));
        Err(reason)
    }

    /// Apply one provider message. Invalid messages never change what renders:
    /// they are recorded on the instance and reported to the provider.
    pub fn apply(&mut self, applet: &str, message: ProviderMessage) -> Result<(), String> {
        match message {
            ProviderMessage::Register { manifest } => {
                if manifest.id != applet {
                    return self.reject(
                        applet,
                        None,
                        format!(
                            "manifest id {} does not match provider {applet}",
                            manifest.id
                        ),
                    );
                }
                if let Err(error) = validate_manifest(&manifest) {
                    return self.reject(applet, None, error.to_string());
                }
                self.manifests.insert(manifest.id.clone(), manifest);
                Ok(())
            }
            ProviderMessage::Mount {
                instance,
                placement,
                scope,
                lifetime,
                document,
            } => {
                let Some(manifest) = self.manifests.get(applet) else {
                    return self.reject(
                        applet,
                        Some(&instance),
                        format!("applet {applet} is not registered"),
                    );
                };
                if let Some(existing) = self.instances.get(&instance)
                    && existing.instance.applet != applet
                {
                    return self.reject(
                        applet,
                        None,
                        format!("instance {instance} belongs to another applet"),
                    );
                }
                if let Err(error) = validate_document(&document, manifest, &self.limits) {
                    return self.reject(applet, Some(&instance), error.to_string());
                }
                self.instances.insert(
                    instance.clone(),
                    Mounted {
                        instance: Instance {
                            id: instance,
                            applet: applet.to_owned(),
                            placement,
                            scope,
                            lifetime,
                            document: *document,
                        },
                        busy: false,
                        last_error: None,
                    },
                );
                Ok(())
            }
            ProviderMessage::Patch {
                instance,
                base_revision,
                ops,
                assets,
            } => {
                let (manifest, mounted) = match self.owned(applet, &instance) {
                    Ok(pair) => pair,
                    Err(reason) => return self.reject(applet, None, reason),
                };
                let mut next = match apply_patch(&mounted.instance.document, base_revision, &ops) {
                    Ok(next) => next,
                    Err(error) => {
                        let revision = mounted.instance.document.revision;
                        self.outbox.push((
                            applet.to_owned(),
                            HostMessage::Resync {
                                instance: instance.clone(),
                                revision,
                                reason: error.to_string(),
                            },
                        ));
                        return self.reject(applet, Some(&instance), error.to_string());
                    }
                };
                for asset in assets {
                    next.assets.retain(|existing| existing.id != asset.id);
                    next.assets.push(asset);
                }
                if let Err(error) = validate_document(&next, manifest, &self.limits) {
                    return self.reject(applet, Some(&instance), error.to_string());
                }
                let mounted = self.instances.get_mut(&instance).expect("owned instance");
                mounted.instance.document = next;
                mounted.last_error = None;
                Ok(())
            }
            ProviderMessage::Move {
                instance,
                placement,
            } => {
                if let Err(reason) = self.owned(applet, &instance) {
                    return self.reject(applet, None, reason);
                }
                self.instances
                    .get_mut(&instance)
                    .expect("owned")
                    .instance
                    .placement = placement;
                Ok(())
            }
            ProviderMessage::Busy { instance, busy } => {
                if let Err(reason) = self.owned(applet, &instance) {
                    return self.reject(applet, None, reason);
                }
                self.instances.get_mut(&instance).expect("owned").busy = busy;
                Ok(())
            }
            ProviderMessage::Toast { instance, .. } => match self.owned(applet, &instance) {
                Ok(_) => Ok(()),
                Err(reason) => self.reject(applet, None, reason),
            },
            ProviderMessage::Close { instance } => {
                if let Err(reason) = self.owned(applet, &instance) {
                    return self.reject(applet, None, reason);
                }
                self.instances.remove(&instance);
                Ok(())
            }
        }
    }

    fn owned(&self, applet: &str, instance: &str) -> Result<(&Manifest, &Mounted), String> {
        let mounted = self
            .instances
            .get(instance)
            .ok_or_else(|| format!("unknown instance {instance}"))?;
        if mounted.instance.applet != applet {
            return Err(format!("instance {instance} belongs to another applet"));
        }
        let manifest = self
            .manifests
            .get(applet)
            .ok_or_else(|| format!("applet {applet} is not registered"))?;
        Ok((manifest, mounted))
    }

    /// Update a bound state key from a control (input, toggle, select, tabs).
    /// Local state is optimistic and travels with the next action.
    pub fn set_state(&mut self, instance: &str, key: &str, value: Value) -> bool {
        let Some(mounted) = self.instances.get_mut(instance) else {
            return false;
        };
        let Value::Object(state) = &mut mounted.instance.document.state else {
            return false;
        };
        if state.get(key) == Some(&value) {
            return false;
        }
        state.insert(key.to_owned(), value);
        true
    }

    // Placement queries used by transcript and sidebar hosts.
    #[allow(dead_code)]
    pub fn state(&self, instance: &str, key: &str) -> Option<&Value> {
        self.instances
            .get(instance)?
            .instance
            .document
            .state
            .get(key)
    }

    /// Route a user action. Host actions are re-checked against the manifest
    /// at dispatch time, not only at validation, so capability revocation is
    /// immediate.
    pub fn dispatch(
        &mut self,
        instance: &str,
        action: &Action,
        source_key: Option<String>,
    ) -> Option<Effect> {
        let mounted = self.instances.get(instance)?;
        let manifest = self.manifests.get(&mounted.instance.applet)?;
        let arg = |name: &str| {
            action
                .args
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        use jcode_applet_types::Capability as C;
        let _ = manifest;
        let applet = mounted.instance.applet.clone();
        let allowed = |capability| self.allowed(&applet, capability);
        let effect = match action.action.as_str() {
            host_action::OPEN_URL if allowed(C::OpenUrl) => {
                let url = arg("url");
                (url.starts_with("https://")
                    || url.starts_with("http://")
                    || url.starts_with("mailto:"))
                .then_some(Effect::OpenUrl(url))?
            }
            host_action::COPY if allowed(C::Clipboard) => Effect::Copy(arg("text")),
            host_action::START_CHAT if allowed(C::StartChat) => Effect::StartChat {
                prompt: arg("prompt"),
            },
            host_action::SEND_PROMPT if allowed(C::SendPrompt) => Effect::SendPrompt {
                session_id: arg("session_id"),
                prompt: arg("prompt"),
            },
            host_action::OPEN_FILE if allowed(C::ReadFiles) => Effect::OpenFile(arg("path")),
            host_action::CLOSE => {
                self.close(instance);
                Effect::Closed
            }
            host_action::SET_STATE => {
                let key = arg("key");
                let value = action.args.get("value").cloned().unwrap_or(Value::Null);
                self.set_state(instance, &key, value);
                Effect::Rerender
            }
            name if name.starts_with("host.") => return None,
            _ => {
                let message = HostMessage::Action {
                    instance: instance.to_owned(),
                    revision: mounted.instance.document.revision,
                    action: action.clone(),
                    state: mounted.instance.document.state.clone(),
                    source_key,
                };
                self.outbox
                    .push((mounted.instance.applet.clone(), message.clone()));
                Effect::Provider(message)
            }
        };
        Some(effect)
    }

    /// Record a transport-level problem on an instance without changing its
    /// last good document.
    pub fn mark_error(&mut self, instance: &str, error: String) {
        if let Some(mounted) = self.instances.get_mut(instance) {
            mounted.last_error = Some(error);
            mounted.busy = false;
        }
    }

    pub fn close(&mut self, instance: &str) {
        if let Some(mounted) = self.instances.remove(instance) {
            self.outbox.push((
                mounted.instance.applet,
                HostMessage::Closed {
                    instance: instance.to_owned(),
                },
            ));
        }
    }

    /// Drop ephemeral instances whose provider went away.
    pub fn provider_disconnected(&mut self, applet: &str) {
        self.instances.retain(|_, mounted| {
            mounted.instance.applet != applet || mounted.instance.lifetime != Lifetime::Ephemeral
        });
    }

    /// Whether an instance is visible for the active workspace and session.
    pub fn in_scope(mounted: &Mounted, dir: Option<&str>, session: Option<&str>) -> bool {
        match &mounted.instance.scope {
            Scope::Global => true,
            Scope::Workspace { dir: d } => dir == Some(d.as_str()),
            Scope::Session { session_id } => session == Some(session_id.as_str()),
        }
    }

    /// Snapshot for hot reload (everything except ephemeral) or restart
    /// (persistent only).
    pub fn snapshot(&self, restart: bool) -> HostSnapshot {
        HostSnapshot {
            manifests: self.manifests.values().cloned().collect(),
            instances: self
                .instances
                .values()
                .filter(|mounted| match mounted.instance.lifetime {
                    Lifetime::Ephemeral => false,
                    Lifetime::Session => !restart,
                    Lifetime::Persistent => true,
                })
                .cloned()
                .collect(),
        }
    }

    /// Restore a snapshot, revalidating everything: a snapshot written by an
    /// older UI generation must not bypass validation.
    pub fn restore(&mut self, snapshot: HostSnapshot) -> usize {
        let mut dropped = 0;
        for manifest in snapshot.manifests {
            if validate_manifest(&manifest).is_ok() {
                self.manifests.insert(manifest.id.clone(), manifest);
            }
        }
        for mounted in snapshot.instances {
            let valid = self
                .manifests
                .get(&mounted.instance.applet)
                .is_some_and(|manifest| {
                    validate_document(&mounted.instance.document, manifest, &self.limits).is_ok()
                });
            if valid {
                self.instances.insert(mounted.instance.id.clone(), mounted);
            } else {
                dropped += 1;
            }
        }
        dropped
    }
}

/// Decoded image assets, shared by all instances and keyed by content hash so
/// identical bytes decode once and patches never re-decode unchanged images.
#[derive(Default)]
pub(crate) struct AssetCache {
    by_hash: HashMap<[u8; 32], std::sync::Arc<gpui::Image>>,
}

impl AssetCache {
    pub fn image(&mut self, mime: &str, base64_data: &str) -> Option<std::sync::Arc<gpui::Image>> {
        use base64::Engine;
        use sha2::Digest;
        let hash: [u8; 32] = sha2::Sha256::digest(base64_data.as_bytes()).into();
        if let Some(image) = self.by_hash.get(&hash) {
            return Some(image.clone());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64_data.trim())
            .ok()?;
        let format = sniff(&bytes)?;
        // The declared type must match the bytes: never trust the label.
        if mime_of(format) != mime {
            return None;
        }
        let image = crate::image_cache::encoded(format, bytes);
        self.by_hash.insert(hash, image.clone());
        Some(image)
    }

    pub fn data_uri(&mut self, uri: &str) -> Option<std::sync::Arc<gpui::Image>> {
        let (mime, payload) = uri.strip_prefix("data:")?.split_once(";base64,")?;
        self.image(mime, payload)
    }
}

fn sniff(bytes: &[u8]) -> Option<gpui::ImageFormat> {
    use gpui::ImageFormat as F;
    Some(match bytes {
        [0x89, b'P', b'N', b'G', ..] => F::Png,
        [0xff, 0xd8, 0xff, ..] => F::Jpeg,
        [b'G', b'I', b'F', b'8', ..] => F::Gif,
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => F::Webp,
        _ => {
            let head = std::str::from_utf8(&bytes[..bytes.len().min(512)]).ok()?;
            if head.contains("<svg") {
                F::Svg
            } else {
                return None;
            }
        }
    })
}

fn mime_of(format: gpui::ImageFormat) -> &'static str {
    use gpui::ImageFormat as F;
    match format {
        F::Png => "image/png",
        F::Jpeg => "image/jpeg",
        F::Gif => "image/gif",
        F::Webp => "image/webp",
        F::Svg => "image/svg+xml",
        _ => "",
    }
}

#[cfg(test)]
#[path = "applet_host_tests.rs"]
mod tests;
