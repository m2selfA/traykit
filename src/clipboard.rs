//! Shared clipboard ownership, bounded locking and preallocated HGLOBAL buffers.
use anyhow::{Result, bail, ensure};
use clipboard_win::{Clipboard, formats};
use serde::Serialize;
use std::{sync::OnceLock, time::Duration};
use windows::{
    Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
        System::{
            DataExchange::{GetClipboardSequenceNumber, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        },
        UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_EX_TOOLWINDOW, WS_POPUP},
    },
    core::w,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ClipboardSummary {
    pub sequence: u32,
    pub unicode_text: bool,
    pub image: bool,
    pub file_drop: bool,
    pub virtual_file: bool,
}

/// Query format availability without opening, locking, reading or changing the clipboard.
pub fn summarize() -> Result<ClipboardSummary> {
    static PNG_FORMAT: OnceLock<Option<u32>> = OnceLock::new();
    static FILE_DESCRIPTOR_FORMAT: OnceLock<Option<u32>> = OnceLock::new();
    static FILE_CONTENTS_FORMAT: OnceLock<Option<u32>> = OnceLock::new();
    let before = unsafe { GetClipboardSequenceNumber() };
    let png = *PNG_FORMAT
        .get_or_init(|| clipboard_win::register_format("PNG").map(|format| format.get()));
    let file_descriptor = *FILE_DESCRIPTOR_FORMAT.get_or_init(|| {
        clipboard_win::register_format("FileGroupDescriptorW").map(|format| format.get())
    });
    let file_contents = *FILE_CONTENTS_FORMAT
        .get_or_init(|| clipboard_win::register_format("FileContents").map(|format| format.get()));
    let summary = ClipboardSummary {
        sequence: before,
        unicode_text: clipboard_win::is_format_avail(formats::CF_UNICODETEXT),
        image: png.is_some_and(clipboard_win::is_format_avail)
            || clipboard_win::is_format_avail(formats::CF_DIBV5)
            || clipboard_win::is_format_avail(formats::CF_DIB)
            || clipboard_win::is_format_avail(formats::CF_BITMAP),
        file_drop: clipboard_win::is_format_avail(formats::CF_HDROP),
        virtual_file: file_descriptor.is_some_and(clipboard_win::is_format_avail)
            && file_contents.is_some_and(clipboard_win::is_format_avail),
    };
    let after = unsafe { GetClipboardSequenceNumber() };
    ensure!(
        before == after,
        "剪贴板在状态检查期间发生变化，请重试（未读取或更改内容）"
    );
    Ok(summary)
}

/// A real owner HWND is required when EmptyClipboard precedes SetClipboardData.
pub(crate) struct Owner(pub HWND);
impl Owner {
    pub(crate) fn new() -> Result<Self> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("TrayKit Clipboard"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }?;
        Ok(Self(hwnd))
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

pub(crate) fn lock(owner: HWND) -> Result<Clipboard> {
    for _ in 0..25 {
        if let Ok(clipboard) = Clipboard::new_for(owner.0.cast()) {
            return Ok(clipboard);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    bail!("剪贴板正被其他程序占用，请稍后重试（未更改原内容）")
}

/// Preallocate every movable buffer before EmptyClipboard; ownership transfers only on successful SetClipboardData.
pub(crate) struct GlobalMemory(HGLOBAL);
impl GlobalMemory {
    pub(crate) fn copy(bytes: &[u8]) -> Result<Self> {
        let memory = Self(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }?);
        let ptr = unsafe { GlobalLock(memory.0) };
        ensure!(!ptr.is_null(), "Cannot lock output clipboard memory");
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
            let _ = GlobalUnlock(memory.0);
        }
        Ok(memory)
    }
    pub(crate) fn publish(&mut self, format: u32) -> Result<()> {
        unsafe { SetClipboardData(format, Some(HANDLE(self.0.0))) }?;
        self.0 = HGLOBAL::default();
        Ok(())
    }
}
impl Drop for GlobalMemory {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = GlobalFree(Some(self.0));
            }
        }
    }
}
