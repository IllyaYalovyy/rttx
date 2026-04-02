pub mod handle;
#[doc(hidden)]
pub mod links;
pub mod persistent_widget;
pub mod widget;

fn normalized_shortcut_modifiers(modifiers: gtk4::gdk::ModifierType) -> gtk4::gdk::ModifierType {
    let shortcut_mask = gtk4::gdk::ModifierType::SHIFT_MASK
        | gtk4::gdk::ModifierType::CONTROL_MASK
        | gtk4::gdk::ModifierType::ALT_MASK
        | gtk4::gdk::ModifierType::SUPER_MASK
        | gtk4::gdk::ModifierType::HYPER_MASK
        | gtk4::gdk::ModifierType::META_MASK;

    modifiers & shortcut_mask
}
