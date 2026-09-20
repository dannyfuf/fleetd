use fleet_ui_kit::text_input;
use gpui::KeyBinding;

/// The editor's own rows, shared with `fleet-lazygit` and the kit's tests.
///
/// The gallery is the kit's acceptance test, so it must bind exactly what a real host binds and
/// nothing more: every container key the surface around an editor owns is bound by the gallery
/// itself, next to its own actions.
pub fn bindings() -> Vec<KeyBinding> {
    text_input::default_bindings()
}
