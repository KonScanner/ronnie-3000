//! Native platform capability reporting.

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, WindowHandle,
};

/// Observable native window capabilities, without exposing host-specific handle values.
#[derive(Debug, Default)]
pub struct WindowProbe {
    /// Normalized native window backend name.
    pub backend: Option<&'static str>,
    /// Whether a native window handle was available.
    pub has_window_handle: bool,
    /// Whether a native display handle was available.
    pub has_display_handle: bool,
}

/// Borrowed native parent-window contract for a later embedded editor adapter.
pub struct EmbeddedParent<'a> {
    window: WindowHandle<'a>,
    display: DisplayHandle<'a>,
}

impl HasWindowHandle for EmbeddedParent<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(self.window)
    }
}

impl HasDisplayHandle for EmbeddedParent<'_> {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(self.display)
    }
}

/// Borrows native handles without taking ownership of the host window.
///
/// # Errors
///
/// Returns the platform handle error when either handle is unavailable.
pub fn embedded_parent<W>(parent: &W) -> Result<EmbeddedParent<'_>, HandleError>
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    Ok(EmbeddedParent {
        window: parent.window_handle()?,
        display: parent.display_handle()?,
    })
}

/// Creates a native file picker associated with the supplied parent window.
pub fn native_picker<W>(parent: &W) -> rfd::FileDialog
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    rfd::FileDialog::new()
        .set_title("Open a Ronnie model")
        .add_filter("Audio models", &["nam", "wav"])
        .set_parent(parent)
}

/// Reports native handles and the normalized backend for a window-like value.
#[must_use]
pub fn probe_window<W>(window: &W) -> WindowProbe
where
    W: HasWindowHandle + HasDisplayHandle + ?Sized,
{
    let raw_window = window.window_handle().ok().map(|handle| handle.as_raw());
    WindowProbe {
        backend: raw_window.as_ref().map(backend_name),
        has_window_handle: raw_window.is_some(),
        has_display_handle: window.display_handle().is_ok(),
    }
}

fn backend_name(handle: &RawWindowHandle) -> &'static str {
    match handle {
        RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_) => "x11",
        RawWindowHandle::Wayland(_) => "wayland",
        RawWindowHandle::Win32(_) => "windows",
        RawWindowHandle::AppKit(_) => "macos",
        _ => "other",
    }
}
