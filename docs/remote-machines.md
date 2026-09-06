# Remote machine connections

Desktop's Machines picker connects normal native session panels through the shared
SDK and system OpenSSH. Selecting a default affects new session panels, not
existing panels or explicit local folder/terminal/help actions. Remote session
identities are namespaced and survive workspace snapshots.

## Implementation commits

- Desktop picker/routing/preferences: `165766c`.
- Shared SDK and `api --stdio`: Jcode `797481a264f7ae6039d1a4babea38d19919d8d2a`.

## Verification (2026-09-06)

- Remote suite: 29 passing tests across preferences, bounded SSH alias discovery,
  namespace validation, SDK socket-pair routing, create/fork/reconnect, native
  GPUI picker/shortcuts, draft preservation/retry, snapshots, and local filesystem
  isolation. The real project test executable was used.
- Local regression subsets: 38 harness tests passed (one existing ignored test),
  13 startup tests passed, and 9 configuration tests passed.
- Shared SDK: real localhost OpenSSH against a freshly built `jcode api --stdio`
  passed authentication, handshake, create, no-reply context persistence, list,
  disconnect/reattach, strict host-key rejection, shared daemon survival, and
  daemon EOF while stdin remains open. The shared API-server suite passed 84 tests.
- Native picker acceptance: `python3 scripts/accept-machines.py
  target/machines-acceptance` uses private Xvfb, isolated configuration/home, and
  an inert runtime. It exercises saved defaults, process restart, hostname entry,
  and switching back to this computer without touching a real remote machine.
  All five checks passed: picker controls, persisted remote default, restoration
  after process restart, typed host plus Enter, and switching back to local.
  Evidence: `target/machines-acceptance/report.json` and companion screenshots.
- `cargo build -p jcode-desktop -p jcode-desktop-ui` passed. The normal
  `--reload-ui` path activated the newly built UI generation in the running app.
  `target/machines-final-review.png` was captured and visually inspected.

The configured real `desktop` SSH target timed out, so it was not used as evidence
of a successful remote connection. No remote software was installed, no SSH trust
settings were changed, and no provider inference was used by the SSH acceptance.

The host editor explicitly supports mouse refocus. Its GPUI regression switches
away before clicking back and typing. Native automation initially clicked the
word SSH in the help paragraph rather than the field, then encountered ambiguous
placeholder OCR. Targeting the field relative to its Connect via SSH button fixed
the test, with ordinary global X11 keyboard input unchanged. Connection failure
and retry are above the machine list for smaller windows.

The final shared SDK rerun passed all 47 checks: 23 unit tests, 21 integration
tests, and 3 doctests. Additional root CLI tests timed out during compilation,
not test execution. Direct fresh-CLI flag/help checks and the native SSH workflow
were verified separately.

## Requirement-to-observation map

The two user requirements are easier remote connection and an optional persistent
remote default for new panels. The rows below also cover the new public controls
and their safety boundaries. `W` denotes `workspace::remote_tests`, `H` denotes
`harness::remote_tests`, `C` denotes `config::tests`, and `D` denotes
`remote_targets::tests`. Named tests are in the real Desktop crate, not copied
implementations. Socket-pair/recording-bridge checks are identified as such rather
than presented as live SSH evidence.

| Requirement or changed output | Concrete check | Observed result |
| --- | --- | --- |
| Discoverable Machines entry and one-click remote Connect | Native `accept-machines.py`; W `machines_picker_connect_default_shortcuts_and_local_override` | Picker opened from the sidebar. Selecting a host emitted a remote create with that host and no local working directory. |
| Settings entry opens the same usable picker | W `settings_machine_entry_opens_the_connectable_picker` | Passed: clicking Settings → Machines mounted the picker and usable host input. |
| SSH alias or `user@hostname`, Enter and Connect via SSH button | Native `accept-machines.py`; W `machines_input_validates_and_connects_without_changing_default` | Real native typing plus Enter persisted `builder@lab`. GPUI validates mouse refocus, Enter, invalid-host feedback and unchanged default. The separate Connect via SSH button assertion also passed and emitted the typed host. |
| Reuse SSH aliases and remember recent targets | D `discovers_aliases_and_bounded_recursive_includes_without_commands`; C `remote_preferences_default_local_and_normalize_safe_unique_targets`; native custom-host report | Aliases from included files are found without executing commands. Safe unique recent hosts are retained and custom host entry appears in saved preferences. |
| Set default affects future generic and pinned panels, not current sessions | W `machines_picker_connect_default_shortcuts_and_local_override` | Set default emitted no create and left the existing panel unchanged. Both Super+N and Ctrl+Alt+Enter emitted remote creates without forwarding a pinned local path. |
| Default persists across restart and can return to this computer | Native `accept-machines.py`; C `remote_preferences_persist_shared_and_standalone_and_switch_back_local` | Native restart restored workstation as default. Switching to local persisted an empty-string override. Both supported TOML layouts round-tripped. |
| One-off local Connect does not overwrite a remote default | W `machines_picker_connect_default_shortcuts_and_local_override` | Local Connect emitted a local create while the saved/default target remained remote. |
| Explicit folder and help actions stay local | W `explicit_folder_and_help_stay_local_with_a_remote_default` | Passed: choosing a local folder emitted a local create with that directory, and Help emitted a local help-draft create while preserving the remote default. Terminal creation remains the existing local `Panel::new_terminal` path, verified by source inspection rather than claimed as a new live SSH terminal test. |
| Remote startup works independently of local readiness and preserves draft | W `remote_startup_is_independent_of_local_runtime_and_promotes_draft` | Remote create occurred before local Connected. Local readiness did not duplicate it. The existing typed draft was sent to the promoted remote ID. |
| Connection status/error and explicit retry remain meaningful | W `remote_failure_survives_local_status_and_startup_retry_is_explicit`; H `remote_create_failure_is_visible_and_never_retries_a_startup_draft` | Local status did not overwrite the SSH error. Failure did not consume/retry the draft. Clicking Retry issued one correlated request and hid Retry while in flight. |
| SSH identities and badge survive restoration without confusing local sessions | W `remote_identity_survives_snapshot_and_local_files_are_not_shown`; `harness::remote::tests::{remote_ids_round_trip_without_collisions,malformed_remote_ids_never_fall_back_to_local}` | Namespaced IDs survived snapshots and decoding. Malformed remote IDs were rejected, never treated as local. Real native SSH badges were visually checked in restored-default.png. |
| Existing native session operations work with remote real IDs | H `remote_worker_routes_every_native_operation_and_namespaces_outputs`; H `remote_create_preserves_request_id_and_sends_working_dir_only_as_json` | Socket-pair protocol checks verified decoded request IDs, namespaced response/event IDs, native operations, request correlation and JSON-only directory transport. |
| Remote panels never read matching local files or become local favorites | H `remote_metadata_never_reads_matching_local_files`; W `remote_identity_survives_snapshot_and_local_files_are_not_shown`; W `remote_history_never_becomes_a_local_pinned_folder` | Matching local metadata was ignored, local Files content was excluded, and remote history did not become a local pinned directory. |
| Safe host handling and bounded discovery | D `rejects_options_urls_and_shell_syntax`, `validates_destinations_and_preserves_spelling`, `fifo_is_skipped_without_blocking`, `discovery_caps_hosts_and_include_depth`, `discovery_caps_file_attempts_and_never_emits_a_truncated_alias`; H `invalid_remote_create_host_never_starts_a_transport` | Invalid targets never launched SSH. Valid destinations retained their spelling. FIFOs were skipped and traversal/size/host limits held. |
| Preserve unrelated settings and refuse unsafe preference writes | C `invalid_remote_preferences_do_not_overwrite_config`; C `remote_preferences_preserve_inline_comments_and_replace_multiline_values` | Invalid input left configuration intact. Inline comments, unrelated fields and multiline arrays survived valid edits. |
| Closing picker restores composer keyboard focus | W `hiding_machines_returns_focus_to_the_existing_composer`; W input test | Hiding Machines and Escape returned focus to the existing editor. |
| SSH trust, startup timeout, diagnostics and process cleanup | SDK SSH tests `preserves_system_ssh_configuration_without_weakening_host_verification`, `handshake_timeout_is_bounded_even_without_request_timeout_and_reaps_child`, `ssh_stderr_is_reported_and_failed_child_is_reaped`, `final_clone_drop_kills_and_reaps_but_earlier_clone_drop_does_not`; H `last_bridge_handle_stops_its_coordinator_for_ssh_cleanup`; real SDK localhost test | Strict host verification rejected an untrusted host. Handshake timeout was bounded, stderr reached errors, and child processes were reaped at final ownership release. Hot-reload bridge shutdown also terminated its coordinator. |

### Real Desktop-to-SSH acceptance

`scripts/accept-real-ssh.py --cli /path/to/fresh/jcode target/real-ssh-acceptance`
runs the ordinary Desktop app, system OpenSSH and two separate native daemons.
It does not set screenshot fixture mode or replace the Desktop bridge. A private
Xvfb display, home/config/runtime, client and host keys, known-hosts file, and
loopback sshd isolate the check from the user's accounts and machines. The CLI
must support `api --stdio`. Dependencies include OpenSSH, Xvfb, Openbox, xdotool,
ImageMagick and Tesseract. Logs and report stay in the requested output directory.

The script checks live namespaced panel IDs, remote `/rename` persistence,
no-reply context, actual reattachment after app restart, default-driven panel
creation, picker Connect, mixed local/remote panels and both default choices.
Empty native sessions are intentionally provisional, so it persists a rename
before attaching a second client. No provider turn is requested. Initial test
failures were caught assertions: the OCR selector hit explanatory prose/local
Connect, and the state snapshot became available before the new frame rendered.
The selector now waits for actual rendered buttons and excludes prose.


Observed live result at 07:57 UTC: **passed**. Picker Connect created a fourth
real remote panel. Local Connect created a fifth panel on the separate local
daemon. Setting local as default made Super+N create the second local panel.
Setting the remote default in the UI made Super+N create the fifth remote panel.
The seven final IDs comprised five `ssh://jcode-test/...` IDs and two ordinary
local IDs, including both original remote IDs restored after process restart.
The remote names and no-reply context persisted, and diagnostics confirmed actual
SSH reattachment, not just restoration of saved panel placeholders. Evidence:
`target/real-ssh-acceptance/report.json`, `final-state.txt`,
`local-desktop-diagnostics.log`, and the accompanying native screenshots.

This closes the original usability and default-target feedback loop through the
actual application, SSH transport and native runtime. It does not establish
connectivity to the user's unavailable external `desktop` host. SSH prerequisites
and the distinction between new remote session panels and local terminal/file
browsing remain documented in the picker and README.


Final mapped regression run at 08:04 UTC: `CARGO_BUILD_JOBS=1 cargo test
-p jcode-desktop-ui --lib remote -- --test-threads=1` **passed all 31 tests**.
The named Settings, Connect-button, and explicit local-folder/help checks above
are included in `target/remote-mapped-tests.log`. An intermediate expanded input
test reached its new button assertion successfully but then tested the editor's
Escape handler after focus had moved to a button. Explicitly refocusing the host
editor before the editor-specific Escape check corrected the test setup. Earlier
attempts waited for the shared build lock or received SIGTERM under host memory
pressure. The final ordinary Cargo test command completed successfully.

Only tests, acceptance scripts and this evidence document changed during this
audit. The feature itself remains the previously built and hot-reloaded version.
