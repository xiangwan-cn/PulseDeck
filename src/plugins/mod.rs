pub mod loader;
pub mod manifest;
pub mod one_shot;
pub mod persistent;
pub mod protocol;
pub mod supervisor;

#[cfg(feature = "scrcpy-forge")]
pub mod scrcpy_forge;

pub fn build_page(
    handle: tokio::runtime::Handle,
    page: &crate::core::config::PageConfig,
) -> Option<gtk::Widget> {
    #[cfg(feature = "scrcpy-forge")]
    if scrcpy_forge::accepts(page.kind.as_deref()) {
        use gtk::prelude::Cast;
        return Some(scrcpy_forge::build(handle, page).upcast());
    }

    let _ = (handle, page);
    None
}

pub fn is_optional_page(kind: Option<&str>) -> bool {
    matches!(kind, Some("scrcpy-forge"))
}
