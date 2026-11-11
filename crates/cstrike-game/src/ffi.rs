//! Win32 and COM calls: the system mouse parameters client.dll sets itself (`SystemParametersInfoA`,
//! `SPI_GETMOUSE` / `SPI_SETMOUSE`), the foreground window, and the MOTD page's WebView2 lockdown.

#![allow(unsafe_code)]

/// `SPI_GETMOUSE`: the two acceleration thresholds and the acceleration level.
#[cfg(windows)]
pub fn mouse_params() -> Option<[i32; 3]> {
    let mut params = [0i32; 3];
    // SAFETY: SPI_GETMOUSE writes three ints through pvParam, which points to `params`.
    let ok = unsafe { SystemParametersInfoA(SPI_GETMOUSE, 0, params.as_mut_ptr().cast(), 0) };
    (ok != 0).then_some(params)
}

/// `SPI_SETMOUSE` with `fWinIni` 0: for the session only, not written to the user profile.
#[cfg(windows)]
pub fn set_mouse_params(params: [i32; 3]) -> bool {
    let mut params = params;
    // SAFETY: SPI_SETMOUSE reads three ints through pvParam, which points to `params`.
    unsafe { SystemParametersInfoA(SPI_SETMOUSE, 0, params.as_mut_ptr().cast(), 0) != 0 }
}

#[cfg(not(windows))]
pub fn mouse_params() -> Option<[i32; 3]> {
    None
}

#[cfg(not(windows))]
pub fn set_mouse_params(_params: [i32; 3]) -> bool {
    false
}

#[cfg(windows)]
const SPI_GETMOUSE: u32 = 3;
#[cfg(windows)]
const SPI_SETMOUSE: u32 = 4;

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn SystemParametersInfoA(action: u32, param: u32, pv_param: *mut core::ffi::c_void, win_ini: u32) -> i32;
    fn GetForegroundWindow() -> isize;
}

/// Whether `window` is the foreground window. Keyboard focus moving into a child (the MOTD
/// page) leaves it there; switching to another application does not.
#[cfg(windows)]
pub fn is_foreground(window: &winit::window::Window) -> bool {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = window.window_handle() else { return false };
    let RawWindowHandle::Win32(h) = handle.as_raw() else { return false };
    // SAFETY: GetForegroundWindow takes nothing and returns a window handle or null.
    unsafe { GetForegroundWindow() == h.hwnd.get() }
}

#[cfg(not(windows))]
pub fn is_foreground(_window: &winit::window::Window) -> bool {
    false
}

/// The MOTD page's WebView2 settings and handlers wry does not cover: no script bridge, nothing
/// dropped in from outside, external protocols (`steam://`, `ms-settings:`, …), login prompts
/// and certificate error pages cancelled, and frames navigating only where `allowed` lets them
/// (wry's handler sees the main frame).
#[cfg(windows)]
pub fn lock_down_webview(
    webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    allowed: fn(&str) -> bool,
) -> windows_core::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL, ICoreWebView2_10, ICoreWebView2_14, ICoreWebView2_18, ICoreWebView2Controller4,
    };
    use webview2_com::{
        BasicAuthenticationRequestedEventHandler, LaunchingExternalUriSchemeEventHandler, NavigationStartingEventHandler, ServerCertificateErrorDetectedEventHandler,
    };
    use windows_core::Interface;
    let mut token = 0i64;
    // SAFETY: calls on live WebView2 objects from the thread that created them; each handler
    // gets its event's arguments for the duration of the call, and the URI string it is handed
    // is taken (and freed) once.
    unsafe {
        let settings = webview.Settings()?;
        settings.SetAreHostObjectsAllowed(false)?;
        settings.SetIsWebMessageEnabled(false)?;
        controller.cast::<ICoreWebView2Controller4>()?.SetAllowExternalDrop(false)?;
        webview.cast::<ICoreWebView2_18>()?.add_LaunchingExternalUriScheme(
            &LaunchingExternalUriSchemeEventHandler::create(Box::new(|_, args| args.map_or(Ok(()), |a| a.SetCancel(true)))),
            &mut token,
        )?;
        webview.cast::<ICoreWebView2_10>()?.add_BasicAuthenticationRequested(
            &BasicAuthenticationRequestedEventHandler::create(Box::new(|_, args| args.map_or(Ok(()), |a| a.SetCancel(true)))),
            &mut token,
        )?;
        webview.cast::<ICoreWebView2_14>()?.add_ServerCertificateErrorDetected(
            &ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| args.map_or(Ok(()), |a| a.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)))),
            &mut token,
        )?;
        webview.add_FrameNavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = windows_core::PWSTR::null();
                args.Uri(&mut uri)?;
                if allowed(&webview2_com::take_pwstr(uri)) { Ok(()) } else { args.SetCancel(true) }
            })),
            &mut token,
        )?;
    }
    Ok(())
}
