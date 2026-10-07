//! 桌面霸權術 🏆:一鍵把其他 App 視窗最小化,只留霓璃獨佔桌面;可一鍵復原。
//!
//! - clear():列舉所有「可見、有標題、非工具視窗、非霓璃自己」的頂層視窗,最小化並記住。
//! - restore():把剛剛收起來的那批視窗還原回去。
//! - 由 agent 的 clear_desktop / restore_desktop 工具呼叫。

use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// 記住被霸權術最小化的視窗 handle(供復原)。
pub struct HegemonyState(pub Mutex<Vec<isize>>);

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongW, GetWindowTextLengthW, IsIconic, IsWindowVisible, ShowWindow,
        GWL_EXSTYLE, SW_MINIMIZE, SW_RESTORE, WS_EX_TOOLWINDOW,
    };

    struct Ctx {
        skip: isize,
        out: Vec<isize>,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut Ctx);
        let h = hwnd as isize;
        let is_tool = (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW) != 0;
        // 跳過:她自己、隱藏、已最小化、沒標題、工具視窗
        if h == ctx.skip
            || IsWindowVisible(hwnd) == 0
            || IsIconic(hwnd) != 0
            || GetWindowTextLengthW(hwnd) == 0
            || is_tool
        {
            return 1; // 繼續列舉
        }
        ctx.out.push(h);
        1
    }

    /// 最小化除了 skip 之外的所有「真實 App 視窗」,回傳被收起來的清單。
    pub fn minimize_others(skip: isize) -> Vec<isize> {
        let mut ctx = Ctx { skip, out: Vec::new() };
        unsafe {
            EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM);
            for &h in &ctx.out {
                ShowWindow(h as HWND, SW_MINIMIZE);
            }
        }
        ctx.out
    }

    pub fn restore_all(list: &[isize]) {
        unsafe {
            for &h in list {
                ShowWindow(h as HWND, SW_RESTORE);
            }
        }
    }
}

#[cfg(windows)]
fn main_hwnd(app: &AppHandle) -> isize {
    app.get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as isize)
        .unwrap_or(0)
}

/// 清空桌面:最小化其他所有視窗,霓璃置中獨佔。
pub fn clear(app: &AppHandle) -> String {
    #[cfg(windows)]
    {
        let skip = main_hwnd(app);
        let collected = win::minimize_others(skip);
        let n = collected.len();
        *app.state::<HegemonyState>().0.lock().unwrap() = collected;
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.center();
            let _ = w.set_focus();
        }
        if n == 0 {
            "桌面本來就很乾淨嘛~ 那我就大方地站中間了 👑".into()
        } else {
            format!("桌面霸權術發動!我把 {n} 個視窗通通收起來了,現在整個桌面都是我的~ 👑✨")
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        "桌面霸權術只在 Windows 上有效啦~".into()
    }
}

/// 復原:把剛剛收起來的視窗全部叫回來。
pub fn restore(app: &AppHandle) -> String {
    #[cfg(windows)]
    {
        let list = std::mem::take(&mut *app.state::<HegemonyState>().0.lock().unwrap());
        let n = list.len();
        win::restore_all(&list);
        if n == 0 {
            "桌面本來就沒被我清空喔~(沒東西要還原)".into()
        } else {
            format!("好啦好啦~ 我把 {n} 個視窗都放回原位了 😌")
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        "桌面霸權術只在 Windows 上有效啦~".into()
    }
}
