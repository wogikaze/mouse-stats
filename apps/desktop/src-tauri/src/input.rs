// WM_WTSSESSION_CHANGE events 1–8 and the suspend/resume WM_POWERBROADCAST
// codes that can interrupt Raw Input delivery. A power-status change is not one.
fn should_reset_held_state(message: u32, event: usize) -> bool {
    match message {
        0x02b1 => (1..=8).contains(&event),
        0x0218 => matches!(event, 4 | 6 | 7 | 18),
        _ => false,
    }
}
pub(crate) fn timer_release_code(scan: u16, extended: bool, virtual_key: u16) -> Option<u16> {
    crate::aggregate::normalize(scan, extended, false, virtual_key)
}

#[cfg(windows)]
mod win {
    use crate::{aggregate::normalize, model::now, Runtime};
    use std::{
        mem::size_of,
        sync::{Arc, OnceLock},
    };
    use windows::{
        core::w,
        Win32::{
            Foundation::*,
            System::{LibraryLoader::GetModuleHandleW, RemoteDesktop::*},
            UI::{
                Input::{KeyboardAndMouse::*, *},
                WindowsAndMessaging::*,
            },
        },
    };
    static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if let Some(runtime) = RUNTIME.get() {
            match msg {
                WM_INPUT => {
                    let mut raw = RAWINPUT::default();
                    let mut size = size_of::<RAWINPUT>() as u32;
                    let result = GetRawInputData(
                        HRAWINPUT(lp.0 as *mut _),
                        RID_INPUT,
                        Some((&mut raw as *mut RAWINPUT).cast()),
                        &mut size,
                        size_of::<RAWINPUTHEADER>() as u32,
                    );
                    if result == u32::MAX {
                        runtime.error("Raw Input の読み取りに失敗しました");
                    } else if result as usize >= size_of::<RAWINPUTHEADER>() {
                        if let Ok(mut agg) = runtime.aggregate.lock() {
                            let t = now();
                            if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
                                let k = raw.data.keyboard;
                                if let Some(code) = normalize(
                                    k.MakeCode,
                                    k.Flags & 2 != 0,
                                    k.Flags & 4 != 0,
                                    k.VKey,
                                ) {
                                    agg.key(
                                        raw.header.hDevice.0 as usize,
                                        code,
                                        k.Flags & 1 == 0,
                                        t,
                                    );
                                }
                            } else if raw.header.dwType == RIM_TYPEMOUSE.0 {
                                let m = raw.data.mouse;
                                agg.mouse(
                                    m.lLastX,
                                    m.lLastY,
                                    m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0,
                                    t,
                                );
                            }
                        }
                    }
                }
                WM_INPUT_DEVICE_CHANGE => {
                    if wp.0 == GIDC_REMOVAL as usize {
                        if let Ok(mut a) = runtime.aggregate.lock() {
                            a.release_device(lp.0 as usize);
                        }
                    }
                }
                WM_WTSSESSION_CHANGE | WM_POWERBROADCAST
                    if super::should_reset_held_state(msg, wp.0) =>
                {
                    runtime
                        .sync_requested
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(mut a) = runtime.aggregate.lock() {
                        a.reset_held();
                    }
                }
                WM_TIMER => {
                    // Repair missing key-up only after Windows reports the key released.
                    if let Ok(mut a) = runtime.aggregate.lock() {
                        for scan in 1..=0x7fu16 {
                            for ext in [false, true] {
                                let vk = MapVirtualKeyW(
                                    if ext {
                                        0xe000 | scan as u32
                                    } else {
                                        scan as u32
                                    },
                                    MAPVK_VSC_TO_VK_EX,
                                );
                                if vk != 0 && GetAsyncKeyState(vk as i32) >= 0 {
                                    // Apply the same aliases as WM_INPUT (PrintScreen/SysRq,
                                    // NumLock, Pause) before clearing a possibly lost key-up.
                                    if let Some(physical) =
                                        super::timer_release_code(scan, ext, vk as u16)
                                    {
                                        a.release_code(physical);
                                    }
                                }
                            }
                        }
                    }
                }
                WM_DESTROY => PostQuitMessage(0),
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
    pub fn start(runtime: Arc<Runtime>) -> std::sync::mpsc::Receiver<Result<usize, String>> {
        let (ready, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = RUNTIME.set(runtime.clone());
            let result: windows::core::Result<()> = unsafe {
                (|| {
                    let module = GetModuleHandleW(None)?;
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(proc),
                        hInstance: module.into(),
                        lpszClassName: w!("MouseStatsInput"),
                        ..Default::default()
                    };
                    if RegisterClassW(&class) == 0 {
                        return Err(windows::core::Error::from_thread());
                    }
                    let hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        w!("MouseStatsInput"),
                        w!("MouseStatsInput"),
                        WINDOW_STYLE::default(),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        Some(module.into()),
                        None,
                    )?;
                    RegisterRawInputDevices(
                        &[
                            RAWINPUTDEVICE {
                                usUsagePage: 1,
                                usUsage: 2,
                                dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                                hwndTarget: hwnd,
                            },
                            RAWINPUTDEVICE {
                                usUsagePage: 1,
                                usUsage: 6,
                                dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
                                hwndTarget: hwnd,
                            },
                        ],
                        size_of::<RAWINPUTDEVICE>() as u32,
                    )?;
                    WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION)?;
                    if SetTimer(Some(hwnd), 1, 1000, None) == 0 {
                        return Err(windows::core::Error::from_thread());
                    }
                    let _ = ready.send(Ok(hwnd.0 as usize));
                    let mut msg = MSG::default();
                    loop {
                        let code = GetMessageW(&mut msg, None, 0, 0).0;
                        if code == -1 {
                            return Err(windows::core::Error::from_thread());
                        }
                        if code == 0 {
                            break;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    Ok(())
                })()
            };
            if result.is_err() {
                let _ = ready.send(Err("Raw Input initialization failed".into()));
                runtime.error("Windows 入力計測スレッドが停止しました");
            }
        });
        receiver
    }
}

#[cfg(windows)]
pub use win::start;
#[cfg(not(windows))]
pub fn start(runtime: std::sync::Arc<crate::Runtime>) {
    runtime.error("入力計測は Windows のみ対応しています");
}

#[cfg(test)]
mod event_tests {
    use super::should_reset_held_state;

    #[test]
    fn only_session_transitions_and_suspend_resume_reset_held_keys() {
        for event in 1..=8 {
            assert!(should_reset_held_state(0x02b1, event));
        }
        assert!(!should_reset_held_state(0x02b1, 9)); // remote-control mode
        for event in [4, 6, 7, 18] {
            assert!(should_reset_held_state(0x0218, event));
        }
        for event in [0, 2, 9, 10, 20] {
            assert!(!should_reset_held_state(0x0218, event));
        }
        assert!(!should_reset_held_state(0x1234, 7));
    }
}
