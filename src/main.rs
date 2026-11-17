#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("TrayKit currently targets Windows only");

mod cli;
mod clipboard;
mod config;
mod hotkey;
mod settings;
mod tools;
mod tray;

use anyhow::{Context, Result};
use config::Config;
use std::{
    cell::RefCell,
    io::Write,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tray::NativeBridge;
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE},
        System::{
            Console::{ATTACH_PARENT_PROCESS, AttachConsole},
            Threading::CreateMutexW,
        },
    },
    core::w,
};
use windui::prelude::*;

pub enum AppEvent {
    Run(usize),
    Settings,
    Quit,
    Feedback(Result<String, String>),
}

struct Instance(HANDLE);
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl Instance {
    fn acquire() -> Result<Option<Self>> {
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\TrayKit.66D53E91")) }?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Ok(None);
        }
        Ok(Some(Self(handle)))
    }
}

fn write_cli(mut stream: impl Write, text: &str) {
    if text.is_empty() {
        return;
    }
    let _ = stream.write_all(text.as_bytes());
    if !text.ends_with('\n') {
        let _ = stream.write_all(b"\n");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
        let response = cli::execute(&args);
        write_cli(std::io::stdout().lock(), &response.stdout);
        write_cli(std::io::stderr().lock(), &response.stderr);
        if response.code != 0 {
            std::process::exit(response.code);
        }
        return;
    }
    if let Err(error) = run() {
        eprintln!("TrayKit: {error:#}");
        // Startup failures must remain visible even in the GUI subsystem release binary.
        let text: Vec<u16> = format!("TrayKit 启动失败：{error:#}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                windows::core::PCWSTR(text.as_ptr()),
                w!("TrayKit"),
                windows::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let Some(_instance) = Instance::acquire()? else {
        return Ok(());
    };
    let (config, config_error) = match Config::load() {
        Ok(config) => (config, None),
        Err(error) => (
            Config::default(),
            Some(format!("配置读取失败，使用默认值：{error:#}")),
        ),
    };
    let icon = image::load_from_memory(include_bytes!("../assets/traykit.png"))?.to_rgba8();
    let window_icon = WindowIcon::from_rgba(icon.width(), icon.height(), icon.clone().into_raw())
        .context("Invalid application icon")?;
    let tray_icon = image::imageops::resize(&icon, 32, 32, image::imageops::FilterType::Lanczos3);
    let tools = Arc::new(tools::registered());
    let state = Rc::new(RefCell::new(config.clone()));
    let execution_state = state.clone();
    let busy = Arc::new(AtomicBool::new(false));
    let status = signal(String::from(
        "就绪。工具转换仅在内存中完成，不生成图片文件。",
    ));

    // Resident mode drains channels even with no visible windows (ordinary hidden
    // windows in windui 0.33 do not). Settings are created only when requested.
    let mut app = App::resident("TrayKit").icon(window_icon.clone());
    let settings_factory = Rc::new(RefCell::new(
        None::<Rc<dyn Fn() -> windui::event::WindowRequest>>,
    ));
    let show_settings = settings_factory.clone();
    let tray_handle = app.tray_handle();
    let tx_slot_rx = Rc::new(RefCell::new(None::<Sender<AppEvent>>));
    let (tool_set, running, feedback) = (tools.clone(), busy.clone(), status);
    let receiver_slot = tx_slot_rx.clone();
    let tx = app.channel(move |ctx, event: AppEvent| match event {
        AppEvent::Run(index) => {
            let Some(tool) = tool_set.get(index).cloned() else {
                return;
            };
            if !execution_state.borrow().is_enabled(tool.id()) {
                tray_handle.notify("TrayKit", "此工具已禁用，请先在设置中启用");
                return;
            }
            let summary = match clipboard::summarize() {
                Ok(summary) => summary,
                Err(error) => {
                    tray_handle.notify("TrayKit", format!("无法检查剪贴板状态：{error:#}"));
                    return;
                }
            };
            if let Some(reason) = tool.availability(&summary).reason() {
                tray_handle.notify("TrayKit", reason);
                return;
            }
            if running
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                tray_handle.notify("TrayKit", "工具正在运行，请稍候");
                return;
            }
            feedback.set(format!("正在执行：{}…", tool.name()));
            let sender = receiver_slot.borrow().as_ref().unwrap().clone();
            let finished = running.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("traykit-tool".into())
                .spawn(move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tool.run()));
                    let result = match result {
                        Ok(Ok(message)) => Ok(message),
                        Ok(Err(error)) => Err(format!("{error:#}")),
                        Err(_) => Err("工具发生异常，请重试".into()),
                    };
                    finished.store(false, Ordering::Release);
                    let _ = sender.send(AppEvent::Feedback(result));
                })
            {
                running.store(false, Ordering::Release);
                tray_handle.notify("TrayKit", format!("无法启动工具任务：{error}"));
            }
        }
        AppEvent::Settings => {
            if std::env::var_os("TRAYKIT_DIAGNOSTICS").is_some() {
                eprintln!("[traykit] settings event received");
            }
            let factory = show_settings.borrow().as_ref().cloned();
            if let Some(factory) = factory {
                ctx.open_window(factory());
            }
        }
        AppEvent::Quit => {
            if running.load(Ordering::Acquire) {
                tray_handle.notify("TrayKit", "工具尚未完成，请完成后再退出");
            } else {
                tray_handle.quit();
            }
        }
        AppEvent::Feedback(result) => {
            let (title, message) = match result {
                Ok(message) => ("TrayKit · 完成", message),
                Err(message) => ("TrayKit · 提示", message),
            };
            eprintln!("{title}: {message}");
            feedback.set(message.clone());
            tray_handle.notify(title, message);
        }
    });
    *tx_slot_rx.borrow_mut() = Some(tx.clone());
    let bridge = Rc::new(NativeBridge::new(
        tx.clone(),
        tools
            .iter()
            .map(|tool| {
                config
                    .is_enabled(tool.id())
                    .then(|| tool.name().to_string())
            })
            .collect(),
        &icon,
    )?);
    if let Err(error) = bridge.apply_config(&config, false) {
        let _ = tx.send(AppEvent::Feedback(Err(format!(
            "启动配置未能全部应用：{error:#}。请从托盘打开设置修正冲突。"
        ))));
    }
    if let Some(error) = config_error {
        let _ = tx.send(AppEvent::Feedback(Err(error)));
    }

    let left_bridge = bridge.clone();
    let settings_tx = tx.clone();
    let quit_tx = tx.clone();
    let tray = Tray::new()
        .tooltip("TrayKit · 左键工具 / 右键设置")
        .icon_rgba(32, 32, tray_icon.as_raw())
        .on_left_click(move |_| left_bridge.request_menu())
        .menu(vec![
            TrayMenuItem::item("设置…", move |_| {
                let _ = settings_tx.send(AppEvent::Settings);
            }),
            TrayMenuItem::separator(),
            TrayMenuItem::item("退出 TrayKit", move |_| {
                let _ = quit_tx.send(AppEvent::Quit);
            }),
        ]);

    let settings_bridge = bridge.clone();
    *settings_factory.borrow_mut() = Some(Rc::new(move || {
        let (state, bridge, tx) = (state.clone(), settings_bridge.clone(), tx.clone());
        Window::new("TrayKit 设置", 620, 600)
            .single("settings")
            .icon(window_icon.clone())
            .content(move || settings::view(state, bridge, tx, status))
    }));
    anyhow::ensure!(
        app.tray(tray).run_resident(),
        "WindUI resident host failed to initialize"
    );
    // Break the channel -> settings factory -> sender cycle before native cleanup.
    settings_factory.borrow_mut().take();
    drop(bridge);
    Ok(())
}
