//! WindUI owns the tray and settings UI. This bridge defers native menus outside
//! WindUI callbacks (which hold internal window borrows) and reports real hotkey errors.
use crate::{
    AppEvent,
    config::Config,
    hotkey::{Bindings, Shortcut, Target},
};
use anyhow::{Context, Result};
use std::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicIsize, Ordering},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    },
    core::{PCWSTR, w},
};
use windui::prelude::Sender;

const POPUP: u32 = WM_APP + 91;
const TOOL_BASE: usize = 100;
static MIDDLE_OWNER: AtomicIsize = AtomicIsize::new(0);

struct NativeState {
    tx: Sender<AppEvent>,
    tools: RefCell<Vec<Option<String>>>,
    routes: RefCell<std::collections::BTreeMap<usize, (Shortcut, Target)>>,
    popup_active: Cell<bool>,
    bitmap: HBITMAP,
}

pub struct NativeBridge {
    hwnd: HWND,
    state: Box<NativeState>,
    bindings: RefCell<Bindings>,
    middle_hook: RefCell<Option<HHOOK>>,
}

impl NativeBridge {
    pub fn new(
        tx: Sender<AppEvent>,
        labels: Vec<Option<String>>,
        icon: &image::RgbaImage,
    ) -> Result<Self> {
        let instance = unsafe { GetModuleHandleW(None) }?;
        let class = w!("TrayKit.NativeBridge");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        anyhow::ensure!(
            unsafe { RegisterClassW(&wc) } != 0,
            "Cannot register native event window"
        );
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!("TrayKit Events"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }?;
        let state = Box::new(NativeState {
            tx,
            tools: RefCell::new(labels),
            routes: RefCell::new(Default::default()),
            popup_active: Cell::new(false),
            bitmap: menu_bitmap(icon).unwrap_or_default(),
        });
        unsafe {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                (&*state as *const NativeState) as isize,
            );
        }
        Ok(Self {
            hwnd,
            state,
            bindings: RefCell::new(Bindings::default()),
            middle_hook: RefCell::new(None),
        })
    }

    pub fn request_menu(&self) {
        if let Err(error) = unsafe { PostMessageW(Some(self.hwnd), POPUP, WPARAM(0), LPARAM(0)) } {
            eprintln!("Cannot request tools menu: {error}");
        }
    }

    /// Stage all registrations while keeping old ones alive, then persist and commit together.
    pub fn apply_config(&self, config: &Config, persist: bool) -> Result<()> {
        self.apply_config_with(config, || if persist { config.save() } else { Ok(()) })
    }

    fn apply_config_with(
        &self,
        config: &Config,
        persist: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        config.validate()?;
        let tools = crate::tools::registered();
        let mut wanted = vec![(config.menu_hotkey.parse()?, Target::Menu)];
        for (index, tool) in tools.iter().enumerate() {
            if config.is_enabled(tool.id())
                && let Some(value) = config.tool_hotkeys.get(tool.id())
            {
                wanted.push((value.parse()?, Target::Tool(index)));
            }
        }
        let pending = self.bindings.borrow().prepare(self.hwnd, wanted)?;
        let labels = tools
            .iter()
            .map(|tool| {
                config
                    .is_enabled(tool.id())
                    .then(|| match config.tool_hotkeys.get(tool.id()) {
                        Some(key) => format!("{}\t{}", tool.name(), key),
                        None => tool.name().to_string(),
                    })
            })
            .collect();
        let old_middle = self.middle_hook.borrow().is_some();
        self.set_middle_click(config.middle_click)?;
        if let Err(error) = persist() {
            let rollback = self.set_middle_click(old_middle);
            return Err(error).context(format!(
                "设置未保存；旧热键已保留；中键回滚错误={:?}",
                rollback.err()
            ));
        }
        *self.state.routes.borrow_mut() = self.bindings.borrow_mut().commit(pending);
        *self.state.tools.borrow_mut() = labels;
        Ok(())
    }

    pub fn set_middle_click(&self, enabled: bool) -> Result<()> {
        if enabled == self.middle_hook.borrow().is_some() {
            return Ok(());
        }
        if enabled {
            let module = unsafe { GetModuleHandleW(None) }?;
            let hook =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(module.into()), 0) }
                    .context("Cannot install middle-click hook")?;
            MIDDLE_OWNER.store(self.hwnd.0 as isize, Ordering::Release);
            *self.middle_hook.borrow_mut() = Some(hook);
        } else {
            let hook = self.middle_hook.borrow().expect("enabled hook");
            unsafe { UnhookWindowsHookEx(hook) }?;
            self.middle_hook.borrow_mut().take();
            MIDDLE_OWNER.store(0, Ordering::Release);
        }
        Ok(())
    }
}

impl Drop for NativeBridge {
    fn drop(&mut self) {
        let _ = self.set_middle_click(false);
        *self.bindings.borrow_mut() = Bindings::default();
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            if !self.state.bitmap.is_invalid() {
                let _ = DeleteObject(self.state.bitmap.into());
            }
        }
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && wp.0 as u32 == WM_MBUTTONUP {
        // Do no menu work inside a low-level hook; ignore injected input and pass the click on.
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        if info.flags & 1 == 0 {
            let owner = MIDDLE_OWNER.load(Ordering::Acquire);
            if owner != 0 {
                unsafe {
                    let _ = PostMessageW(Some(HWND(owner as *mut _)), POPUP, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const NativeState).as_ref() };
    if let Some(state) = state {
        match msg {
            WM_HOTKEY => {
                let route = state
                    .routes
                    .borrow()
                    .get(&wp.0)
                    .copied()
                    .filter(|(shortcut, _)| {
                        let bits = lp.0 as u32;
                        bits & 0xFFFF == shortcut.modifiers && bits >> 16 == shortcut.key
                    })
                    .map(|(_, target)| target);
                match route {
                    Some(Target::Menu) => unsafe {
                        let _ = PostMessageW(Some(hwnd), POPUP, WPARAM(0), LPARAM(0));
                    },
                    Some(Target::Tool(index)) => {
                        let _ = state.tx.send(AppEvent::Run(index));
                    }
                    None => {} // Ignore stale messages from registrations removed at Save.
                }
                return LRESULT(0);
            }
            POPUP => {
                if !state.popup_active.replace(true) {
                    if let Err(error) = popup(hwnd, state) {
                        let _ = state
                            .tx
                            .send(AppEvent::Feedback(Err(format!("菜单打开失败：{error:#}"))));
                    }
                    state.popup_active.set(false);
                }
                return LRESULT(0);
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

struct Menu(HMENU);
impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

fn popup(hwnd: HWND, state: &NativeState) -> Result<()> {
    let menu = Menu(unsafe { CreatePopupMenu() }?);
    let tools = state.tools.borrow().clone();
    let clipboard = crate::clipboard::summarize();
    let registry = crate::tools::registered();
    let mut position = 0;
    for (index, name) in tools.iter().enumerate() {
        let Some(name) = name else { continue };
        let label: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let available = clipboard.as_ref().is_ok_and(|summary| {
            registry
                .get(index)
                .is_some_and(|tool| tool.availability(summary).is_available())
        });
        let flags = if available {
            MF_STRING
        } else {
            MF_STRING | MF_GRAYED
        };
        unsafe { AppendMenuW(menu.0, flags, TOOL_BASE + index, PCWSTR(label.as_ptr())) }?;
        if !state.bitmap.is_invalid() {
            let info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_BITMAP,
                hbmpItem: state.bitmap,
                ..Default::default()
            };
            unsafe { SetMenuItemInfoW(menu.0, position, true, &info) }?;
        }
        position += 1;
    }
    if position == 0 {
        unsafe {
            AppendMenuW(
                menu.0,
                MF_STRING | MF_GRAYED,
                0,
                w!("未启用任何工具，请在设置中启用"),
            )
        }?;
    } else if clipboard.is_err() {
        unsafe {
            AppendMenuW(
                menu.0,
                MF_STRING | MF_GRAYED,
                0,
                w!("剪贴板状态不可用，请稍后重试"),
            )
        }?;
    }
    unsafe {
        AppendMenuW(menu.0, MF_SEPARATOR, 0, PCWSTR::null())?;
        AppendMenuW(menu.0, MF_STRING, 1, w!("设置…"))?;
        AppendMenuW(menu.0, MF_STRING, 2, w!("退出 TrayKit"))?;
        let mut point = POINT::default();
        GetCursorPos(&mut point)?;
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            Some(0),
            hwnd,
            None,
        )
        .0 as usize;
        if std::env::var_os("TRAYKIT_DIAGNOSTICS").is_some() {
            eprintln!("[traykit] menu command={command}");
        }
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        match command {
            1 => {
                let _ = state.tx.send(AppEvent::Settings);
            }
            2 => {
                let _ = state.tx.send(AppEvent::Quit);
            }
            id if id >= TOOL_BASE
                && id < TOOL_BASE + tools.len()
                && tools[id - TOOL_BASE].is_some() =>
            {
                let _ = state.tx.send(AppEvent::Run(id - TOOL_BASE));
            }
            _ => {}
        }
    }
    Ok(())
}

fn menu_bitmap(icon: &image::RgbaImage) -> Result<HBITMAP> {
    let icon = image::imageops::resize(icon, 20, 20, image::imageops::FilterType::Lanczos3);
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of_val(&info.bmiHeader) as u32;
    info.bmiHeader.biWidth = 20;
    info.bmiHeader.biHeight = -20;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut pixels = std::ptr::null_mut();
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut pixels, None, 0) }?;
    // Native menus expect premultiplied BGRA in a 32-bit DIB section.
    for (index, pixel) in icon.pixels().enumerate() {
        let a = u16::from(pixel[3]);
        let bgra = [
            ((u16::from(pixel[2]) * a + 127) / 255) as u8,
            ((u16::from(pixel[1]) * a + 127) / 255) as u8,
            ((u16::from(pixel[0]) * a + 127) / 255) as u8,
            pixel[3],
        ];
        unsafe {
            std::ptr::copy_nonoverlapping(bgra.as_ptr(), pixels.cast::<u8>().add(index * 4), 4);
        }
    }
    Ok(bitmap)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_config_filters_disables_swaps_and_rolls_back_persistence_failure() {
        let mut app = windui::prelude::App::resident("TrayKit configuration test");
        let tx = app.channel(|_, _: AppEvent| {});
        let tools = crate::tools::registered();
        let bridge = NativeBridge::new(
            tx,
            tools.iter().map(|t| Some(t.name().into())).collect(),
            &image::RgbaImage::new(1, 1),
        )
        .unwrap();
        let mut free = Vec::new();
        for key in 0x70..=0x7B {
            let shortcut = Shortcut { modifiers: 7, key };
            if let Ok(guard) =
                crate::hotkey::Registration::new(bridge.hwnd, 3000 + key as i32, shortcut)
            {
                free.push((format!("Ctrl+Alt+Shift+F{}", key - 0x70 + 1), guard));
                if free.len() == 3 {
                    break;
                }
            }
        }
        assert_eq!(free.len(), 3, "Need 3 spare Ctrl+Alt+Shift shortcuts");
        let (third, third_guard) = free.pop().unwrap();
        let (second, second_guard) = free.pop().unwrap();
        let (first, first_guard) = free.pop().unwrap();
        drop((first_guard, second_guard, third_guard));
        let mut config = Config {
            menu_hotkey: first.clone(),
            ..Config::default()
        };
        config
            .tool_hotkeys
            .insert("clipboard-image".into(), second.clone());
        bridge.apply_config(&config, false).unwrap();
        let original_routes = bridge.state.routes.borrow().clone();
        let original_labels = bridge.state.tools.borrow().clone();
        let mut changed = config.clone();
        changed.menu_hotkey = third.clone();
        changed.middle_click = true;
        changed.disabled_tools.insert("clipboard-image".into());
        assert!(
            bridge
                .apply_config_with(&changed, || Err(anyhow::anyhow!("injected disk failure")))
                .is_err()
        );
        assert_eq!(*bridge.state.routes.borrow(), original_routes);
        assert_eq!(*bridge.state.tools.borrow(), original_labels);
        assert!(bridge.middle_hook.borrow().is_none());
        let _third_free =
            crate::hotkey::Registration::new(bridge.hwnd, 4000, third.parse().unwrap()).unwrap();
        config.disabled_tools.insert("clipboard-image".into());
        bridge.apply_config(&config, false).unwrap();
        assert!(bridge.state.tools.borrow()[0].is_none());
        assert_eq!(bridge.state.routes.borrow().len(), 1);
        let probe =
            crate::hotkey::Registration::new(bridge.hwnd, 4001, second.parse().unwrap()).unwrap();
        drop(probe);
        config.disabled_tools.clear();
        config.menu_hotkey = second.clone();
        config.tool_hotkeys.insert("clipboard-image".into(), first);
        bridge.apply_config(&config, false).unwrap();
        assert_eq!(bridge.state.routes.borrow().len(), 2);
        for tool in &tools {
            config.disabled_tools.insert(tool.id().into());
        }
        bridge.apply_config(&config, false).unwrap();
        assert!(bridge.state.tools.borrow().iter().all(Option::is_none));
        assert_eq!(bridge.state.routes.borrow().len(), 1);
    }
}
