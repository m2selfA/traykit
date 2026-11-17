use super::{ActionCategory, InputKind, OutputKind, SideEffect, Tool, ToolDescriptor};
use crate::clipboard::{GlobalMemory, Owner, lock};
use anyhow::{Context, Result, bail, ensure};
use clipboard_win::{formats, raw};
use image::{ImageEncoder, ImageReader, RgbaImage};
use std::{
    fs::File,
    io::{Cursor, Read},
    mem::size_of,
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::{HGLOBAL, HWND},
    Graphics::Gdi::{BITMAP, GetObjectW, HGDIOBJ},
    System::{
        Com::{DVASPECT_CONTENT, FORMATETC, IStream, STGMEDIUM, TYMED_HGLOBAL, TYMED_ISTREAM},
        DataExchange::{GetClipboardData, GetClipboardSequenceNumber},
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{OleGetClipboard, OleInitialize, OleUninitialize, ReleaseStgMedium},
    },
    UI::Shell::{FD_FILESIZE, FILEDESCRIPTORW},
};

const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;

pub struct ClipboardImage;
impl Tool for ClipboardImage {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "clipboard-image",
            name: "修复剪贴板图片",
            description: "将内存图片或图片文件引用转换为可直接粘贴的图片，不生成文件",
            category: ActionCategory::Image,
            accepted_inputs: &[
                InputKind::Image,
                InputKind::FileDrop,
                InputKind::VirtualFile,
            ],
            output: OutputKind::Image,
            side_effects: &[
                SideEffect::ClipboardRead,
                SideEffect::ClipboardWrite,
                SideEffect::ReferencedFileRead,
            ],
            supports_preview: false,
            supports_cancel: false,
        }
    }

    fn run(&self) -> Result<String> {
        convert()
    }
}

struct Snapshot {
    sequence: u32,
    images: Vec<(String, Vec<u8>)>,
    files: Vec<PathBuf>,
    errors: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct VirtualDescriptor {
    index: i32,
    name: String,
    size: Option<u64>,
}

struct OleGuard;
impl OleGuard {
    fn new() -> Result<Self> {
        unsafe { OleInitialize(None) }.context("Cannot initialize OLE clipboard access")?;
        Ok(Self)
    }
}
impl Drop for OleGuard {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct Medium(STGMEDIUM);
impl Drop for Medium {
    fn drop(&mut self) {
        unsafe { ReleaseStgMedium(&mut self.0) };
    }
}

fn copy_hglobal(handle: HGLOBAL, exact_size: Option<usize>) -> Result<Vec<u8>> {
    let available = unsafe { GlobalSize(handle) };
    ensure!(available > 0, "Virtual clipboard data is empty");
    let size = exact_size.unwrap_or(available);
    ensure!(
        size > 0 && size <= MAX_BYTES && size <= available,
        "Virtual clipboard data exceeds its declared buffer or the 128 MiB limit"
    );
    let pointer = unsafe { GlobalLock(handle) };
    ensure!(!pointer.is_null(), "Cannot lock virtual clipboard data");
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) }.to_vec();
    unsafe {
        let _ = GlobalUnlock(handle);
    }
    Ok(bytes)
}

fn parse_virtual_descriptors(bytes: &[u8]) -> Result<Vec<VirtualDescriptor>> {
    ensure!(bytes.len() >= 4, "Truncated FileGroupDescriptorW header");
    let count = u32::from_le_bytes(bytes[..4].try_into()?) as usize;
    ensure!(
        count <= 256,
        "Virtual clipboard file list exceeds 256 items"
    );
    let descriptor_size = size_of::<FILEDESCRIPTORW>();
    let required = 4usize
        .checked_add(
            count
                .checked_mul(descriptor_size)
                .context("Virtual descriptor size overflow")?,
        )
        .context("Virtual descriptor size overflow")?;
    ensure!(
        bytes.len() >= required,
        "Truncated FileGroupDescriptorW payload"
    );
    let mut descriptors = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 4 + index * descriptor_size;
        let descriptor = unsafe {
            std::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<FILEDESCRIPTORW>())
        };
        let units = descriptor.cFileName;
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        let name = String::from_utf16(&units[..end]).context("Invalid virtual filename UTF-16")?;
        let size = if descriptor.dwFlags & FD_FILESIZE.0 as u32 != 0 {
            Some((u64::from(descriptor.nFileSizeHigh) << 32) | u64::from(descriptor.nFileSizeLow))
        } else {
            None
        };
        descriptors.push(VirtualDescriptor {
            index: index as i32,
            name,
            size,
        });
    }
    Ok(descriptors)
}

fn choose_virtual_image(descriptors: &[VirtualDescriptor]) -> Result<Option<&VirtualDescriptor>> {
    let mut rejected_size = false;
    for descriptor in descriptors {
        if !supported_file(Path::new(&descriptor.name)) {
            continue;
        }
        if descriptor
            .size
            .is_some_and(|size| size == 0 || size > MAX_BYTES as u64)
        {
            rejected_size = true;
            continue;
        }
        return Ok(Some(descriptor));
    }
    if rejected_size {
        bail!("Virtual image is empty or exceeds the 128 MiB input limit");
    }
    Ok(None)
}

fn read_stream(stream: &IStream, expected_size: Option<usize>) -> Result<Vec<u8>> {
    if let Some(size) = expected_size {
        ensure!(
            size > 0 && size <= MAX_BYTES,
            "Virtual image is empty or exceeds the 128 MiB input limit"
        );
    }
    let mut output = Vec::with_capacity(expected_size.unwrap_or(64 * 1024));
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let request = if let Some(expected) = expected_size {
            expected.saturating_sub(output.len()).min(chunk.len())
        } else {
            (MAX_BYTES + 1 - output.len()).min(chunk.len())
        };
        if request == 0 {
            break;
        }
        let mut read = 0u32;
        unsafe {
            stream
                .Read(chunk.as_mut_ptr().cast(), request as u32, Some(&mut read))
                .ok()
        }
        .context("Cannot read virtual FileContents stream")?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read as usize]);
        ensure!(
            output.len() <= MAX_BYTES,
            "Virtual image exceeds the 128 MiB input limit"
        );
    }
    if let Some(expected) = expected_size {
        ensure!(
            output.len() == expected,
            "Virtual image ended before its declared size"
        );
    }
    ensure!(!output.is_empty(), "Virtual image content is empty");
    Ok(output)
}

fn read_medium(medium: &Medium, expected_size: Option<usize>) -> Result<Vec<u8>> {
    match medium.0.tymed {
        value if value == TYMED_HGLOBAL.0 as u32 => {
            let handle = unsafe { medium.0.u.hGlobal };
            copy_hglobal(handle, expected_size)
        }
        value if value == TYMED_ISTREAM.0 as u32 => {
            let stream = unsafe {
                let pointer = std::ptr::addr_of!(medium.0.u.pstm).cast::<Option<IStream>>();
                (&*pointer).clone()
            }
            .context("Virtual FileContents returned an empty IStream")?;
            read_stream(&stream, expected_size)
        }
        value => bail!("Unsupported virtual FileContents storage medium: {value}"),
    }
}

fn read_virtual_image() -> Result<Option<(String, Vec<u8>)>> {
    let _ole = OleGuard::new()?;
    let data = unsafe { OleGetClipboard() }.context("Cannot access OLE clipboard data object")?;
    let descriptor_format = clipboard_win::register_format("FileGroupDescriptorW")
        .context("Cannot register FileGroupDescriptorW")?
        .get();
    let contents_format = clipboard_win::register_format("FileContents")
        .context("Cannot register FileContents")?
        .get();
    let descriptor_request = FORMATETC {
        cfFormat: u16::try_from(descriptor_format).context("Invalid descriptor format ID")?,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let descriptor_medium = Medium(
        unsafe { data.GetData(&descriptor_request) }.context("Cannot read FileGroupDescriptorW")?,
    );
    ensure!(
        descriptor_medium.0.tymed == TYMED_HGLOBAL.0 as u32,
        "FileGroupDescriptorW was not returned as HGLOBAL"
    );
    let descriptor_bytes = copy_hglobal(unsafe { descriptor_medium.0.u.hGlobal }, None)?;
    let descriptors = parse_virtual_descriptors(&descriptor_bytes)?;
    let Some(descriptor) = choose_virtual_image(&descriptors)? else {
        return Ok(None);
    };
    let expected_size = descriptor.size.map(usize::try_from).transpose()?;
    let contents_request = FORMATETC {
        cfFormat: u16::try_from(contents_format).context("Invalid contents format ID")?,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: descriptor.index,
        tymed: (TYMED_HGLOBAL.0 | TYMED_ISTREAM.0) as u32,
    };
    let contents_medium = Medium(
        unsafe { data.GetData(&contents_request) }
            .with_context(|| format!("Cannot read virtual image {}", descriptor.name))?,
    );
    let bytes = read_medium(&contents_medium, expected_size)?;
    Ok(Some((format!("VirtualFile（{}）", descriptor.name), bytes)))
}

fn read_global(format: u32) -> Result<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) }?;
    let size = unsafe { GlobalSize(HGLOBAL(handle.0)) };
    ensure!(
        size > 0 && size <= MAX_BYTES,
        "Clipboard format exceeds the 128 MiB input limit or is empty"
    );
    let mut bytes = Vec::new();
    raw::get_vec(format, &mut bytes)
        .map_err(|e| anyhow::anyhow!("Read clipboard format {format}: {e}"))?;
    Ok(bytes)
}

fn snapshot(owner: HWND) -> Result<Snapshot> {
    let _clipboard = lock(owner)?;
    let mut images = Vec::new();
    let mut errors = Vec::new();
    if let Some(png) = clipboard_win::register_format("PNG")
        && clipboard_win::is_format_avail(png.get())
    {
        match read_global(png.get()) {
            Ok(data) => images.push(("PNG".into(), data)),
            Err(e) => errors.push(e.to_string()),
        }
    }
    for (name, format) in [("DIBV5", formats::CF_DIBV5), ("DIB", formats::CF_DIB)] {
        if clipboard_win::is_format_avail(format) {
            match read_global(format).and_then(|data| dib_to_bmp(&data)) {
                Ok(data) => images.push((name.into(), data)),
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
    }
    if clipboard_win::is_format_avail(formats::CF_BITMAP) {
        let bitmap = (|| -> Result<Vec<u8>> {
            let handle = unsafe { GetClipboardData(formats::CF_BITMAP) }?;
            let mut info = BITMAP::default();
            let count = unsafe {
                GetObjectW(
                    HGDIOBJ(handle.0),
                    std::mem::size_of::<BITMAP>() as i32,
                    Some((&mut info as *mut BITMAP).cast()),
                )
            };
            ensure!(
                count != 0 && info.bmWidth > 0 && info.bmHeight > 0,
                "Invalid clipboard bitmap"
            );
            check_dimensions(info.bmWidth as u32, info.bmHeight as u32)?;
            let mut bytes = Vec::new();
            raw::get_bitmap(&mut bytes).map_err(|e| anyhow::anyhow!("Read CF_BITMAP: {e}"))?;
            Ok(bytes)
        })();
        match bitmap {
            Ok(data) => images.push(("Bitmap".into(), data)),
            Err(e) => errors.push(e.to_string()),
        }
    }
    let files = if clipboard_win::is_format_avail(formats::CF_HDROP) {
        let file_list = (|| -> Result<Vec<PathBuf>> {
            // Bound the file count before the library allocates or enumerates paths.
            let drop_handle = unsafe { GetClipboardData(formats::CF_HDROP) }?;
            ensure!(
                unsafe { GlobalSize(HGLOBAL(drop_handle.0)) } <= MAX_BYTES,
                "FileDrop payload exceeds limit"
            );
            let count = unsafe {
                windows::Win32::UI::Shell::DragQueryFileW(
                    windows::Win32::UI::Shell::HDROP(drop_handle.0),
                    u32::MAX,
                    None,
                )
            };
            ensure!(count <= 256, "剪贴板文件列表超过 256 项，未执行转换");
            // Read Unicode paths, including non-BMP names, without lossy conversion.
            clipboard_win::get::<Vec<PathBuf>, _>(formats::FileList)
                .map_err(|e| anyhow::anyhow!("FileDrop: {e}"))
        })();
        match file_list {
            Ok(paths) => paths,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    drop(_clipboard);
    let before_virtual = unsafe { GetClipboardSequenceNumber() };
    if clipboard_win::register_format("FileGroupDescriptorW")
        .is_some_and(|format| clipboard_win::is_format_avail(format.get()))
        && clipboard_win::register_format("FileContents")
            .is_some_and(|format| clipboard_win::is_format_avail(format.get()))
    {
        match read_virtual_image() {
            Ok(Some(image)) => images.push(image),
            Ok(None) => errors.push("虚拟文件列表中没有受支持的图片".into()),
            Err(error) => errors.push(format!("VirtualFile: {error:#}")),
        }
    }
    // Read after delayed OLE formats have been materialized, then bind write-back to this sequence.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    ensure!(
        sequence != 0,
        "Cannot obtain clipboard sequence; content left unchanged"
    );
    ensure!(
        before_virtual == sequence,
        "读取虚拟文件期间剪贴板发生变化，已取消转换（未更改新内容）"
    );
    Ok(Snapshot {
        sequence,
        images,
        files,
        errors,
    })
}

fn check_dimensions(width: u32, height: u32) -> Result<()> {
    ensure!(
        width > 0
            && height > 0
            && width <= MAX_DIMENSION
            && height <= MAX_DIMENSION
            && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "Image dimensions exceed the 32-megapixel / 16384-pixel safety limit"
    );
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<RgbaImage> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_BYTES,
        "Invalid image byte length"
    );
    let (width, height) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    check_dimensions(width, height)?;
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_BYTES as u64);
    reader.limits(limits);
    let image = reader.decode().context("Cannot decode image from memory")?;
    check_dimensions(image.width(), image.height())?;
    Ok(image.to_rgba8())
}

fn supported_file(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        ["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"]
            .iter()
            .any(|ext| s.eq_ignore_ascii_case(ext))
    })
}

fn choose_image(snapshot: &Snapshot) -> Result<(RgbaImage, String)> {
    let mut errors = snapshot.errors.clone();
    for (name, bytes) in &snapshot.images {
        match decode(bytes) {
            Ok(image) => return Ok((image, name.clone())),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    for path in snapshot.files.iter().filter(|path| supported_file(path)) {
        let file_image = (|| -> Result<RgbaImage> {
            let file =
                File::open(path).context("图片引用不可读取（可能已过期或来自其他 RDP 会话）")?;
            ensure!(
                file.metadata()?.len() <= MAX_BYTES as u64,
                "Image file exceeds 128 MiB limit"
            );
            let mut bytes = Vec::new();
            file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
            // Decode only the completely read buffer. No image::open, temporary paths, or save calls.
            decode(&bytes)
        })();
        match file_image {
            Ok(image) => return Ok((image, "FileDrop（首个可读图片）".into())),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if errors.is_empty() {
        bail!("剪贴板中没有可识别的图片或可读取的图片文件引用（未更改原内容）");
    }
    bail!("无法读取剪贴板图片，原内容未更改：{}", errors.join("；"))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .context("Truncated DIB header")?
            .try_into()?,
    ))
}

fn dib_to_bmp(dib: &[u8]) -> Result<Vec<u8>> {
    let header = u32_at(dib, 0)? as usize;
    ensure!(
        [40, 52, 56, 108, 124].contains(&header) && dib.len() >= header,
        "Unsupported or truncated DIB header"
    );
    let bits = u16::from_le_bytes(dib[14..16].try_into()?);
    let used = u32_at(dib, 32)? as usize;
    let palette = if used > 0 {
        used
    } else if bits <= 8 {
        1usize << bits
    } else {
        0
    };
    ensure!(palette <= 256, "Invalid DIB palette length");
    let compression = u32_at(dib, 16)?;
    let masks = if header == 40 {
        match compression {
            3 => 12,
            6 => 16,
            _ => 0,
        }
    } else {
        0
    };
    let offset = 14 + header + masks + palette * 4;
    ensure!(
        offset <= dib.len() + 14 && dib.len() <= MAX_BYTES,
        "Truncated or oversized DIB"
    );
    let mut bmp = Vec::with_capacity(dib.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    Ok(bmp)
}

fn put32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn encode_dib(image: &RgbaImage, alpha: bool) -> Vec<u8> {
    let (width, height) = image.dimensions();
    let header = if alpha { 124 } else { 40 };
    let stride = if alpha {
        width as usize * 4
    } else {
        (width as usize * 3 + 3) & !3
    };
    let mut data = vec![0; header + stride * height as usize];
    put32(&mut data, 0, header as u32);
    put32(&mut data, 4, width);
    put32(&mut data, 8, height); // Positive height: bottom-up, including Office compatibility.
    data[12..14].copy_from_slice(&1u16.to_le_bytes());
    data[14..16].copy_from_slice(&(if alpha { 32u16 } else { 24u16 }).to_le_bytes());
    put32(&mut data, 20, (stride * height as usize) as u32);
    if alpha {
        put32(&mut data, 16, 3);
        for (offset, value) in [
            (40, 0x00ff0000),
            (44, 0x0000ff00),
            (48, 0x000000ff),
            (52, 0xff000000),
            (56, 0x73524742),
            (108, 4),
        ] {
            put32(&mut data, offset, value);
        }
    }
    for y in 0..height {
        for x in 0..width {
            let pixel = image.get_pixel(x, y).0;
            let dst = header
                + (height - 1 - y) as usize * stride
                + x as usize * if alpha { 4 } else { 3 };
            for (channel, source) in [2, 1, 0].into_iter().enumerate() {
                data[dst + channel] = if alpha {
                    pixel[source]
                } else {
                    ((u32::from(pixel[source]) * u32::from(pixel[3])
                        + 255 * (255 - u32::from(pixel[3]))
                        + 127)
                        / 255) as u8
                };
            }
            if alpha {
                data[dst + 3] = pixel[3];
            }
        }
    }
    data
}

pub fn convert() -> Result<String> {
    let owner = Owner::new()?;
    let snapshot = snapshot(owner.0)?;
    let (image, source) = choose_image(&snapshot)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    let png_format = clipboard_win::register_format("PNG")
        .context("Cannot register PNG clipboard format")?
        .get();
    let mut outputs = [
        (
            formats::CF_DIB,
            GlobalMemory::copy(&encode_dib(&image, false))?,
        ),
        (
            formats::CF_DIBV5,
            GlobalMemory::copy(&encode_dib(&image, true))?,
        ),
        (png_format, GlobalMemory::copy(&png)?),
    ];
    let _clipboard = lock(owner.0)?;
    ensure!(
        unsafe { GetClipboardSequenceNumber() } == snapshot.sequence,
        "转换期间剪贴板已被其他程序更改，已取消写回（保留新内容）"
    );
    clipboard_win::empty().map_err(|e| anyhow::anyhow!("Cannot clear clipboard: {e}"))?;
    let mut published = 0;
    let mut failures = Vec::new();
    for (format, memory) in &mut outputs {
        match memory.publish(*format) {
            Ok(()) => published += 1,
            Err(e) => failures.push(e.to_string()),
        }
    }
    ensure!(
        published > 0,
        "Windows refused all clipboard writes after clearing: {}",
        failures.join("; ")
    );
    if !failures.is_empty() {
        eprintln!(
            "Some clipboard representations could not be published: {}",
            failures.join("; ")
        );
    }
    Ok(format!(
        "已转换 {} × {} 图片，来源：{}，写入 {} 种图片格式（无新增文件）",
        image.width(),
        image.height(),
        source,
        published
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> RgbaImage {
        RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 30, 60, 90, 128,
            ],
        )
        .unwrap()
    }
    #[test]
    fn dib_bottom_up_padding_and_v5_alpha_roundtrip() {
        let source = sample();
        let rgb = decode(&dib_to_bmp(&encode_dib(&source, false)).unwrap()).unwrap();
        let rgba = decode(&dib_to_bmp(&encode_dib(&source, true)).unwrap()).unwrap();
        assert_eq!(rgba, source);
        assert_eq!(rgb.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(rgb.get_pixel(0, 1).0, [0, 0, 255, 255]);
        assert_eq!(rgb.get_pixel(1, 1).0, [142, 157, 172, 255]);
    }
    #[test]
    fn memory_image_precedes_files_and_bad_format_falls_back() {
        let bytes = dib_to_bmp(&encode_dib(&sample(), true)).unwrap();
        let snapshot = Snapshot {
            sequence: 1,
            images: vec![("bad".into(), vec![1, 2, 3]), ("DIBV5".into(), bytes)],
            files: vec!["missing.png".into()],
            errors: Vec::new(),
        };
        let (image, source) = choose_image(&snapshot).unwrap();
        assert_eq!(image, sample());
        assert_eq!(source, "DIBV5");
    }
    fn virtual_descriptors(entries: &[(&str, u64)]) -> Vec<u8> {
        const DESCRIPTOR_BYTES: usize = 592;
        const FLAGS_OFFSET: usize = 0;
        const SIZE_HIGH_OFFSET: usize = 64;
        const SIZE_LOW_OFFSET: usize = 68;
        const NAME_OFFSET: usize = 72;
        let mut bytes = vec![0; 4 + DESCRIPTOR_BYTES * entries.len()];
        bytes[..4].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for (index, (name, size)) in entries.iter().enumerate() {
            let descriptor = 4 + index * DESCRIPTOR_BYTES;
            bytes[descriptor + FLAGS_OFFSET..descriptor + FLAGS_OFFSET + 4]
                .copy_from_slice(&0x40u32.to_le_bytes());
            bytes[descriptor + SIZE_HIGH_OFFSET..descriptor + SIZE_HIGH_OFFSET + 4]
                .copy_from_slice(&((*size >> 32) as u32).to_le_bytes());
            bytes[descriptor + SIZE_LOW_OFFSET..descriptor + SIZE_LOW_OFFSET + 4]
                .copy_from_slice(&(*size as u32).to_le_bytes());
            for (offset, unit) in name.encode_utf16().enumerate() {
                let start = descriptor + NAME_OFFSET + offset * 2;
                bytes[start..start + 2].copy_from_slice(&unit.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn virtual_file_descriptors_select_a_bounded_image_by_lindex() {
        let descriptors = parse_virtual_descriptors(&virtual_descriptors(&[
            ("notes.txt", 20),
            ("截图.png", 1_234),
        ]))
        .unwrap();
        let image = choose_virtual_image(&descriptors).unwrap().unwrap();
        assert_eq!(image.index, 1);
        assert_eq!(image.name, "截图.png");
        assert_eq!(image.size, Some(1_234));

        let oversized =
            parse_virtual_descriptors(&virtual_descriptors(&[("large.png", MAX_BYTES as u64 + 1)]))
                .unwrap();
        assert!(choose_virtual_image(&oversized).is_err());
        assert!(parse_virtual_descriptors(&[1, 0, 0, 0]).is_err());
    }

    #[test]
    fn reads_real_com_memory_stream_with_and_without_declared_size() {
        let payload = b"virtual-image-bytes";
        let exact = unsafe { windows::Win32::UI::Shell::SHCreateMemStream(Some(payload)) }.unwrap();
        assert_eq!(read_stream(&exact, Some(payload.len())).unwrap(), payload);

        let unknown =
            unsafe { windows::Win32::UI::Shell::SHCreateMemStream(Some(payload)) }.unwrap();
        assert_eq!(read_stream(&unknown, None).unwrap(), payload);

        let truncated =
            unsafe { windows::Win32::UI::Shell::SHCreateMemStream(Some(payload)) }.unwrap();
        assert!(read_stream(&truncated, Some(payload.len() + 1)).is_err());
    }

    #[test]
    fn rejects_bad_headers_and_extreme_dimensions() {
        assert!(dib_to_bmp(&[0; 4]).is_err());
        assert!(decode(&[1, 2, 3]).is_err());
        assert!(check_dimensions(0, 1).is_err());
        assert!(check_dimensions(16_384, 16_384).is_err());
        assert!(check_dimensions(32, 32).is_ok());
        assert!(supported_file(Path::new("图像.PNG")));
        assert!(!supported_file(Path::new("bad.exe")));
    }
}
