//! Local recovery and cached account usage actions.
use super::*;

impl Panel {
    pub(super) fn handle_recovery_command(
        &mut self,
        content: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match content.trim() {
            "/fix" => {
                self.bridge.send(Command::RefreshRuntime {
                    session_id: self.session_id.clone(),
                });
                crate::accounts::request_refresh();
                self.open_recovery_models(cx);
                self.items.push(Item::Assistant("Refreshing runtime and account status. Choose an available model in the native recovery picker to continue. Your conversation and draft are preserved.".into()));
            }
            "/usage" => {
                crate::accounts::request_refresh();
                let accounts = cx
                    .try_global::<usage::StatusAccounts>()
                    .map(|state| state.0.clone())
                    .unwrap_or_default();
                let mut text = String::from(
                    "## Connected provider usage\n\nLatest cached provider reports. A fresh account refresh has been requested. These limits are per login, not combined.\n\n",
                );
                for account in accounts
                    .iter()
                    .filter(|account| account.status == "available")
                {
                    text.push_str(&format!("### {}\n\n", account.display_name));
                    if account.usage_reports.is_empty() {
                        append_limits(&mut text, &account.limits);
                    } else {
                        for report in &account.usage_reports {
                            text.push_str(&format!(
                                "**{}**\n\n",
                                report
                                    .account_label
                                    .as_deref()
                                    .unwrap_or(&report.provider_name)
                            ));
                            append_limits(&mut text, &report.limits);
                            for (name, value) in &report.extra_info {
                                text.push_str(&format!("- {name}: {value}\n"));
                            }
                        }
                    }
                }
                if accounts.iter().all(|account| account.status != "available") {
                    text.push_str("No connected provider usage is available yet. Open `/accounts` to sign in or inspect account status.\n");
                }
                self.items.push(Item::Assistant(text));
            }
            _ => return false,
        }
        self.transcript_measurements.dirty = true;
        cx.notify();
        true
    }
}

fn append_limits(text: &mut String, limits: &[crate::accounts::UsageLimit]) {
    if limits.is_empty() {
        text.push_str("No usage limits reported by this provider.\n\n");
    }
    for limit in limits {
        let used = if limit.usage_percent.is_finite() {
            format!("{:.0}% used", limit.usage_percent.clamp(0., 100.))
        } else {
            "usage not reported".into()
        };
        let reset = limit
            .reset_in
            .as_deref()
            .map(|value| format!(", resets in {value}"))
            .unwrap_or_default();
        text.push_str(&format!("- {}: {used}{reset}\n", limit.name));
    }
}
