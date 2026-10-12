//! Windows system tray icon with a right-click context menu.
//!
//! The icon runs its own Win32 message loop on a dedicated thread, fully
//! independent of the tokio runtime. Right-clicking the icon pops up a menu:
//!
//! - "Start with Windows" checkbox, backed by the HKCU
//!   `Software\Microsoft\Windows\CurrentVersion\Run` registry key
//! - "Copy pairing URL", for when the console (and its QR code) is hidden
//! - "Exit", which asks the main thread to shut everything down
//!
//! On non-Windows platforms this module compiles to a no-op stub.

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::ffi::OsStr;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    use tokio::sync::oneshot;

    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{
        GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Registry::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// Custom window message the tray icon uses to report mouse events.
    const WM_TRAYICON: u32 = WM_APP + 1;
    const IDM_AUTOSTART: usize = 1001;
    const IDM_COPY_URL: usize = 1002;
    const IDM_EXIT: usize = 1003;
    const IDM_SHOW_QR: usize = 1004;
    /// CF_UNICODETEXT clipboard format id.
    const CF_UNICODETEXT: u32 = 13;
    /// Target size for the QR popup bitmap (square, pixels).
    const QR_PX: u32 = 384;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "OpenMouse";
    const ICON_PX: i32 = 64;

    /// State owned by the tray thread; the pairing URL is fixed for the run
    /// while the exit sender is consumed the first time Exit is clicked.
    struct TrayState {
        pair_url: String,
        exit_tx: Option<oneshot::Sender<()>>,
        qr_hwnd: Option<HWND>,
    }

    thread_local! {
        static TRAY_STATE: RefCell<Option<TrayState>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn in_rounded_rect(x: i32, y: i32, x0: i32, y0: i32, x1: i32, y1: i32, r: i32) -> bool {
        if x < x0 || x >= x1 || y < y0 || y >= y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= r * r
    }

    /// Rasterize a 64x64 mouse glyph: white mouse on a blue disc.
    /// Pixels are 0xAARRGGBB with a real alpha channel.
    fn draw_icon() -> Vec<u32> {
        const BLUE: u32 = 0xFF2E7CF6;
        const WHITE: u32 = 0xFFFFFFFF;
        let mut px = vec![0u32; (ICON_PX * ICON_PX) as usize];
        for y in 0..ICON_PX {
            for x in 0..ICON_PX {
                let dx = x - ICON_PX / 2;
                let dy = y - ICON_PX / 2;
                let mut c = 0u32;
                if dx * dx + dy * dy <= 28 * 28 {
                    c = BLUE;
                }
                // Mouse body.
                if in_rounded_rect(x, y, 24, 16, 40, 48, 8) {
                    c = WHITE;
                }
                // Scroll wheel cut out of the body.
                if in_rounded_rect(x, y, 30, 21, 34, 29, 2) {
                    c = BLUE;
                }
                px[(y * ICON_PX + x) as usize] = c;
            }
        }
        px
    }

    /// Build an HICON from the rasterized glyph via a 32bpp DIB section.
    unsafe fn create_icon() -> HICON {
        let px = draw_icon();
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = ICON_PX;
        bi.bmiHeader.biHeight = -ICON_PX; // top-down DIB
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut std::ffi::c_void = null_mut();
        let hbm_color = match CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(h) => h,
            Err(_) => return HICON(null_mut()),
        };
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        // Monochrome mask; the color bitmap's alpha channel carries transparency.
        let hbm_mask = CreateBitmap(ICON_PX, ICON_PX, 1, 1, None);
        let ii = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let hicon = CreateIconIndirect(&ii).unwrap_or(HICON(null_mut()));
        // The icon holds its own copies; free the source bitmaps.
        let _ = DeleteObject(hbm_color.into());
        let _ = DeleteObject(hbm_mask.into());
        hicon
    }

    fn autostart_enabled() -> bool {
        unsafe {
            let subkey = wide(RUN_KEY);
            let mut hkey = HKEY(null_mut());
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                KEY_READ,
                &mut hkey,
            )
            .is_err()
            {
                return false;
            }
            let name = wide(VALUE_NAME);
            let exists =
                RegQueryValueExW(hkey, PCWSTR(name.as_ptr()), None, None, None, None).is_ok();
            let _ = RegCloseKey(hkey);
            exists
        }
    }

    fn set_autostart(enabled: bool) {
        unsafe {
            let subkey = wide(RUN_KEY);
            let mut hkey = HKEY(null_mut());
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                KEY_SET_VALUE,
                &mut hkey,
            )
            .is_err()
            {
                eprintln!("[openmouse] tray: could not open Run key");
                return;
            }
            let name = wide(VALUE_NAME);
            if enabled {
                match std::env::current_exe() {
                    Ok(exe) => {
                        // Quote the path so directories with spaces survive.
                        // --tray keeps boot launches silent (no console popup).
                        let cmd = format!("\"{}\" --tray", exe.display());
                        let data = wide(&cmd);
                        let bytes: &[u8] = std::slice::from_raw_parts(
                            data.as_ptr() as *const u8,
                            data.len() * 2,
                        );
                        if RegSetValueExW(hkey, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes))
                            .is_err()
                        {
                            eprintln!("[openmouse] tray: could not write autostart value");
                        }
                    }
                    Err(e) => eprintln!("[openmouse] tray: current_exe failed: {e}"),
                }
            } else {
                // Deleting a missing value just means "already off".
                let _ = RegDeleteValueW(hkey, PCWSTR(name.as_ptr()));
            }
            let _ = RegCloseKey(hkey);
        }
    }

    /// Copy text to the Windows clipboard (CF_UNICODETEXT).
    unsafe fn copy_text_to_clipboard(text: &str) {
        let wide: Vec<u16> = OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect();
        if OpenClipboard(Some(HWND(null_mut()))).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let bytes = wide.len() * 2;
        if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let ptr = GlobalLock(hmem);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
                let _ = GlobalUnlock(hmem);
                // On success the system owns the memory; on failure free it.
                if SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hmem.0))).is_err() {
                    let _ = GlobalFree(Some(hmem));
                }
            } else {
                let _ = GlobalFree(Some(hmem));
            }
        }
        let _ = CloseClipboard();
    }

    /// Render the pairing URL as a QR code into a 32bpp DIB bitmap.
    /// Returns the bitmap and its side length in pixels (always square).
    unsafe fn qr_bitmap(pair_url: &str) -> Option<(HBITMAP, i32)> {
        use image::Luma;
        use qrcode::QrCode;
        let code = QrCode::new(pair_url.as_bytes()).ok()?;
        let img = code
            .render::<Luma<u8>>()
            .max_dimensions(QR_PX, QR_PX)
            .quiet_zone(true)
            .build();
        let side = img.width() as i32;
        let raw = img.into_raw();
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = side;
        bi.bmiHeader.biHeight = -side; // top-down DIB
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut std::ffi::c_void = null_mut();
        let hbm = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u32, raw.len());
        for (d, g) in dst.iter_mut().zip(raw.iter()) {
            *d = if *g > 128 { 0xFF_FF_FF_FF } else { 0xFF_00_00_00 };
        }
        Some((hbm, side))
    }

    struct QrWindowData {
        hbm: HBITMAP,
        side: i32,
    }

    /// Create (or raise) the centered, topmost QR popup window.
    /// Runs on the tray thread, which owns the message loop.
    unsafe fn create_qr_window(pair_url: &str) -> Option<HWND> {
        let (hbm, side) = match qr_bitmap(pair_url) {
            Some(b) => b,
            None => {
                eprintln!("[openmouse] tray: QR render failed");
                return None;
            }
        };
        let instance: HINSTANCE = GetModuleHandleW(PCWSTR::null())
            .map(|h| h.into())
            .unwrap_or(HINSTANCE(null_mut()));
        let class_name = wide("OpenMouseQrWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(qr_wnd_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..zeroed()
        };
        // Tolerate the class already existing from an earlier popup.
        let _ = RegisterClassW(&wc);

        let pad = 24i32;
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU;
        let client = side + pad * 2;
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: client,
            bottom: client,
        };
        let _ = AdjustWindowRect(&mut rc, style, false.into());
        let win_w = rc.right - rc.left;
        let win_h = rc.bottom - rc.top;
        let x = (GetSystemMetrics(SM_CXSCREEN) - win_w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - win_h) / 2;

        let title = wide("OpenMouse \u{2014} scan to pair");
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            style | WS_VISIBLE,
            x,
            y,
            win_w,
            win_h,
            None,
            None,
            Some(instance),
            None,
        )
        .ok()?;
        let data = Box::new(QrWindowData { hbm, side });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
        let _ = SetForegroundWindow(hwnd);
        Some(hwnd)
    }

    unsafe extern "system" fn qr_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut QrWindowData;
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                if !hdc.0.is_null() {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rc);
                    let white = GetStockObject(WHITE_BRUSH);
                    if !white.0.is_null() {
                        let _ = FillRect(hdc, &rc, HBRUSH(white.0));
                    }
                    if !data.is_null() {
                        let d = &*data;
                        let memdc = CreateCompatibleDC(Some(hdc));
                        if !memdc.0.is_null() {
                            let old = SelectObject(memdc, d.hbm.into());
                            let x = (rc.right - rc.left - d.side) / 2;
                            let y = (rc.bottom - rc.top - d.side) / 2;
                            let _ =
                                BitBlt(hdc, x, y, d.side, d.side, Some(memdc), 0, 0, SRCCOPY);
                            SelectObject(memdc, old);
                            let _ = DeleteDC(memdc);
                        }
                    }
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut QrWindowData;
                if !data.is_null() {
                    let d = Box::from_raw(data);
                    let _ = DeleteObject(d.hbm.into());
                }
                TRAY_STATE.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut() {
                        st.qr_hwnd = None;
                    }
                });
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn show_context_menu(hwnd: HWND) {
        let hmenu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        let autostart_text = wide("Start with Windows");
        let checked = autostart_enabled();
        let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(hmenu, flags, IDM_AUTOSTART, PCWSTR(autostart_text.as_ptr()));
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
        let qr_text = wide("Show QR code");
        let _ = AppendMenuW(hmenu, MF_STRING, IDM_SHOW_QR, PCWSTR(qr_text.as_ptr()));
        let copy_text = wide("Copy pairing URL");
        let _ = AppendMenuW(hmenu, MF_STRING, IDM_COPY_URL, PCWSTR(copy_text.as_ptr()));
        let exit_text = wide("Exit");
        let _ = AppendMenuW(hmenu, MF_STRING, IDM_EXIT, PCWSTR(exit_text.as_ptr()));

        // Snapshot what the click handlers need; the borrow must not be held
        // across TrackPopupMenu's modal loop.
        let pair_url = TRAY_STATE.with(|s| {
            s.borrow()
                .as_ref()
                .map(|st| st.pair_url.clone())
                .unwrap_or_default()
        });

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu dismisses correctly when clicking away.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(hmenu);

        // With TPM_RETURNCMD the return value carries the chosen item id.
        match cmd.0 as usize {
            IDM_AUTOSTART => set_autostart(!checked),
            IDM_SHOW_QR => {
                let existing =
                    TRAY_STATE.with(|s| s.borrow().as_ref().and_then(|st| st.qr_hwnd));
                match existing {
                    Some(qr_hwnd) => {
                        let _ = SetForegroundWindow(qr_hwnd);
                    }
                    None => {
                        if let Some(qr_hwnd) = create_qr_window(&pair_url) {
                            TRAY_STATE.with(|s| {
                                if let Some(st) = s.borrow_mut().as_mut() {
                                    st.qr_hwnd = Some(qr_hwnd);
                                }
                            });
                        }
                    }
                }
            }
            IDM_COPY_URL => copy_text_to_clipboard(&pair_url),
            IDM_EXIT => TRAY_STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    if let Some(tx) = st.exit_tx.take() {
                        let _ = tx.send(());
                    }
                }
            }),
            _ => {}
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TRAYICON => {
                if lparam.0 as u32 == WM_RBUTTONUP {
                    show_context_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                // Close any open QR popup before quitting the message loop.
                let qr_hwnd =
                    TRAY_STATE.with(|s| s.borrow().as_ref().and_then(|st| st.qr_hwnd));
                if let Some(qr_hwnd) = qr_hwnd {
                    let _ = DestroyWindow(qr_hwnd);
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn run(tx: mpsc::Sender<isize>, pair_url: String, exit_tx: oneshot::Sender<()>) {
        TRAY_STATE.with(|s| {
            *s.borrow_mut() = Some(TrayState {
                pair_url,
                exit_tx: Some(exit_tx),
                qr_hwnd: None,
            });
        });
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(PCWSTR::null())
                .map(|h| h.into())
                .unwrap_or(HINSTANCE(null_mut()));
            let class_name = wide("OpenMouseTrayWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                let _ = tx.send(0);
                return;
            }
            let title = wide("OpenMouse tray");
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            ) {
                Ok(h) => h,
                Err(_) => {
                    let _ = tx.send(0);
                    return;
                }
            };
            // Hand the window handle back so the main thread can shut us down.
            let _ = tx.send(hwnd.0 as isize);

            let hicon = create_icon();
            let tip = wide("OpenMouse");
            let mut nid: NOTIFYICONDATAW = zeroed();
            nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAYICON;
            nid.hIcon = hicon;
            for (i, c) in tip.iter().take(127).enumerate() {
                nid.szTip[i] = *c;
            }
            if !hicon.0.is_null() {
                let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            }

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            if !hicon.0.is_null() {
                let _ = DestroyIcon(hicon);
            }
        }
    }

    pub(crate) struct Inner {
        hwnd: HWND,
        thread: Option<JoinHandle<()>>,
    }

    impl Inner {
        pub fn shutdown(mut self) {
            unsafe {
                // Nudge the tray thread's message loop to exit via its window.
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// Hide this process's console window (used for `--tray` launches).
    pub(crate) fn hide_console() {
        unsafe {
            let hwnd = GetConsoleWindow();
            if !hwnd.0.is_null() {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    pub(crate) fn spawn(pair_url: String) -> (super::TrayHandle, oneshot::Receiver<()>) {
        let (hwnd_tx, hwnd_rx) = mpsc::channel::<isize>();
        let (exit_tx, exit_rx) = oneshot::channel::<()>();
        let thread = thread::spawn(move || run(hwnd_tx, pair_url, exit_tx));
        // If the tray thread died before creating its window, recv fails and
        // we degrade gracefully: the server still runs, just without an icon.
        let hwnd_raw = hwnd_rx.recv().unwrap_or(0);
        if hwnd_raw == 0 {
            eprintln!("[openmouse] tray icon unavailable; continuing without it");
        }
        (
            super::TrayHandle {
                inner: Some(Inner {
                    hwnd: HWND(hwnd_raw as *mut std::ffi::c_void),
                    thread: Some(thread),
                }),
            },
            exit_rx,
        )
    }
}

pub struct TrayHandle {
    #[cfg(windows)]
    inner: Option<imp::Inner>,
}

impl TrayHandle {
    pub fn shutdown(self) {
        #[cfg(windows)]
        if let Some(inner) = self.inner {
            inner.shutdown();
        }
    }
}

#[cfg(windows)]
pub fn spawn(pair_url: String) -> (TrayHandle, tokio::sync::oneshot::Receiver<()>) {
    imp::spawn(pair_url)
}

#[cfg(windows)]
pub fn hide_console() {
    imp::hide_console()
}

#[cfg(not(windows))]
pub fn spawn(_pair_url: String) -> (TrayHandle, tokio::sync::oneshot::Receiver<()>) {
    let (_tx, rx) = tokio::sync::oneshot::channel();
    (TrayHandle {}, rx)
}

#[cfg(not(windows))]
pub fn hide_console() {}
