use super::*;
use windows_sys::Win32::{
    Foundation::RECT,
    UI::WindowsAndMessaging::{
        GetWindowRect, IsWindowVisible, SW_SHOW, SendMessageW, WS_OVERLAPPED,
    },
};

#[test]
fn panel_handle_stays_a_visible_child_across_focus_and_geometry_changes() {
    unsafe {
        let owner = CreateWindowExW(
            0,
            windows_sys::w!("STATIC"),
            windows_sys::w!(""),
            WS_OVERLAPPED,
            10,
            20,
            900,
            700,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        assert!(!owner.is_null());
        let handle = create_panel_handle(owner, 0);
        assert!(!handle.is_null());
        ShowWindow(
            owner,
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
        );
        let mut results = Vec::new();
        for (x, scale) in [(400.0, 1.0), (300.0, 1.5), (500.0, 2.0)] {
            let area = Area {
                x,
                y: 40.0,
                width: 7.0,
                height: 500.0,
            };
            position_panel_handle(handle, area, scale);
            // Keyboard focus enters/leaves a WebView child; the handle must
            // remain a child and retain its hit target without activation.
            SendMessageW(
                owner,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_KILLFOCUS,
                0,
                0,
            );
            let mut rect = RECT::default();
            GetWindowRect(handle, &mut rect);
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(owner, &mut origin);
            results.push((
                GetParent(handle) == owner,
                IsWindowVisible(handle) != 0,
                rect.left - origin.x,
                rect.top - origin.y,
                SendMessageW(
                    handle,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_NCHITTEST,
                    0,
                    0,
                ),
            ));
        }
        ShowWindow(owner, SW_HIDE);
        let hidden_with_owner = IsWindowVisible(handle) == 0;
        ShowWindow(owner, SW_SHOW);
        let restored_with_owner = IsWindowVisible(handle) != 0;
        DestroyWindow(owner);
        assert!(hidden_with_owner && restored_with_owner);
        for ((child, visible, x, y, hit), expected) in
            results.into_iter().zip([(400, 40), (450, 60), (1000, 80)])
        {
            assert!(child && visible);
            assert_eq!((x, y), expected);
            assert_eq!(
                hit,
                windows_sys::Win32::UI::WindowsAndMessaging::HTCLIENT as isize
            );
        }
    }
}
