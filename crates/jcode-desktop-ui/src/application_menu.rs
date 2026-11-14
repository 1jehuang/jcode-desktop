//! Native application menus. Call `init` after the application's key bindings
//! are installed so menu shortcut labels reflect the existing workspace map.

use gpui::App;
#[cfg(any(target_os = "macos", test))]
use gpui::{Menu, MenuItem, OsAction, SystemMenuType};

#[cfg(any(target_os = "macos", test))]
use crate::{input, workspace};

#[cfg(any(target_os = "macos", test))]
use crate::ui_actions::{About, CheckForUpdates, Hide, HideOthers, OpenSettings, ShowAll};

/// Current-generation implementations behind listeners installed only once.
/// GPUI app globals survive UI reload, so replacing these pointers prevents
/// retained listeners from invoking stale UI code or showing duplicate prompts.
#[cfg(any(target_os = "macos", test))]
struct MenuCallbacks {
    check_for_updates: fn(&mut App),
    about: fn(&mut App),
    hide: fn(&mut App),
    hide_others: fn(&mut App),
    show_all: fn(&mut App),
}

#[cfg(any(target_os = "macos", test))]
impl gpui::Global for MenuCallbacks {}

#[cfg(any(target_os = "macos", test))]
fn show_about(cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let detail = format!(
        "Jcode Desktop {}\nRevision {}\n{}",
        crate::build_info::version(),
        crate::build_info::revision(),
        crate::build_info::age(),
    );
    let _ = window.update(cx, |_, window, cx| {
        let _response = window.prompt(
            gpui::PromptLevel::Info,
            "About Jcode",
            Some(&detail),
            &["OK"],
            cx,
        );
    });
}

/// Install the native macOS menu without changing other platforms' shortcuts.
///
/// The workspace owns `OpenSettings`, New Session, and Quit. Global native
/// commands resolve their current-generation implementation on every dispatch.
pub fn init(cx: &mut App) {
    #[cfg(any(target_os = "macos", test))]
    {
        use gpui::KeyBinding;

        let already_registered = cx.try_global::<MenuCallbacks>().is_some();
        cx.set_global(MenuCallbacks {
            check_for_updates: |cx| {
                workspace::Workspace::check_and_apply_update();
                cx.refresh_windows();
            },
            about: show_about,
            hide: |cx| cx.hide(),
            hide_others: |cx| cx.hide_other_apps(),
            show_all: |cx| cx.unhide_other_apps(),
        });
        if !already_registered {
            cx.on_action(|_: &CheckForUpdates, cx| {
                let callback = cx.global::<MenuCallbacks>().check_for_updates;
                callback(cx);
            });
            cx.on_action(|_: &About, cx| {
                let callback = cx.global::<MenuCallbacks>().about;
                callback(cx);
            });
            cx.on_action(|_: &Hide, cx| {
                let callback = cx.global::<MenuCallbacks>().hide;
                callback(cx);
            });
            cx.on_action(|_: &HideOthers, cx| {
                let callback = cx.global::<MenuCallbacks>().hide_others;
                callback(cx);
            });
            cx.on_action(|_: &ShowAll, cx| {
                let callback = cx.global::<MenuCallbacks>().show_all;
                callback(cx);
            });
        }
        cx.bind_keys([
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
        ]);
        // Keep Jcode's existing navigation and closing conventions. Cmd-H
        // focuses left, Cmd-Q closes a panel, and Cmd-Shift-Q quits. Hide is
        // still available from the menu without stealing a workspace shortcut.
        cx.set_menus(menus());
    }
    #[cfg(not(any(target_os = "macos", test)))]
    let _ = cx;
}

#[cfg(any(target_os = "macos", test))]
fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Jcode").items([
            MenuItem::action("About Jcode", About),
            MenuItem::separator(),
            MenuItem::action("New Session", workspace::NewPanel),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Jcode", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Jcode", workspace::Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Action;

    #[derive(Default)]
    struct Invocations {
        checks: usize,
        about: usize,
    }
    impl gpui::Global for Invocations {}

    #[gpui::test]
    fn repeated_initialization_dispatches_once_and_uses_current_callbacks(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(Invocations::default());
            init(cx);
            cx.global_mut::<MenuCallbacks>().check_for_updates =
                |_| panic!("stale callback invoked");
            init(cx);
            cx.global_mut::<MenuCallbacks>().check_for_updates = |cx| {
                cx.global_mut::<Invocations>().checks += 1;
            };
            cx.global_mut::<MenuCallbacks>().about = |cx| {
                cx.global_mut::<Invocations>().about += 1;
            };
            cx.dispatch_action(&CheckForUpdates);
            cx.dispatch_action(&About);
            assert_eq!(cx.global::<Invocations>().checks, 1);
            assert_eq!(cx.global::<Invocations>().about, 1);
        });
    }

    #[test]
    fn application_menu_exposes_expected_commands_and_services() {
        let menus = menus();
        assert_eq!(menus[0].name.as_ref(), "Jcode");
        let labels: Vec<_> = menus[0]
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, .. } => Some(name.as_ref()),
                MenuItem::SystemMenu(menu) => Some(menu.name.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "About Jcode",
                "New Session",
                "Settings…",
                "Check for Updates…",
                "Services",
                "Hide Jcode",
                "Hide Others",
                "Show All",
                "Quit Jcode",
            ]
        );
        assert!(menus[0].items.iter().any(|item| matches!(
            item,
            MenuItem::SystemMenu(menu) if menu.menu_type == SystemMenuType::Services
        )));
        assert!(matches!(
            &menus[0].items[2],
            MenuItem::Action { action, .. } if action.partial_eq(&workspace::NewPanel)
        ));
        assert!(matches!(
            menus[0].items.last(),
            Some(MenuItem::Action { action, .. }) if action.partial_eq(&workspace::Quit)
        ));
    }

    #[test]
    fn edit_menu_uses_native_os_actions_and_existing_input_actions() {
        let menus = menus();
        let edit = &menus[1];
        assert_eq!(edit.name.as_ref(), "Edit");
        let actions: Vec<_> = edit
            .items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action {
                    action,
                    os_action: Some(os_action),
                    ..
                } => Some((action.as_ref(), *os_action)),
                _ => None,
            })
            .collect();
        let expected: [(Box<dyn Action>, OsAction); 6] = [
            (Box::new(input::Undo), OsAction::Undo),
            (Box::new(input::Redo), OsAction::Redo),
            (Box::new(input::Cut), OsAction::Cut),
            (Box::new(input::Copy), OsAction::Copy),
            (Box::new(input::Paste), OsAction::Paste),
            (Box::new(input::SelectAll), OsAction::SelectAll),
        ];
        assert_eq!(actions.len(), expected.len());
        for ((actual, os_action), (expected, expected_os_action)) in
            actions.iter().zip(expected.iter())
        {
            assert!(actual.partial_eq(expected.as_ref()));
            assert!(os_action == expected_os_action);
        }
    }
}
