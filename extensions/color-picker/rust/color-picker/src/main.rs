use raycast_rust_macros::raycast;
use serde::Serialize;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::{
    core::w,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
        UI::Input::KeyboardAndMouse::*,
        UI::WindowsAndMessaging::*,
    },
};

static PICKER_RUNNING: AtomicBool = AtomicBool::new(false);

const ZOOM: i32 = 8;
const CAPTURE_SIZE: i32 = 19; // odd number for a center pixel
const LOUPE_SIZE: i32 = CAPTURE_SIZE * ZOOM; // 152px
const BORDER_W: i32 = 3;
const WINDOW_SIZE: i32 = LOUPE_SIZE + BORDER_W * 2; // 158px
const CROSSHAIR_HALF: i32 = ZOOM / 2;

static mut PICKED_COLOR: Option<(u8, u8, u8)> = None;
static mut CANCELLED: bool = false;

// Cached screen snapshot (captured with loupe hidden to avoid self-capture)
static mut SNAP_DC: HDC = unsafe { mem::zeroed() };
static mut SNAP_BMP: HBITMAP = unsafe { mem::zeroed() };
static mut SNAP_OLD: HGDIOBJ = unsafe { mem::zeroed() };
static mut SNAP_PIXEL: COLORREF = COLORREF(0);

/// Window procedure for the magnifier loupe overlay.
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);

                // Capture screen snapshot around cursor into cached DC
                // (loupe is excluded from capture via WDA_EXCLUDEFROMCAPTURE)
                let hscreen_dc = GetDC(None);
                let _ = BitBlt(
                    SNAP_DC, 0, 0,
                    CAPTURE_SIZE, CAPTURE_SIZE,
                    Some(hscreen_dc),
                    pt.x - CAPTURE_SIZE / 2,
                    pt.y - CAPTURE_SIZE / 2,
                    SRCCOPY,
                );
                SNAP_PIXEL = GetPixel(hscreen_dc, pt.x, pt.y);
                ReleaseDC(None, hscreen_dc);

                // Center the loupe on the cursor (no clamping — let it go off-screen)
                let half = WINDOW_SIZE / 2;
                let lx = pt.x - half;
                let ly = pt.y - half;

                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    lx,
                    ly,
                    WINDOW_SIZE,
                    WINDOW_SIZE,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);

                // Create memory DC for compositing
                let hmem_dc = CreateCompatibleDC(Some(hdc));
                let hbmp = CreateCompatibleBitmap(hdc, WINDOW_SIZE, WINDOW_SIZE);
                let hold = SelectObject(hmem_dc, hbmp.into());

                // Fill background with border color
                let border_brush = CreateSolidBrush(COLORREF(0x00444444));
                let bg_rect = RECT { left: 0, top: 0, right: WINDOW_SIZE, bottom: WINDOW_SIZE };
                let _ = FillRect(hmem_dc, &bg_rect, border_brush);
                let _ = DeleteObject(border_brush.into());

                // Clip magnified content to inner circle
                let inner_rgn = CreateEllipticRgn(BORDER_W, BORDER_W, BORDER_W + LOUPE_SIZE, BORDER_W + LOUPE_SIZE);
                SelectClipRgn(hmem_dc, Some(inner_rgn));

                // StretchBlt from cached snapshot (not live screen — avoids self-capture)
                let _ = SetStretchBltMode(hmem_dc, COLORONCOLOR);
                let _ = StretchBlt(
                    hmem_dc,
                    BORDER_W, BORDER_W,
                    LOUPE_SIZE, LOUPE_SIZE,
                    Some(SNAP_DC),
                    0, 0,
                    CAPTURE_SIZE, CAPTURE_SIZE,
                    SRCCOPY,
                );

                // Draw crosshair around the center pixel
                let center = WINDOW_SIZE / 2;
                let cross_left = center - CROSSHAIR_HALF;
                let cross_top = center - CROSSHAIR_HALF;

                // Outer dark rect (border of the selected pixel)
                let dark_pen = CreatePen(PS_SOLID, 2, COLORREF(0x00000000));
                let old_pen = SelectObject(hmem_dc, dark_pen.into());
                let null_brush = GetStockObject(NULL_BRUSH);
                let old_brush = SelectObject(hmem_dc, null_brush);
                let _ = Rectangle(hmem_dc, cross_left - 1, cross_top - 1, cross_left + ZOOM + 1, cross_top + ZOOM + 1);

                // Inner white rect
                let white_pen = CreatePen(PS_SOLID, 1, COLORREF(0x00FFFFFF));
                SelectObject(hmem_dc, white_pen.into());
                let _ = Rectangle(hmem_dc, cross_left, cross_top, cross_left + ZOOM, cross_top + ZOOM);

                SelectObject(hmem_dc, old_pen);
                SelectObject(hmem_dc, old_brush);
                let _ = DeleteObject(dark_pen.into());
                let _ = DeleteObject(white_pen.into());

                // Remove clip region so we can draw the border ring
                SelectClipRgn(hmem_dc, None);

                // Draw circular border ring
                let border_pen = CreatePen(PS_SOLID, BORDER_W, COLORREF(0x00444444));
                let old_pen2 = SelectObject(hmem_dc, border_pen.into());
                let null_brush2 = GetStockObject(NULL_BRUSH);
                let old_brush2 = SelectObject(hmem_dc, null_brush2);
                let _ = Ellipse(hmem_dc, BORDER_W / 2, BORDER_W / 2, WINDOW_SIZE - BORDER_W / 2, WINDOW_SIZE - BORDER_W / 2);
                SelectObject(hmem_dc, old_pen2);
                SelectObject(hmem_dc, old_brush2);
                let _ = DeleteObject(border_pen.into());

                // Blit composited result to window
                let _ = BitBlt(hdc, 0, 0, WINDOW_SIZE, WINDOW_SIZE, Some(hmem_dc), 0, 0, SRCCOPY);

                SelectObject(hmem_dc, hold);
                let _ = DeleteObject(hbmp.into());
                let _ = DeleteDC(hmem_dc);
                let _ = DeleteObject(inner_rgn.into());

                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                if msg == WM_LBUTTONDOWN {
                    // Use cached pixel color (not live GetPixel which would capture the loupe)
                    let pixel = SNAP_PIXEL;
                    let r = (pixel.0 & 0xFF) as u8;
                    let g = ((pixel.0 >> 8) & 0xFF) as u8;
                    let b = ((pixel.0 >> 16) & 0xFF) as u8;
                    PICKED_COLOR = Some((r, g, b));
                } else {
                    CANCELLED = true;
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let vk = wparam.0 as u32;
                if vk == VK_ESCAPE.0 as u32 {
                    CANCELLED = true;
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[derive(Serialize)]
struct Color {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
    #[serde(rename = "colorSpace")]
    color_space: String,
}

#[raycast]
fn pick_color() -> std::result::Result<Option<Color>, String> {
    unsafe {
        // Make process DPI-aware so coordinates match screen pixels
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // Prevent multiple instances
        if PICKER_RUNNING.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }

        PICKED_COLOR = None;
        CANCELLED = false;

        // Create snapshot DC for caching screen captures
        let hscreen_dc = GetDC(None);
        SNAP_DC = CreateCompatibleDC(Some(hscreen_dc));
        SNAP_BMP = CreateCompatibleBitmap(hscreen_dc, CAPTURE_SIZE, CAPTURE_SIZE);
        SNAP_OLD = SelectObject(SNAP_DC, SNAP_BMP.into());
        ReleaseDC(None, hscreen_dc);

        // Get initial cursor position to center loupe
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let half = WINDOW_SIZE / 2;
        let initial_x = pt.x - half;
        let initial_y = pt.y - half;

        // Register a layered window class for the loupe
        let class_name = w!("RaycastColorPickerLoupe");
        let hinstance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.to_string())?.into();

        let wc = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_CROSS).map_err(|e| e.to_string())?,
            lpszClassName: class_name,
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            return Err("Failed to register window class".to_string());
        }

        // Create a popup tool window centered on cursor (no taskbar button)
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class_name,
            w!(""),
            WS_POPUP,
            initial_x, initial_y,
            WINDOW_SIZE,
            WINDOW_SIZE,
            None,
            None,
            Some(hinstance),
            None,
        ).map_err(|e| e.to_string())?;

        // Make the window semi-opaque
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);

        // Set circular window region
        let rgn = CreateEllipticRgn(0, 0, WINDOW_SIZE, WINDOW_SIZE);
        SetWindowRgn(hwnd, Some(rgn), true);

        // Exclude loupe from screen capture so it doesn't capture itself
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

        // Create an invisible fullscreen input window to capture mouse & keyboard globally
        let input_class = w!("RaycastColorPickerInput");
        let input_wc = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_CROSS).map_err(|e| e.to_string())?,
            lpszClassName: input_class,
            ..Default::default()
        };
        RegisterClassExW(&input_wc);

        let screen_w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let screen_h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let screen_x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let screen_y = GetSystemMetrics(SM_YVIRTUALSCREEN);

        let input_hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            input_class,
            w!(""),
            WS_POPUP,
            screen_x, screen_y,
            screen_w, screen_h,
            None,
            None,
            Some(hinstance),
            None,
        ).map_err(|e| e.to_string())?;

        // Fully transparent input window
        let _ = SetLayeredWindowAttributes(input_hwnd, COLORREF(0), 1, LWA_ALPHA);

        let _ = ShowWindow(input_hwnd, SW_SHOWNOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        // Hide the real cursor
        let mut counter = ShowCursor(false);
        while counter >= 0 {
            counter = ShowCursor(false);
        }

        // Start a timer to update position ~60fps
        let _ = SetTimer(Some(hwnd), 1, 16, None);

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Cleanup
        let _ = KillTimer(Some(hwnd), 1);
        let _ = DestroyWindow(hwnd);
        let _ = DestroyWindow(input_hwnd);
        let _ = UnregisterClassW(class_name, Some(hinstance));
        let _ = UnregisterClassW(input_class, Some(hinstance));

        // Cleanup snapshot DC
        SelectObject(SNAP_DC, SNAP_OLD);
        let _ = DeleteObject(SNAP_BMP.into());
        let _ = DeleteDC(SNAP_DC);

        // Restore cursor
        counter = ShowCursor(true);
        while counter < 0 {
            counter = ShowCursor(true);
        }

        // Release the running guard
        PICKER_RUNNING.store(false, Ordering::SeqCst);

        if CANCELLED {
            return Ok(None);
        }

        match PICKED_COLOR {
            Some((r, g, b)) => Ok(Some(Color {
                red: r as f32 / 255.0,
                green: g as f32 / 255.0,
                blue: b as f32 / 255.0,
                alpha: 1.0,
                color_space: "sRGB".to_string(),
            })),
            None => Ok(None),
        }
    }
}
