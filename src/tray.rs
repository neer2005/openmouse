//! Windows system tray icon with a right-click context menu.
//!
//! The icon runs its own Win32 message loop on a dedicated thread, fully
//! independent of the tokio runtime. Right-clicking the icon pops up a menu
//! that currently holds one item: a "Start with Windows" checkbox backed by
//! the HKCU `Software\Microsoft\Windows\CurrentVersion\Run` registry key.
//!
//! On non-Windows platforms this module compiles to a no-op stub.

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Registry::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// Custom window message the tray icon uses to report mouse events.
    const WM_TRAYICON: u32 = WM_APP + 1;
    const IDM_AUTOSTART: usize = 1001;
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "OpenMouse";
    const ICON_PX: i32 = 64;

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
                        let cmd = format!("\"{}\"", exe.display());
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

    unsafe fn show_context_menu(hwnd: HWND) {
        let hmenu = match CreatePopupMenu() {
            Ok(m) => m,
            Err(_) => return,
        };
        let text = wide("Start with Windows");
        let checked = autostart_enabled();
        let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(hmenu, flags, IDM_AUTOSTART, PCWSTR(text.as_ptr()));

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
        if cmd.0 as usize == IDM_AUTOSTART {
            set_autostart(!checked);
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
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn run(tx: mpsc::Sender<isize>) {
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

    pub(crate) fn spawn() -> super::TrayHandle {
        let (tx, rx) = mpsc::channel::<isize>();
        let thread = thread::spawn(move || run(tx));
        // If the tray thread died before creating its window, recv fails and
        // we degrade gracefully: the server still runs, just without an icon.
        let hwnd_raw = rx.recv().unwrap_or(0);
        if hwnd_raw == 0 {
            eprintln!("[openmouse] tray icon unavailable; continuing without it");
        }
        super::TrayHandle {
            #[cfg(windows)]
            inner: Some(Inner {
                hwnd: HWND(hwnd_raw as *mut std::ffi::c_void),
                thread: Some(thread),
            }),
        }
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
pub fn spawn() -> TrayHandle {
    imp::spawn()
}

#[cfg(not(windows))]
pub fn spawn() -> TrayHandle {
    TrayHandle {}
}
